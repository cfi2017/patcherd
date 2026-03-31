use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Spec of a PatchRule custom resource.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct PatchRuleSpec {
    pub selector: Selector,
    pub patches: Vec<BinaryPatch>,
    #[serde(default, rename = "patcherImage")]
    pub patcher_image: Option<String>,
}

/// Label selector — supports matchLabels only (covers 99% of use-cases).
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct Selector {
    #[serde(default, rename = "matchLabels")]
    pub match_labels: Option<BTreeMap<String, String>>,
}

/// A single binary patch specification.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct BinaryPatch {
    #[serde(rename = "binaryPath")]
    pub binary_path: String,
    pub find: String,
    pub replace: String,
    #[serde(default, rename = "containerName")]
    pub container_name: Option<String>,
}

impl Selector {
    /// Returns true if the given labels satisfy this selector.
    pub fn matches(&self, labels: &BTreeMap<String, String>) -> bool {
        match &self.match_labels {
            Some(match_labels) => match_labels
                .iter()
                .all(|(k, v)| labels.get(k).is_some_and(|lv| lv == v)),
            None => true,
        }
    }
}
