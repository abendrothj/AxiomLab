//! HTTP JWT authentication middleware for the AxiomLab server.
//!
//! # Algorithm
//! HMAC-SHA256 (`HS256`).  The shared secret is read from
//! `AXIOMLAB_JWT_SECRET` (base64-encoded, ≥32 bytes recommended).
//!
//! When the env var is absent the middleware logs a warning and accepts all
//! requests (open / dev mode).  Set the var in production.
//!
//! # Token format
//! Standard JWT with claims:
//! ```json
//! { "sub": "operator_id", "role": "operator|pi|machine", "exp": <unix>, "iat": <unix> }
//! ```
//!
//! Generate tokens with:
//! ```
//! cargo run -p agent_runtime --bin tokengen -- --operator-id alice --role operator --ttl-hours 8
//! ```

use axum::{
    extract::Request,
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde::{Deserialize, Serialize};

// ── Claims ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    /// Subject — operator identifier (e.g. "alice", "ci-bot").
    pub sub: String,
    /// Role: one of "operator", "pi", or "machine".
    pub role: String,
    /// Issued-at (Unix seconds).
    pub iat: u64,
    /// Expiry (Unix seconds).
    pub exp: u64,
}

/// Request extension injected by `require_operator_jwt` for downstream handlers.
#[derive(Debug, Clone)]
pub struct OperatorId(pub String);

// ── Secret loading ────────────────────────────────────────────────────────────

/// Read and base64-decode `AXIOMLAB_JWT_SECRET`.
/// Returns `None` when the variable is absent — middleware falls through (dev mode).
pub fn jwt_secret_from_env() -> Option<Vec<u8>> {
    let raw = std::env::var("AXIOMLAB_JWT_SECRET").ok()?;
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    match STANDARD.decode(raw.trim()) {
        Ok(bytes) if bytes.len() >= 16 => Some(bytes),
        Ok(_short) => {
            tracing::warn!("AXIOMLAB_JWT_SECRET is too short (< 16 bytes) — treating as absent");
            None
        }
        Err(e) => {
            tracing::warn!("AXIOMLAB_JWT_SECRET is not valid base64: {e}");
            None
        }
    }
}

// ── Middleware ─────────────────────────────────────────────────────────────────

/// Axum middleware layer: verify `Authorization: Bearer <jwt>`.
///
/// * If `AXIOMLAB_JWT_SECRET` is not set → accept all (dev/open mode, warn once).
/// * If token is missing or invalid → `401 Unauthorized`.
/// * On success → injects `OperatorId` extension for downstream handlers.
pub async fn require_operator_jwt(
    headers: HeaderMap,
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let secret = match jwt_secret_from_env() {
        Some(s) => s,
        None => {
            tracing::warn!(
                "AXIOMLAB_JWT_SECRET not set — authentication disabled (dev mode). \
                 Set this variable before deploying to production."
            );
            // Inject a placeholder operator so downstream handlers don't panic.
            req.extensions_mut().insert(OperatorId("unauthenticated".into()));
            return Ok(next.run(req).await);
        }
    };

    let token = extract_bearer(&headers).ok_or_else(|| {
        tracing::debug!("Request rejected: missing Authorization header");
        StatusCode::UNAUTHORIZED
    })?;

    let key = DecodingKey::from_secret(&secret);
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;

    let claims = decode::<JwtClaims>(token, &key, &validation)
        .map_err(|e| {
            tracing::debug!("JWT verification failed: {e}");
            StatusCode::UNAUTHORIZED
        })?
        .claims;

    req.extensions_mut().insert(OperatorId(claims.sub.clone()));
    Ok(next.run(req).await)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn extract_bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
    value.strip_prefix("Bearer ")
}

/// Validate a raw JWT string against the configured secret.
///
/// Returns `Ok(claims)` on success or a descriptive error string on failure.
/// Used for non-middleware contexts (e.g., WebSocket upgrade validation).
pub fn validate_jwt(token: &str) -> Result<JwtClaims, String> {
    let secret = jwt_secret_from_env()
        .ok_or_else(|| "AXIOMLAB_JWT_SECRET not configured".to_string())?;
    let key = DecodingKey::from_secret(&secret);
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;
    decode::<JwtClaims>(token, &key, &validation)
        .map(|d| d.claims)
        .map_err(|e| e.to_string())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};

    fn make_token(secret: &[u8], sub: &str, role: &str, exp_offset_secs: i64) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let exp = if exp_offset_secs >= 0 {
            now + exp_offset_secs as u64
        } else {
            now.saturating_sub((-exp_offset_secs) as u64)
        };
        let claims = JwtClaims { sub: sub.into(), role: role.into(), iat: now, exp };
        encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(secret)).unwrap()
    }

    #[test]
    fn valid_token_decodes() {
        let secret = b"super-secret-key-for-testing-only";
        let token = make_token(secret, "alice", "operator", 3600);
        let key = DecodingKey::from_secret(secret);
        let mut v = Validation::new(Algorithm::HS256);
        v.validate_exp = true;
        let result = decode::<JwtClaims>(&token, &key, &v);
        assert!(result.is_ok());
        let claims = result.unwrap().claims;
        assert_eq!(claims.sub, "alice");
        assert_eq!(claims.role, "operator");
    }

    #[test]
    fn expired_token_rejected() {
        let secret = b"super-secret-key-for-testing-only";
        let token = make_token(secret, "bob", "pi", -3600); // expired 1h ago
        let key = DecodingKey::from_secret(secret);
        let mut v = Validation::new(Algorithm::HS256);
        v.validate_exp = true;
        let result = decode::<JwtClaims>(&token, &key, &v);
        assert!(result.is_err());
    }

    #[test]
    fn wrong_secret_rejected() {
        let token = make_token(b"correct-secret-key-long-enough-12", "carol", "operator", 3600);
        let key = DecodingKey::from_secret(b"wrong-secret-key-long-enough-123");
        let mut v = Validation::new(Algorithm::HS256);
        v.validate_exp = true;
        let result = decode::<JwtClaims>(&token, &key, &v);
        assert!(result.is_err());
    }

    #[test]
    fn extract_bearer_parses() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer my.token.here"),
        );
        assert_eq!(extract_bearer(&headers), Some("my.token.here"));
    }
}
