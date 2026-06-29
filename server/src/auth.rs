//! JWT bearer authentication for mutating routes (`POST /api/queue`).
//!
//! HS256, validated against `AXIOMLAB_JWT_SECRET`. If no secret is configured the
//! server runs in open dev mode (a warning is logged at startup).

use axum::http::HeaderMap;
use jsonwebtoken::{DecodingKey, Validation, decode};
use serde::Deserialize;

// Fields are consumed by `jsonwebtoken` during decode/exp-validation, not read
// directly here.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Claims {
    sub: String,
    exp: usize,
}

/// Verify the `Authorization: Bearer <jwt>` header against the configured secret.
/// Returns `Ok(())` in dev mode (no secret set).
pub fn verify(headers: &HeaderMap, secret: &Option<String>) -> Result<(), String> {
    let Some(secret) = secret else {
        return Ok(()); // dev mode
    };
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or_else(|| "missing bearer token".to_string())?;

    let mut validation = Validation::default();
    validation.validate_exp = true;
    decode::<Claims>(token, &DecodingKey::from_secret(secret.as_bytes()), &validation)
        .map(|_| ())
        .map_err(|e| format!("invalid token: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};

    fn token(secret: &str, exp: usize) -> String {
        #[derive(serde::Serialize)]
        struct C {
            sub: String,
            exp: usize,
        }
        encode(&Header::default(), &C { sub: "op".into(), exp }, &EncodingKey::from_secret(secret.as_bytes())).unwrap()
    }

    #[test]
    fn dev_mode_allows() {
        assert!(verify(&HeaderMap::new(), &None).is_ok());
    }

    #[test]
    fn valid_token_accepted() {
        let mut h = HeaderMap::new();
        let exp = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 3600) as usize;
        h.insert(axum::http::header::AUTHORIZATION, format!("Bearer {}", token("s3cr3t", exp)).parse().unwrap());
        assert!(verify(&h, &Some("s3cr3t".into())).is_ok());
    }

    #[test]
    fn wrong_secret_rejected() {
        let mut h = HeaderMap::new();
        let exp = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + 3600) as usize;
        h.insert(axum::http::header::AUTHORIZATION, format!("Bearer {}", token("other", exp)).parse().unwrap());
        assert!(verify(&h, &Some("s3cr3t".into())).is_err());
    }

    #[test]
    fn missing_token_rejected_when_secret_set() {
        assert!(verify(&HeaderMap::new(), &Some("s3cr3t".into())).is_err());
    }
}
