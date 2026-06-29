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
    Router, middleware,
    routing::{delete, get, post},
};
use metrics_exporter_prometheus::PrometheusBuilder;
use queue::ProtocolQueue;
use state::AppState;
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::{Arc, Mutex};

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

    let database_path = std::env::var("AXIOMLAB_DATABASE_PATH").unwrap_or_else(|_| ".artifacts/runtime/axiomlab.db".into());

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
        approval_queue: Arc::new(ApprovalQueue::open_sqlite(&database_path).expect("open approval journal")),
        protocol_queue: Arc::new(ProtocolQueue::open(
            &database_path,
        ).expect("open protocol queue")),
        tx,
        signer,
        clients: Arc::new(SilaClients::from_env()),
        proofs,
        capability: Arc::new(CapabilityPolicy::default_lab()),
        revocations: Arc::new(RevocationList::from_env()),
        auth: Arc::new(auth::AuthStore::open(&database_path).expect("open auth store")),
        allow_self_approval: std::env::var("AXIOMLAB_ALLOW_SELF_APPROVAL").as_deref()==Ok("1"),
    };

    tokio::spawn(worker::run(state.clone()));

    let render = move || {
        let h = prometheus.clone();
        async move { h.render() }
    };

    let metrics = Router::new()
        .route("/metrics", get(render))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_session))
        .with_state(state.clone());
    let mut app = api_router(state).merge(metrics);

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
    let protected = Router::new()
        .route("/api/status", get(handlers::status))
        .route("/api/auth/me", get(handlers::auth_me))
        .route("/api/auth/logout", post(handlers::auth_logout))
        .route("/api/audit", get(handlers::audit))
        .route("/api/audit/verify", post(handlers::audit_verify))
        .route("/api/agenda", get(handlers::agenda))
        .route("/api/queue", get(handlers::queue_list).post(handlers::queue_push))
        .route("/api/queue/:id", delete(handlers::queue_cancel))
        .route("/api/queue/:id/reconcile", post(handlers::queue_reconcile))
        .route("/api/approvals", get(handlers::approvals_list))
        .route("/api/approvals/history", get(handlers::approvals_history))
        .route("/api/approvals/:id", post(handlers::approvals_resolve))
        .route("/api/lab", get(handlers::lab))
        .route("/ws", get(handlers::ws))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_session));
    Router::new()
        .route("/api/auth/login", get(handlers::auth_login))
        .route("/api/auth/callback", get(handlers::auth_callback))
        .route("/api/auth/dev-login", post(handlers::auth_dev_login))
        .route("/ready", get(handlers::ready))
        .merge(protected)
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
            auth: Arc::new(auth::AuthStore::open(dir.path().join("state.db")).unwrap()),
            allow_self_approval: false,
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
        let state=test_state(); let (_,cookie)=state.auth.create_session("viewer",auth::Role::Viewer).unwrap();
        let app = api_router(state);
        let resp = app.oneshot(Request::get("/api/status").header("cookie",cookie.split(';').next().unwrap()).body(Body::empty()).unwrap()).await.unwrap();
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
    async fn queue_push_accepts_with_session_then_lists() {
        let state = test_state();
        let (principal, cookie) = state.auth.create_session("op", auth::Role::Operator).unwrap();
        let app = api_router(state.clone());
        let req = Request::post("/api/queue")
            .header("content-type", "application/json")
            .header("cookie", cookie.split(';').next().unwrap())
            .header("x-csrf-token", principal.csrf_token)
            .body(Body::from(r#"{"directive":"calibrate"}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        assert_eq!(state.protocol_queue.list().len(), 1);
    }

    #[tokio::test]
    async fn audit_verify_ok_on_empty_chain() {
        let state=test_state(); let (p,cookie)=state.auth.create_session("admin",auth::Role::Admin).unwrap(); let app=api_router(state);
        let resp = app.oneshot(Request::post("/api/audit/verify").header("cookie",cookie.split(';').next().unwrap()).header("x-csrf-token",p.csrf_token).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(body_string(resp).await.contains("\"ok\":true"));
    }

    #[tokio::test]
    async fn approval_history_is_exposed() {
        let state = test_state();
        state.approval_queue.request("move_arm", &serde_json::json!({"x": 1.0}));
        let (_,cookie)=state.auth.create_session("viewer",auth::Role::Viewer).unwrap();
        let app = api_router(state);
        let resp = app.oneshot(Request::get("/api/approvals/history").header("cookie",cookie.split(';').next().unwrap()).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(body_string(resp).await.contains("\"status\":\"pending\""));
    }

    #[tokio::test]
    async fn submitter_cannot_approve_own_run() {
        let state=test_state();let run_id=state.protocol_queue.push_for("move","alice");
        let (approval_id,_rx)=state.approval_queue.request_with_metadata_for_run("move_arm",&serde_json::json!({"x":1}),Some(axiom_types::RiskClass::Actuation),"ApprovalGate","review",std::time::Duration::from_secs(60),Some(run_id));
        let (principal,cookie)=state.auth.create_session("alice",auth::Role::Approver).unwrap();let app=api_router(state);
        let response=app.oneshot(Request::post(format!("/api/approvals/{approval_id}")).header("content-type","application/json").header("cookie",cookie.split(';').next().unwrap()).header("x-csrf-token",principal.csrf_token).body(Body::from(r#"{"approved":true,"notes":"ok"}"#)).unwrap()).await.unwrap();
        assert_eq!(response.status(),StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn development_can_explicitly_allow_self_approval() {
        let mut state=test_state();state.allow_self_approval=true;let run_id=state.protocol_queue.push_for("move","alice");
        let (approval_id,_rx)=state.approval_queue.request_with_metadata_for_run("move_arm",&serde_json::json!({"x":1}),Some(axiom_types::RiskClass::Actuation),"ApprovalGate","review",std::time::Duration::from_secs(60),Some(run_id));
        let (principal,cookie)=state.auth.create_session("alice",auth::Role::Approver).unwrap();let app=api_router(state);
        let response=app.oneshot(Request::post(format!("/api/approvals/{approval_id}")).header("content-type","application/json").header("cookie",cookie.split(';').next().unwrap()).header("x-csrf-token",principal.csrf_token).body(Body::from(r#"{"approved":true,"notes":"isolated development"}"#)).unwrap()).await.unwrap();
        assert_eq!(response.status(),StatusCode::OK);
    }

}
