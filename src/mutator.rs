use std::collections::BTreeMap;
use std::path::Path;

use k8s_openapi::api::core::v1::{Container, EmptyDirVolumeSource, Pod, Volume, VolumeMount};
use kube::api::{Api, DynamicObject, ListParams};
use kube::core::ApiResource;
use sha2::{Digest, Sha256};
use tracing::{error, info};

use crate::types::{BinaryPatch, PatchRuleSpec};

const TOOLS_VOLUME_NAME: &str = "patcher-tools";
const BINS_VOLUME_NAME: &str = "patched-bins";
const TOOLS_MOUNT_PATH: &str = "/patch-tools";
const BINS_MOUNT_PATH: &str = "/patched-bins";
const DEFAULT_PATCHER_IMAGE: &str = "ghcr.io/cfi2017/patcherd/patcher:latest";
const ANNOTATION_INJECTED: &str = "patcher.k8s.io/injected";

/// List PatchRules in the given namespace whose selector matches the pod labels.
pub async fn matching_rules(
    client: &kube::Client,
    namespace: &str,
    pod_labels: &BTreeMap<String, String>,
) -> anyhow::Result<Vec<PatchRuleSpec>> {
    let ar = ApiResource {
        group: "patcher.k8s.io".into(),
        version: "v1alpha1".into(),
        api_version: "patcher.k8s.io/v1alpha1".into(),
        kind: "PatchRule".into(),
        plural: "patchrules".into(),
    };
    let api: Api<DynamicObject> = Api::namespaced_with(client.clone(), namespace, &ar);
    let list = api.list(&ListParams::default()).await?;

    let mut matched = Vec::new();
    for rule in list.items {
        if let Some(spec_value) = rule.data.get("spec") {
            match serde_json::from_value::<PatchRuleSpec>(spec_value.clone()) {
                Ok(spec) => {
                    if spec.selector.matches(pod_labels) {
                        matched.push(spec);
                    }
                }
                Err(e) => {
                    error!("failed to parse PatchRule spec: {}", e);
                }
            }
        }
    }
    Ok(matched)
}

/// Mutate a pod in-place: add volumes, init containers, and volume mounts.
pub fn inject(pod: &mut Pod, rules: &[PatchRuleSpec]) -> anyhow::Result<()> {
    let spec = pod
        .spec
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("pod has no spec"))?;

    // Determine patcher image
    let patcher_image = rules
        .iter()
        .find_map(|r| r.patcher_image.as_deref())
        .unwrap_or(DEFAULT_PATCHER_IMAGE);

    // Ensure shared volumes exist
    ensure_volume(spec, TOOLS_VOLUME_NAME);
    ensure_volume(spec, BINS_VOLUME_NAME);

    // Prepend the patcher-install init container
    ensure_patcher_install(spec, patcher_image);

    // Apply each patch from each rule
    for rule in rules {
        for patch in &rule.patches {
            apply_patch(spec, patch)?;
        }
    }

    // Mark pod as injected
    let annotations = pod.metadata.annotations.get_or_insert_with(BTreeMap::new);
    annotations.insert(ANNOTATION_INJECTED.to_string(), "true".to_string());

    Ok(())
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn ensure_volume(spec: &mut k8s_openapi::api::core::v1::PodSpec, name: &str) {
    let volumes = spec.volumes.get_or_insert_with(Vec::new);
    if !volumes.iter().any(|v| v.name == name) {
        volumes.push(Volume {
            name: name.to_string(),
            empty_dir: Some(EmptyDirVolumeSource::default()),
            ..Volume::default()
        });
    }
}

fn ensure_patcher_install(spec: &mut k8s_openapi::api::core::v1::PodSpec, image: &str) {
    let init_containers = spec.init_containers.get_or_insert_with(Vec::new);
    if init_containers.iter().any(|c| c.name == "patcher-install") {
        return;
    }

    let container = Container {
        name: "patcher-install".to_string(),
        image: Some(image.to_string()),
        command: Some(vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "cp /patcher /patch-tools/ && chmod +x /patch-tools/patcher".to_string(),
        ]),
        volume_mounts: Some(vec![VolumeMount {
            name: TOOLS_VOLUME_NAME.to_string(),
            mount_path: TOOLS_MOUNT_PATH.to_string(),
            ..VolumeMount::default()
        }]),
        ..Container::default()
    };

    // Prepend so it runs before any patch init containers.
    init_containers.insert(0, container);
}

