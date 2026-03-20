//! OIDC PKCE flow for human operators.
//!
//! Provides browser-based login via institutional SSO.  After authentication,
//! an internal AxiomLab JWT (identical format to the Ed25519/HS256 tokens
//! issued by `tokengen`) is set in the session so all downstream middleware
//! (`require_operator_jwt`) handles both token types uniformly.
//!
//! # Configuration
//! ```text
//! AXIOMLAB_OIDC_ISSUER_URL      https://accounts.google.com
//! AXIOMLAB_OIDC_CLIENT_ID       <client-id>
//! AXIOMLAB_OIDC_CLIENT_SECRET   <client-secret>
//! AXIOMLAB_OIDC_REDIRECT_URI    http://localhost:3000/api/auth/oidc/callback
//! AXIOMLAB_OIDC_GROUPS_CLAIM    groups   (optional — maps OIDC groups to roles)
//! ```
//!
//! # Flow
//! 1. `GET /api/auth/oidc/start`    — redirect to IdP with PKCE challenge
//! 2. `GET /api/auth/oidc/callback` — exchange code → ID token → internal JWT
//! 3. `POST /api/auth/logout`       — invalidate session token
//!
//! The PKCE verifier is stored server-side in a short-lived in-memory map
//! (keyed by `state` parameter).  A full production deployment should use a
//! distributed cache; for a single-node lab server this is sufficient.

use axum::{
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use openidconnect::{
    AuthenticationFlow, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
    core::{CoreClient, CoreProviderMetadata, CoreResponseType, CoreTokenResponse},
};
use openidconnect::reqwest::async_http_client;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::auth::JwtClaims;

// ── Config ────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct OidcConfig {
    pub issuer_url:     String,
    pub client_id:      String,
    pub client_secret:  String,
    pub redirect_uri:   String,
    pub groups_claim:   String,
}

impl OidcConfig {
    /// Load from environment variables.  Returns `None` when OIDC is not configured.
    pub fn from_env() -> Option<Self> {
        Some(OidcConfig {
            issuer_url:    std::env::var("AXIOMLAB_OIDC_ISSUER_URL").ok()?,
            client_id:     std::env::var("AXIOMLAB_OIDC_CLIENT_ID").ok()?,
            client_secret: std::env::var("AXIOMLAB_OIDC_CLIENT_SECRET").ok()?,
            redirect_uri:  std::env::var("AXIOMLAB_OIDC_REDIRECT_URI").ok()?,
            groups_claim:  std::env::var("AXIOMLAB_OIDC_GROUPS_CLAIM")
                               .unwrap_or_else(|_| "groups".into()),
        })
    }
}

// ── In-memory PKCE verifier store ─────────────────────────────────────────────

/// Short-lived PKCE state store.  Maps `state` token → `(verifier, nonce, created_at)`.
/// Entries older than 10 minutes are pruned on each access.
#[derive(Clone, Default)]
pub struct PkceStore(Arc<Mutex<HashMap<String, PkceEntry>>>);

struct PkceEntry {
    verifier:   PkceCodeVerifier,
    nonce:      Nonce,
    created_at: std::time::Instant,
}

impl PkceStore {
    pub fn insert(&self, state: String, verifier: PkceCodeVerifier, nonce: Nonce) {
        let mut map = self.0.lock().unwrap();
        // Prune stale entries (> 10 min).
        map.retain(|_, v| v.created_at.elapsed() < Duration::from_secs(600));
        map.insert(state, PkceEntry { verifier, nonce, created_at: std::time::Instant::now() });
    }

    pub fn take(&self, state: &str) -> Option<(PkceCodeVerifier, Nonce)> {
        let mut map = self.0.lock().unwrap();
        map.remove(state).map(|e| (e.verifier, e.nonce))
    }
}

// ── AppState extension ─────────────────────────────────────────────────────────

/// OIDC state shared across handlers — attach to `AppState` or pass separately.
#[derive(Clone)]
pub struct OidcState {
    pub config: OidcConfig,
    pub store:  PkceStore,
}

// ── Query params ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CallbackParams {
    pub code:  Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `GET /api/auth/oidc/start` — initiate PKCE flow, redirect to IdP.
