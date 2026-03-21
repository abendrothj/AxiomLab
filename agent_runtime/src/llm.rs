//! LLM client abstraction and OpenAI-compatible implementation.

use serde::{Deserialize, Serialize};
use std::future::Future;
use thiserror::Error;

// ── Error ────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("missing AXIOMLAB_LLM_ENDPOINT env var")]
    MissingEndpoint,
    #[error("missing AXIOMLAB_LLM_API_KEY env var")]
    MissingApiKey,
    #[error("LLM returned empty response")]
    EmptyResponse,
    /// Server returned HTTP 429; `retry_after_secs` from the `Retry-After` header.
    #[error("rate limited (retry after {retry_after_secs:?}s)")]
    RateLimit { retry_after_secs: Option<u64> },
    /// Request timed out (connect or read).
    #[error("LLM request timed out")]
    Timeout,
    /// Authentication rejected (HTTP 401/403).
    #[error("LLM authentication error: {0}")]
    AuthError(String),
    /// Unexpected HTTP error status.
    #[error("LLM server error {0}: {1}")]
    ServerError(u16, String),
    /// Could not parse the response body.
    #[error("LLM response parse error: {0}")]
    ParseError(String),
    /// All retry attempts exhausted; inner error is the last failure.
    #[error("LLM retries exhausted after {attempts} attempt(s): {source}")]
    RetriesExhausted { attempts: u32, source: Box<LlmError> },
}

// ── Message types ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f64,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ChatMessage,
}

// ── Trait ─────────────────────────────────────────────────────────

/// An async LLM backend that can generate completions.
pub trait LlmBackend: Send + Sync {
    fn chat(&self, messages: &[ChatMessage], temperature: f64)
        -> impl Future<Output = Result<String, LlmError>> + Send;
}

// ── OpenAI-compatible implementation ─────────────────────────────

/// Client for any OpenAI-compatible chat endpoint
/// (OpenAI, Azure, Ollama, vLLM, etc.).
pub struct OpenAiClient {
    client: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
}

impl OpenAiClient {
    /// Build from environment variables:
    /// - `AXIOMLAB_LLM_ENDPOINT` (default: `http://localhost:11434/v1`)
    /// - `AXIOMLAB_LLM_API_KEY` (default: `no-key` for local Ollama)
    /// - `AXIOMLAB_LLM_MODEL` (default: `qwen2.5-coder:7b`)
    pub fn from_env() -> Result<Self, LlmError> {
        let endpoint = std::env::var("AXIOMLAB_LLM_ENDPOINT")
            .unwrap_or_else(|_| "http://localhost:11434/v1".to_owned());
        let api_key =
            std::env::var("AXIOMLAB_LLM_API_KEY").unwrap_or_else(|_| "no-key".to_owned());
        let model =
            std::env::var("AXIOMLAB_LLM_MODEL").unwrap_or_else(|_| "qwen2.5-coder:7b".to_owned());
        Ok(Self {
            client: reqwest::Client::new(),
            endpoint,
            api_key,
            model,
        })
    }

    /// Build with explicit values (useful for tests / non-env setups).
    pub fn new(endpoint: String, api_key: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint,
            api_key,
            model,
        }
    }
}

impl LlmBackend for OpenAiClient {
    async fn chat(
        &self,
        messages: &[ChatMessage],
        temperature: f64,
    ) -> Result<String, LlmError> {
        let url = format!("{}/chat/completions", self.endpoint);
        let body = ChatRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            temperature,
        };
        let resp: ChatResponse = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        resp.choices
            .first()
            .map(|c| c.message.content.clone())
            .ok_or(LlmError::EmptyResponse)
    }
}

// ── In-process mock (for tests and offline dev) ──────────────────

/// A deterministic mock backend that echoes the last user message.
pub struct MockLlm;

impl LlmBackend for MockLlm {
    async fn chat(
        &self,
        messages: &[ChatMessage],
        _temperature: f64,
    ) -> Result<String, LlmError> {
        messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| format!("MOCK_RESPONSE: {}", m.content))
            .ok_or(LlmError::EmptyResponse)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_echoes_last_user_message() {
        let mock = MockLlm;
        let msgs = vec![
            ChatMessage {
                role: "system".into(),
                content: "You are a lab assistant.".into(),
            },
            ChatMessage {
                role: "user".into(),
                content: "Measure the pH.".into(),
            },
        ];
        let reply = mock.chat(&msgs, 0.0).await.unwrap();
        assert!(reply.contains("Measure the pH."));
    }
}