fn apply_patch(
    spec: &mut k8s_openapi::api::core::v1::PodSpec,
    patch: &BinaryPatch,
) -> anyhow::Result<()> {
    // Validate hex lengths
    let find_bytes = decode_hex(&patch.find)?;
    let replace_bytes = decode_hex(&patch.replace)?;
    if find_bytes.len() != replace_bytes.len() {
        anyhow::bail!(
            "find ({} bytes) and replace ({} bytes) must be the same length for {}",
            find_bytes.len(),
            replace_bytes.len(),
            patch.binary_path
        );
    }

    let container_indices = containers_for_patch(spec, patch.container_name.as_deref());
    if container_indices.is_empty() {
        info!("no matching containers for patch on {}", patch.binary_path);
    }

    for i in container_indices {
        let container = &spec.containers[i];
        let slug = slug_for(&container.name, &patch.binary_path);
        let output_path = format!("{}/{}", BINS_MOUNT_PATH, slug);
        let init_name = sanitize_name(&format!("patch-{}", slug));
        let image = container.image.clone().unwrap_or_default();

        // Create the patch init container
        let init_container = Container {
            name: init_name.clone(),
            image: Some(image),
            command: Some(vec![format!("{}/patcher", TOOLS_MOUNT_PATH)]),
            args: Some(vec![
                "--input".to_string(),
                patch.binary_path.clone(),
                "--output".to_string(),
                output_path,
                "--find".to_string(),
                normalize_hex(&patch.find),
                "--replace".to_string(),
                normalize_hex(&patch.replace),
            ]),
            volume_mounts: Some(vec![
                VolumeMount {
                    name: TOOLS_VOLUME_NAME.to_string(),
                    mount_path: TOOLS_MOUNT_PATH.to_string(),
                    read_only: Some(true),
                    ..VolumeMount::default()
                },
                VolumeMount {
                    name: BINS_VOLUME_NAME.to_string(),
                    mount_path: BINS_MOUNT_PATH.to_string(),
                    ..VolumeMount::default()
                },
            ]),
            ..Container::default()
        };

        // Append init container (patcher-install was already prepended)
        let init_containers = spec.init_containers.get_or_insert_with(Vec::new);
        if !init_containers.iter().any(|c| c.name == init_name) {
            init_containers.push(init_container);
        }

        // Add volume mount to the target container so the patched binary
        // shadows the original via subPath.
        let target = &mut spec.containers[i];
        if !has_mount_at(target, &patch.binary_path) {
            let mounts = target.volume_mounts.get_or_insert_with(Vec::new);
            mounts.push(VolumeMount {
                name: BINS_VOLUME_NAME.to_string(),
                mount_path: patch.binary_path.clone(),
                sub_path: Some(slug),
                ..VolumeMount::default()
            });
        }
    }

    Ok(())
}

/// Produce a short unique slug for a (container, binaryPath) pair.
fn slug_for(container_name: &str, binary_path: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{}:{}", container_name, binary_path));
    let hash = hasher.finalize();
    let base = Path::new(binary_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("bin");
    format!("{}-{}", base, hex::encode(&hash[..4]))
}

/// Make a string safe for use as a Kubernetes resource name.
fn sanitize_name(s: &str) -> String {
    let mut result: String = s.to_lowercase().replace(['/', '_'], "-");
    if result.len() > 63 {
        result.truncate(63);
    }
    result.trim_matches('-').to_string()
}

/// Strip decorations so the patcher CLI gets a plain hex string.
fn normalize_hex(s: &str) -> String {
    s.replace([' ', ','], "")
        .replace("0x", "")
        .replace("0X", "")
        .to_uppercase()
}

fn decode_hex(s: &str) -> anyhow::Result<Vec<u8>> {
    let cleaned = normalize_hex(s);
    Ok(hex::decode(&cleaned)?)
}

/// Return indices of containers matching the given name (or all if name is None).
fn containers_for_patch(
    spec: &k8s_openapi::api::core::v1::PodSpec,
    container_name: Option<&str>,
) -> Vec<usize> {
    match container_name {
        Some(name) => spec
            .containers
            .iter()
            .enumerate()
            .filter(|(_, c)| c.name == name)
            .map(|(i, _)| i)
            .collect(),
        None => (0..spec.containers.len()).collect(),
    }
}

fn has_mount_at(container: &Container, path: &str) -> bool {
    container
        .volume_mounts
        .as_ref()
        .is_some_and(|mounts| mounts.iter().any(|m| m.mount_path == path))
}
