use std::convert::Infallible;
use std::net::SocketAddr;

use clap::Parser;
use k8s_openapi::api::core::v1::Pod;
use kube::core::admission::{AdmissionRequest, AdmissionResponse, AdmissionReview};
use patcherd::mutator;
use serde_json::json;
use tracing::{error, info};
use warp::Filter;

#[derive(Parser)]
#[command(about = "Kubernetes admission webhook for binary patching")]
struct Cli {
    /// Port for the webhook HTTPS server.
    #[arg(long, default_value_t = 9443)]
    webhook_port: u16,

    /// Directory containing tls.crt and tls.key.
    #[arg(long, default_value = "/tmp/k8s-webhook-server/serving-certs")]
    cert_dir: String,

    /// Port for the health/readiness HTTP server.
    #[arg(long, default_value_t = 8081)]
    health_port: u16,
}

async fn handle_mutate(
    body: AdmissionReview<Pod>,
    client: kube::Client,
) -> Result<warp::reply::Json, Infallible> {
    let req: AdmissionRequest<Pod> = match body.try_into() {
        Ok(r) => r,
        Err(e) => {
            error!("bad admission request: {}", e);
            let resp = json!({
                "apiVersion": "admission.k8s.io/v1",
                "kind": "AdmissionReview",
                "response": {
                    "uid": "",
                    "allowed": false,
                    "status": { "message": format!("bad request: {}", e) }
                }
            });
            return Ok(warp::reply::json(&resp));
        }
    };

    let resp: AdmissionResponse = mutate_pod(&req, &client).await;
    Ok(warp::reply::json(&resp.into_review()))
}

async fn mutate_pod(req: &AdmissionRequest<Pod>, client: &kube::Client) -> AdmissionResponse {
    let response = AdmissionResponse::from(req);

    let pod = match &req.object {
        Some(pod) => pod,
        None => return response.deny("no pod object in request"),
    };

    // Already injected — skip.
    if pod
        .metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get("patcher.k8s.io/injected"))
        .is_some_and(|v| v == "true")
    {
        info!("pod already injected, skipping");
        return response;
    }

    let namespace = req
        .namespace
        .as_deref()
        .or(pod.metadata.namespace.as_deref())
        .unwrap_or("default");

    let labels = pod.metadata.labels.clone().unwrap_or_default();

    let rules = match mutator::matching_rules(client, namespace, &labels).await {
        Ok(r) => r,
        Err(e) => {
            error!("failed to list PatchRules: {}", e);
            return response; // allow pod to proceed
        }
    };

    if rules.is_empty() {
        info!("no matching PatchRules");
        return response;
    }

    info!("matched {} PatchRule(s), injecting", rules.len());

    let mut patched_pod = pod.clone();
    if let Err(e) = mutator::inject(&mut patched_pod, &rules) {
        error!("injection failed: {}", e);
        return response;
    }

    // Compute JSON patch by diffing original and mutated pods.
    let original = serde_json::to_value(pod).unwrap();
    let modified = serde_json::to_value(&patched_pod).unwrap();
    let patch = json_patch::diff(&original, &modified);

    match response.with_patch(patch) {
        Ok(r) => r,
        Err(e) => {
            error!("failed to build patch response: {}", e);
            AdmissionResponse::from(req)
        }
    }
}

async fn health() -> Result<impl warp::Reply, Infallible> {
    Ok("ok")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    let client = kube::Client::try_default().await?;
    info!("connected to Kubernetes API");

    // --- health / readiness (plain HTTP) ---
    let health_routes = warp::get()
        .and(warp::path("healthz").or(warp::path("readyz")).unify())
        .and_then(health);

    let health_addr: SocketAddr = ([0, 0, 0, 0], cli.health_port).into();
    info!("health server listening on {}", health_addr);
    tokio::spawn(warp::serve(health_routes).run(health_addr));

    // --- webhook (TLS) ---
    let client_filter = warp::any().map(move || client.clone());

    let mutate_route = warp::post()
        .and(warp::path("mutate-v1-pod"))
        .and(warp::body::json())
        .and(client_filter)
        .and_then(handle_mutate);

    let cert_path = format!("{}/tls.crt", cli.cert_dir);
    let key_path = format!("{}/tls.key", cli.cert_dir);
    let webhook_addr: SocketAddr = ([0, 0, 0, 0], cli.webhook_port).into();

    info!("webhook server listening on {} (TLS)", webhook_addr);
    warp::serve(mutate_route)
        .tls()
        .cert_path(&cert_path)
        .key_path(&key_path)
        .run(webhook_addr)
        .await;

    Ok(())
}
