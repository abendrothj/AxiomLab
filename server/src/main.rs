//! AxiomLab server — Axum HTTP + WebSocket front end over the gate pipeline.
//!
//! The audit chain is the system of record; this process exposes it (plus the
//! directive queue, approvals, and lab state) and drives runs through a
//! background [`worker`].

mod auth;
mod handlers;
mod queue;
mod state;
mod worker;

use axiom_audit::{Chain, RevocationList, Signer, signer_from_env};
use axiom_gate::{ApprovalQueue, CapabilityPolicy};
use axiom_proofs::ProofChecker;
use axiom_sila::SilaClients;
use axiom_types::LabState;
use axum::{
    Router,
    routing::{delete, get, post},
};
use metrics_exporter_prometheus::PrometheusBuilder;
use queue::ProtocolQueue;
use state::AppState;
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::{Arc, Mutex};
use tower_http::cors::CorsLayer;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "axiomlab_server=info,axiom_gate=info,axiom_llm=info".into()),
        )
        .init();

    let prometheus = PrometheusBuilder::new().install_recorder().expect("install prometheus recorder");

    // ── Signing ──
    let signer: Arc<dyn Signer> = match signer_from_env() {
        Ok(s) => Arc::from(s),
        Err(e) => {
            tracing::warn!(error = %e, "falling back to an ephemeral signing key");
            Arc::new(axiom_audit::LocalSigner::generate())
        }
    };

    // ── Proof manifest (fail-closed: an unloadable manifest rejects every action) ──
    let manifest_path = std::env::var("AXIOMLAB_PROOF_MANIFEST")
        .unwrap_or_else(|_| ".artifacts/proof/manifest.signed.json".into());
    let proofs = match ProofChecker::load_and_verify(&manifest_path) {
        Ok(c) => {
            tracing::info!(path = %manifest_path, "Proof manifest loaded and verified");
            Arc::new(c)
        }
        Err(e) => {
            tracing::error!(error = %e, "proof manifest unavailable — all gated actions will be rejected");
            Arc::new(ProofChecker::from_manifest_trusted(empty_manifest()))
        }
    };

    let jwt_secret = std::env::var("AXIOMLAB_JWT_SECRET").ok();
    if jwt_secret.is_none() {
        tracing::warn!("AXIOMLAB_JWT_SECRET not set — POST /api/queue runs in open dev mode");
    }

    let chain_path = std::env::var("AXIOMLAB_AUDIT_LOG")
        .unwrap_or_else(|_| ".artifacts/audit/runtime_audit.jsonl".into());

    let (tx, _rx) = tokio::sync::broadcast::channel(1024);
    let state = AppState {
        running: Arc::new(AtomicBool::new(false)),
        iteration: Arc::new(AtomicU32::new(0)),
        audit_chain: Arc::new(Chain::open(chain_path)),
        lab_state: Arc::new(Mutex::new({
            let mut lab = LabState::load();
            lab.seed_default_vessels(); // capacity registry for the ProofGate
            lab
        })),
        approval_queue: Arc::new(ApprovalQueue::new()),
        protocol_queue: Arc::new(ProtocolQueue::open(
            std::env::var("AXIOMLAB_QUEUE_PATH").unwrap_or_else(|_| ".artifacts/runtime/queue.json".into()),
        ).expect("open protocol queue")),
        tx,
        signer,
        clients: Arc::new(SilaClients::from_env()),
        proofs,
        capability: Arc::new(CapabilityPolicy::default_lab()),
        revocations: Arc::new(RevocationList::from_env()),
        jwt_secret: Arc::new(jwt_secret),
    };

    tokio::spawn(worker::run(state.clone()));

    let render = move || {
        let h = prometheus.clone();
        async move { h.render() }
    };

    let mut app = api_router(state)
        .route("/metrics", get(render))
        .layer(CorsLayer::permissive());

    // Serve the built UI if present.
    if std::path::Path::new("ui/dist").is_dir() {
        app = app.fallback_service(tower_http::services::ServeDir::new("ui/dist"));
    }

    let bind = std::env::var("AXIOMLAB_BIND").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let listener = tokio::net::TcpListener::bind(&bind).await.expect("bind");
    tracing::info!(%bind, "AxiomLab server listening");
    axum::serve(listener, app).await.expect("serve");
}