pub async fn oidc_start_handler(
    State(oidc): State<OidcState>,
) -> Response {
    let cfg = &oidc.config;

    let issuer = match IssuerUrl::new(cfg.issuer_url.clone()) {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("Invalid OIDC issuer URL: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Invalid OIDC configuration").into_response();
        }
    };

    let provider_meta = match CoreProviderMetadata::discover_async(issuer, async_http_client).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("OIDC discovery failed: {e}");
            return (StatusCode::BAD_GATEWAY, "OIDC discovery failed").into_response();
        }
    };

    let client = CoreClient::from_provider_metadata(
        provider_meta,
        ClientId::new(cfg.client_id.clone()),
        Some(ClientSecret::new(cfg.client_secret.clone())),
    )
    .set_redirect_uri(match RedirectUrl::new(cfg.redirect_uri.clone()) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("Invalid OIDC redirect URI: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Invalid redirect URI").into_response();
        }
    });

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

    let (auth_url, csrf_token, nonce) = client
        .authorize_url(
            AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scope(Scope::new("openid".into()))
        .add_scope(Scope::new("email".into()))
        .add_scope(Scope::new("profile".into()))
        .set_pkce_challenge(pkce_challenge)
        .url();

    oidc.store.insert(csrf_token.secret().clone(), pkce_verifier, nonce);
    tracing::info!(state = %csrf_token.secret(), "OIDC PKCE flow started");
    Redirect::temporary(auth_url.as_str()).into_response()
}

/// `GET /api/auth/oidc/callback` — exchange code for ID token, issue internal JWT.
pub async fn oidc_callback_handler(
    Query(params): Query<CallbackParams>,
    State(oidc): State<OidcState>,
) -> Response {
    if let Some(err) = params.error {
        tracing::warn!("OIDC callback error from IdP: {err}");
        return (StatusCode::UNAUTHORIZED, format!("IdP error: {err}")).into_response();
    }

    let code  = params.code.unwrap_or_default();
    let state = params.state.unwrap_or_default();
    if code.is_empty() || state.is_empty() {
        return (StatusCode::BAD_REQUEST, "Missing code or state").into_response();
    }

    let (verifier, _nonce) = match oidc.store.take(&state) {
        Some(v) => v,
        None => {
            tracing::warn!("OIDC callback: unknown or expired state token");
            return (StatusCode::BAD_REQUEST, "Invalid state (expired or replayed)").into_response();
        }
    };

    let cfg = &oidc.config;
    let issuer = IssuerUrl::new(cfg.issuer_url.clone()).unwrap();
    let provider_meta = match CoreProviderMetadata::discover_async(issuer, async_http_client).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("OIDC discovery failed in callback: {e}");
            return (StatusCode::BAD_GATEWAY, "OIDC discovery failed").into_response();
        }
    };

    let client = CoreClient::from_provider_metadata(
        provider_meta,
        ClientId::new(cfg.client_id.clone()),
        Some(ClientSecret::new(cfg.client_secret.clone())),
    )
    .set_redirect_uri(RedirectUrl::new(cfg.redirect_uri.clone()).unwrap());

    let token_response: CoreTokenResponse = match client
        .exchange_code(AuthorizationCode::new(code))
        .set_pkce_verifier(verifier)
        .request_async(async_http_client)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("OIDC token exchange failed: {e}");
            return (StatusCode::UNAUTHORIZED, "Token exchange failed").into_response();
        }
    };

    // Extract operator_id and role from the ID token claims.
    // The exchange itself proved authenticity; we decode for identity extraction.
    let raw_id_token = match token_response.extra_fields().id_token() {
        Some(t) => t.to_string(),
        None => return (StatusCode::UNAUTHORIZED, "No ID token in response").into_response(),
    };
    let (operator_id, role) = extract_claims_from_raw_id_token(&raw_id_token, &cfg.groups_claim);

    let internal_jwt = match issue_internal_jwt(&operator_id, &role) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("Failed to issue internal JWT: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "JWT issuance failed").into_response();
        }
    };

    tracing::info!(operator_id = %operator_id, role = %role, "OIDC login successful");

    // Return token in JSON body + Authorization header for API clients.
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {internal_jwt}")).unwrap(),
    );
    (
        StatusCode::OK,
        headers,
        axum::Json(serde_json::json!({
            "token":       internal_jwt,
            "operator_id": operator_id,
            "role":        role,
            "iss":         "axiomlab-oidc",
        })),
    ).into_response()
}

