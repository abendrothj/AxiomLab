//! The LLM client abstraction and an OpenAI-compatible HTTP implementation.

use crate::proposal::{self, ParseError, Proposal};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("http: {0}")]
    Http(String),
    #[error("llm endpoint returned HTTP {0}: {1}")]
    Status(u16, String),
    #[error("empty response")]
    Empty,
    #[error("could not parse proposal: {0}")]
    Parse(#[from] ParseError),
    #[error("scripted client exhausted")]
    ScriptExhausted,
}

/// A source of proposals. Implementations turn a mandate (the full prompt) into
/// a single decoded [`Proposal`].
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn propose(&self, mandate: &str) -> Result<Proposal, LlmError>;
}

// ── OpenAI-compatible HTTP client ──────────────────────────────────────────

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<Msg<'a>>,
    temperature: f64,
}

#[derive(Serialize)]
struct Msg<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}
#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
}
#[derive(Deserialize)]
struct ChoiceMessage {
    content: String,
}

/// OpenAI-compatible chat client (works with the existing Ollama/gateway infra;
/// point it at a Claude-compatible gateway for production).
pub struct HttpLlmClient {
    client: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
}

impl HttpLlmClient {
    pub fn new(endpoint: impl Into<String>, api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::builder().timeout(Duration::from_secs(60)).build().expect("reqwest build"),
            endpoint: endpoint.into(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }

    /// Build from `AXIOMLAB_LLM_ENDPOINT` / `_API_KEY` / `_MODEL`.
    pub fn from_env() -> Self {
        let endpoint = std::env::var("AXIOMLAB_LLM_ENDPOINT").unwrap_or_else(|_| "http://localhost:11434/v1".into());
        let api_key = std::env::var("AXIOMLAB_LLM_API_KEY").unwrap_or_else(|_| "no-key".into());
        let model = std::env::var("AXIOMLAB_LLM_MODEL").unwrap_or_else(|_| "claude-opus-4-8".into());
        Self::new(endpoint, api_key, model)
    }
}

#[async_trait]
impl LlmClient for HttpLlmClient {
    async fn propose(&self, mandate: &str) -> Result<Proposal, LlmError> {
        let url = format!("{}/chat/completions", self.endpoint);
        let body = ChatRequest {
            model: &self.model,
            messages: vec![
                Msg { role: "system", content: mandate },
                Msg { role: "user", content: "Propose your next step as a single JSON object." },
            ],
            temperature: 0.2,
        };
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Http(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(LlmError::Status(status.as_u16(), text));
        }
        let parsed: ChatResponse = resp.json().await.map_err(|e| LlmError::Http(e.to_string()))?;
        let content = parsed.choices.into_iter().next().map(|c| c.message.content).ok_or(LlmError::Empty)?;
        Ok(proposal::parse(&content)?)
    }
}

// ── Scripted client (tests / deterministic replay) ─────────────────────────

/// Returns pre-baked responses in order — for tests and offline replay.
pub struct ScriptedClient {
    responses: Vec<String>,
    idx: Mutex<usize>,
}

impl ScriptedClient {
    pub fn new(responses: Vec<String>) -> Self {
        Self { responses, idx: Mutex::new(0) }
    }
}

#[async_trait]
impl LlmClient for ScriptedClient {
    async fn propose(&self, _mandate: &str) -> Result<Proposal, LlmError> {
        let mut idx = self.idx.lock().unwrap();
        let raw = self.responses.get(*idx).ok_or(LlmError::ScriptExhausted)?.clone();
        *idx += 1;
        Ok(proposal::parse(&raw)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn scripted_returns_in_order() {
        let c = ScriptedClient::new(vec![
            r#"{"tool":"propose_protocol","steps":[]}"#.into(),
            r#"{"tool":"done","summary":"fin"}"#.into(),
        ]);
        assert!(matches!(c.propose("").await.unwrap(), Proposal::Protocol(_)));
        assert!(matches!(c.propose("").await.unwrap(), Proposal::Done { .. }));
        assert!(matches!(c.propose("").await, Err(LlmError::ScriptExhausted)));
    }
}