/// All routes except `/metrics` and static file serving, with `state` attached.
/// Factored out so tests can exercise the real router.
fn api_router(state: AppState) -> Router {
    Router::new()
        .route("/api/status", get(handlers::status))
        .route("/api/audit", get(handlers::audit))
        .route("/api/audit/verify", post(handlers::audit_verify))
        .route("/api/agenda", get(handlers::agenda))
        .route("/api/queue", get(handlers::queue_list).post(handlers::queue_push))
        .route("/api/queue/:id", delete(handlers::queue_cancel))
        .route("/api/approvals", get(handlers::approvals_list))
        .route("/api/approvals/:id", post(handlers::approvals_resolve))
        .route("/api/lab", get(handlers::lab))
        .route("/ready", get(handlers::ready))
        .route("/ws", get(handlers::ws))
        .with_state(state)
}

fn empty_manifest() -> axiom_proofs::ProofManifest {
    axiom_proofs::ProofManifest {
        schema_version: 1,
        generated_unix_secs: 0,
        build: axiom_proofs::BuildIdentity {
            git_commit: String::new(),
            binary_hash: String::new(),
            workspace_hash: String::new(),
            container_image_digest: None,
            device_id: None,
            firmware_version: None,
        },
        artifacts: vec![],
        actions: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn test_state() -> AppState {
        let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
        let (tx, _rx) = tokio::sync::broadcast::channel(16);
        AppState {
            running: Arc::new(AtomicBool::new(false)),
            iteration: Arc::new(AtomicU32::new(0)),
            audit_chain: Arc::new(Chain::open(dir.path().join("a.jsonl"))),
            lab_state: Arc::new(Mutex::new(LabState::default())),
            approval_queue: Arc::new(ApprovalQueue::new()),
            protocol_queue: Arc::new(ProtocolQueue::new()),
            tx,
            signer: Arc::new(axiom_audit::LocalSigner::generate()),
            clients: Arc::new(SilaClients::simulator()),
            proofs: Arc::new(ProofChecker::from_manifest_trusted(empty_manifest())),
            capability: Arc::new(CapabilityPolicy::default_lab()),
            revocations: Arc::new(RevocationList::new()),
            jwt_secret: Arc::new(Some("s3cr3t".into())),
        }
    }

    async fn body_string(resp: axum::response::Response) -> String {
        String::from_utf8(resp.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap()
    }

    #[tokio::test]
    async fn ready_returns_ok() {
        let app = api_router(test_state());
        let resp = app.oneshot(Request::get("/ready").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_string(resp).await, "ok");
    }

    #[tokio::test]
    async fn status_reports_simulator() {
        let app = api_router(test_state());
        let resp = app.oneshot(Request::get("/api/status").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(body_string(resp).await.contains("simulator"));
    }

    #[tokio::test]
    async fn queue_push_requires_auth() {
        let app = api_router(test_state());
        let req = Request::post("/api/queue")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"directive":"x"}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn queue_push_accepts_with_token_then_lists() {
        let state = test_state();
        let exp = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 3600) as usize;
        let token = make_token("s3cr3t", exp);
        let app = api_router(state.clone());
        let req = Request::post("/api/queue")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(r#"{"directive":"calibrate"}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        assert_eq!(state.protocol_queue.list().len(), 1);
    }

    #[tokio::test]
    async fn audit_verify_ok_on_empty_chain() {
        let app = api_router(test_state());
        let resp = app.oneshot(Request::post("/api/audit/verify").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(body_string(resp).await.contains("\"ok\":true"));
    }

    fn make_token(secret: &str, exp: usize) -> String {
        use jsonwebtoken::{EncodingKey, Header, encode};
        #[derive(serde::Serialize)]
        struct C {
            sub: String,
            exp: usize,
        }
        encode(&Header::default(), &C { sub: "op".into(), exp }, &EncodingKey::from_secret(secret.as_bytes())).unwrap()
    }
}
