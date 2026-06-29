//! Server-side sessions, generic OIDC code flow, roles, and CSRF enforcement.
use crate::state::AppState;
use axum::http::{HeaderMap, header};
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use reqwest::Client;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Mutex};

const COOKIE: &str = "axiomlab_session";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Viewer,
    Operator,
    Approver,
    Admin,
    Service,
}
impl Role {
    fn parse(v: &str) -> Option<Self> {
        match v {
            "viewer" => Some(Self::Viewer),
            "operator" => Some(Self::Operator),
            "approver" => Some(Self::Approver),
            "admin" => Some(Self::Admin),
            "service" => Some(Self::Service),
            _ => None,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Operator => "operator",
            Self::Approver => "approver",
            Self::Admin => "admin",
            Self::Service => "service",
        }
    }
    pub fn permits(self, required: Self) -> bool {
        self == Self::Admin
            || self == required
            || matches!(
                (self, required),
                (Self::Operator, Self::Viewer)
                    | (Self::Approver, Self::Viewer)
                    | (Self::Service, Self::Viewer)
            )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Principal {
    pub subject: String,
    pub role: Role,
    pub session_id: String,
    pub csrf_token: String,
}

#[derive(Clone)]
pub struct OidcConfig {
    authorization_endpoint: String,
    token_endpoint: String,
    userinfo_endpoint: String,
    client_id: String,
    client_secret: String,
    redirect_uri: String,
}
impl OidcConfig {
    fn from_env() -> Option<Self> {
        Some(Self {
            authorization_endpoint: std::env::var("AXIOMLAB_OIDC_AUTHORIZATION_ENDPOINT").ok()?,
            token_endpoint: std::env::var("AXIOMLAB_OIDC_TOKEN_ENDPOINT").ok()?,
            userinfo_endpoint: std::env::var("AXIOMLAB_OIDC_USERINFO_ENDPOINT").ok()?,
            client_id: std::env::var("AXIOMLAB_OIDC_CLIENT_ID").ok()?,
            client_secret: std::env::var("AXIOMLAB_OIDC_CLIENT_SECRET").ok()?,
            redirect_uri: std::env::var("AXIOMLAB_OIDC_REDIRECT_URI").ok()?,
        })
    }
}

pub struct AuthStore {
    connection: Mutex<Connection>,
    oidc: Option<OidcConfig>,
    pub dev_mode: bool,
    client: Client,
}
impl AuthStore {
    pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        let connection = Connection::open(path)?;
        connection.execute_batch(include_str!("../migrations/0001_operational_state.sql"))?;
        Ok(Self {
            connection: Mutex::new(connection),
            oidc: OidcConfig::from_env(),
            dev_mode: std::env::var("AXIOMLAB_DEV_AUTH").as_deref() == Ok("1"),
            client: Client::new(),
        })
    }
    pub fn principal(&self, headers: &HeaderMap) -> Result<Principal, String> {
        let id = cookie(headers, COOKIE).ok_or("authentication required")?;
        self.connection.lock().unwrap().query_row("SELECT subject,role,csrf_token FROM sessions WHERE id=?1 AND revoked_secs IS NULL AND expires_secs>?2",params![id,now()],|r|{let role:String=r.get(1)?;Ok(Principal{subject:r.get(0)?,role:Role::parse(&role).unwrap_or(Role::Viewer),session_id:id.clone(),csrf_token:r.get(2)?})}).optional().map_err(|e|e.to_string())?.ok_or_else(||"session expired or revoked".into())
    }
    pub fn authorize(
        &self,
        headers: &HeaderMap,
        role: Role,
        csrf: bool,
    ) -> Result<Principal, String> {
        let p = self.principal(headers)?;
        if !p.role.permits(role) {
            return Err("insufficient role".into());
        }
        if csrf && headers.get("x-csrf-token").and_then(|v| v.to_str().ok()) != Some(&p.csrf_token)
        {
            return Err("invalid CSRF token".into());
        }
        Ok(p)
    }
    pub fn create_session(
        &self,
        subject: &str,
        role: Role,
    ) -> rusqlite::Result<(Principal, String)> {
        let id = random();
        let csrf = random();
        let n = now();
        self.connection.lock().unwrap().execute("INSERT INTO sessions(id,subject,role,csrf_token,created_secs,expires_secs) VALUES(?1,?2,?3,?4,?5,?6)",params![id,subject,role.as_str(),csrf,n,n+28800])?;
        let secure = self
            .oidc
            .as_ref()
            .is_some_and(|config| config.redirect_uri.starts_with("https://"));
        Ok((
            Principal {
                subject: subject.into(),
                role,
                session_id: id.clone(),
                csrf_token: csrf,
            },
            session_cookie(&id, secure),
        ))
    }
    pub fn revoke(&self, headers: &HeaderMap) {
        if let Some(id) = cookie(headers, COOKIE) {
            let _ = self.connection.lock().unwrap().execute(
                "UPDATE sessions SET revoked_secs=?2 WHERE id=?1",
                params![id, now()],
            );
        }
    }
    pub fn begin_oidc(&self, return_to: &str) -> Result<String, String> {
        if !return_to.starts_with('/') || return_to.starts_with("//") {
            return Err("return_to must be a local path".into());
        }
        let c = self.oidc.as_ref().ok_or("OIDC is not configured")?;
        let state = random();
        let nonce = random();
        self.connection
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO oidc_states VALUES(?1,?2,?3,?4)",
                params![state, nonce, now(), return_to],
            )
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "{}?response_type=code&client_id={}&redirect_uri={}&scope=openid%20profile%20email&state={}&nonce={}",
            c.authorization_endpoint,
            enc(&c.client_id),
            enc(&c.redirect_uri),
            enc(&state),
            enc(&nonce)
        ))
    }
    pub async fn finish_oidc(
        &self,
        code: &str,
        state: &str,
    ) -> Result<(Principal, String, String), String> {
        let c = self.oidc.as_ref().ok_or("OIDC is not configured")?;
        let return_to: String = self
            .connection
            .lock()
            .unwrap()
            .query_row(
                "DELETE FROM oidc_states WHERE state=?1 AND created_secs>?2 RETURNING return_to",
                params![state, now() - 600],
                |r| r.get(0),
            )
            .map_err(|_| "invalid or expired OIDC state")?;
        let token: serde_json::Value = self
            .client
            .post(&c.token_endpoint)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", &c.redirect_uri),
                ("client_id", &c.client_id),
                ("client_secret", &c.client_secret),
            ])
            .send()
            .await
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        let access = token["access_token"]
            .as_str()
            .ok_or("missing access_token")?;
        let user: serde_json::Value = self
            .client
            .get(&c.userinfo_endpoint)
            .bearer_auth(access)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        let subject = user["sub"].as_str().ok_or("userinfo omitted sub")?;
        let role = user["axiomlab_role"]
            .as_str()
            .and_then(Role::parse)
            .unwrap_or(Role::Viewer);
        let (p, cookie) = self
            .create_session(subject, role)
            .map_err(|e| e.to_string())?;
        Ok((p, cookie, return_to))
    }
}
fn cookie(h: &HeaderMap, name: &str) -> Option<String> {
    h.get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|p| {
            let (k, v) = p.trim().split_once('=')?;
            (k == name).then(|| v.into())
        })
}
fn session_cookie(id: &str, secure: bool) -> String {
    format!(
        "{COOKIE}={id}; Path=/; HttpOnly; SameSite=Lax; Max-Age=28800{}",
        if secure { "; Secure" } else { "" }
    )
}
pub fn clear_cookie() -> String {
    format!("{COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
}
fn random() -> String {
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}
fn enc(v: &str) -> String {
    urlencoding::encode(v).into_owned()
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn store() -> AuthStore {
        let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
        AuthStore::open(dir.path().join("auth.db")).unwrap()
    }
    fn headers(cookie: &str, csrf: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            cookie.split(';').next().unwrap().parse().unwrap(),
        );
        if let Some(value) = csrf {
            h.insert("x-csrf-token", value.parse().unwrap());
        }
        h
    }
    #[test]
    fn session_roles_and_csrf_are_enforced() {
        let s = store();
        let (p, c) = s.create_session("alice", Role::Operator).unwrap();
        assert!(
            s.authorize(&headers(&c, Some(&p.csrf_token)), Role::Operator, true)
                .is_ok()
        );
        assert!(
            s.authorize(&headers(&c, None), Role::Operator, true)
                .is_err()
        );
        assert!(
            s.authorize(&headers(&c, Some(&p.csrf_token)), Role::Approver, true)
                .is_err()
        );
    }
    #[test]
    fn revoked_session_stops_working() {
        let s = store();
        let (_, c) = s.create_session("alice", Role::Viewer).unwrap();
        let h = headers(&c, None);
        assert!(s.principal(&h).is_ok());
        s.revoke(&h);
        assert!(s.principal(&h).is_err());
    }
}

pub async fn require_session(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    match state.auth.principal(request.headers()) {
        Ok(_) => next.run(request).await,
        Err(error) => (axum::http::StatusCode::UNAUTHORIZED, error).into_response(),
    }
}