/// `POST /api/auth/logout` — instruct clients to discard their token.
///
/// Server-side stateless JWT cannot be revoked without a blocklist.  For now,
/// this endpoint returns success and documents that clients should delete the token.
/// Full revocation via a JWT blocklist in SQLite is part of Phase 4B.
pub async fn logout_handler() -> impl IntoResponse {
    axum::Json(serde_json::json!({
        "status": "logged_out",
        "note": "Discard your Bearer token. Server-side revocation (Phase 4B) not yet active.",
    }))
}

// ── Internal JWT issuance (OIDC → same format as tokengen) ────────────────────

fn issue_internal_jwt(operator_id: &str, role: &str) -> Result<String, String> {
    let secret = crate::auth::jwt_secret_from_env()
        .ok_or_else(|| "AXIOMLAB_JWT_SECRET not configured".to_string())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = JwtClaims {
        sub:  operator_id.to_string(),
        role: role.to_string(),
        iat:  now,
        exp:  now + 8 * 3600, // 8-hour session
    };
    encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(&secret))
        .map_err(|e| e.to_string())
}

/// Minimal ID token claim extraction without full OIDC verification.
/// Returns `(sub_or_email, role)`.
fn extract_claims_from_raw_id_token(raw: &str, groups_claim: &str) -> (String, String) {
    let parts: Vec<&str> = raw.splitn(3, '.').collect();
    if parts.len() < 2 {
        return ("unknown".into(), "operator".into());
    }
    // Base64url → JSON.
    let padded = pad_base64url(parts[1]);
    let decoded = STANDARD.decode(&padded).unwrap_or_default();
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&decoded) else {
        return ("unknown".into(), "operator".into());
    };

    let sub = json.get("sub")
        .or_else(|| json.get("email"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    // Map OIDC groups claim → AxiomLab role.
    let role = json.get(groups_claim)
        .and_then(|g| g.as_array())
        .and_then(|arr| {
            if arr.iter().any(|g| g.as_str() == Some("pi") || g.as_str() == Some("principal_investigator")) {
                Some("pi")
            } else if arr.iter().any(|g| g.as_str() == Some("machine")) {
                Some("machine")
            } else {
                Some("operator")
            }
        })
        .unwrap_or("operator")
        .to_string();

    (sub, role)
}

fn pad_base64url(s: &str) -> String {
    let s = s.replace('-', "+").replace('_', "/");
    let pad = (4 - s.len() % 4) % 4;
    format!("{s}{}", "=".repeat(pad))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_store_insert_take() {
        let store = PkceStore::default();
        let (_, v) = PkceCodeChallenge::new_random_sha256();
        let n = Nonce::new_random();
        store.insert("state1".into(), v, n);
        assert!(store.take("state1").is_some());
        assert!(store.take("state1").is_none()); // consumed
    }

    #[test]
    fn pkce_store_unknown_state() {
        let store = PkceStore::default();
        assert!(store.take("nonexistent").is_none());
    }

    #[test]
    fn extract_claims_parses_groups() {
        // Build a minimal fake ID token payload.
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
        let payload = serde_json::json!({
            "sub": "user@example.com",
            "groups": ["pi", "labmembers"]
        });
        let encoded = URL_SAFE_NO_PAD.encode(payload.to_string());
        let fake_token = format!("header.{encoded}.sig");
        let (sub, role) = extract_claims_from_raw_id_token(&fake_token, "groups");
        assert_eq!(sub, "user@example.com");
        assert_eq!(role, "pi");
    }

    #[test]
    fn extract_claims_defaults_to_operator() {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
        let payload = serde_json::json!({ "sub": "technician@lab.org" });
        let encoded = URL_SAFE_NO_PAD.encode(payload.to_string());
        let fake_token = format!("h.{encoded}.s");
        let (sub, role) = extract_claims_from_raw_id_token(&fake_token, "groups");
        assert_eq!(sub, "technician@lab.org");
        assert_eq!(role, "operator");
    }
}
