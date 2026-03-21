//! Production HTTP LLM backend with retry / back-off.
//!
//! `HttpLlmBackend` wraps an OpenAI-compatible chat endpoint and adds:
//!
//! | Error class                     | Action                                      |
//! |---------------------------------|---------------------------------------------|
//! | HTTP 429 (rate limit)           | Wait `Retry-After` header or 60 s, then retry |
//! | Timeout / 5xx server error      | Exponential back-off: 500 → 1 000 → 2 000 ms + 0–200 ms jitter |
//! | HTTP 401/403 (auth)             | Fail immediately — no retry                 |
//! | Other 4xx / parse error         | Fail immediately — no retry                 |
//! | Retries exhausted               | Return `LlmError::RetriesExhausted`         |

use std::time::Duration;

use rand::Rng;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::llm::{ChatMessage, LlmBackend, LlmError};

// ── Wire protocol types ───────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    temperature: f64,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Debug, Deserialize)]
struct ChoiceMessage {
    content: String,
}

// ── HttpLlmBackend ────────────────────────────────────────────────────────────

/// Production LLM client — OpenAI-compatible endpoint with retry/back-off.
pub struct HttpLlmBackend {
    client:      reqwest::Client,
    endpoint:    String,
    api_key:     String,
    model:       String,
    max_retries: u32,
}

impl HttpLlmBackend {
    /// Build from explicit values.
    pub fn new(
        endpoint:    impl Into<String>,
        api_key:     impl Into<String>,
        model:       impl Into<String>,
        max_retries: u32,
        timeout:     Duration,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("reqwest client build");
        Self {
            client,
            endpoint:    endpoint.into(),
            api_key:     api_key.into(),
            model:       model.into(),
            max_retries,
        }
    }

    /// Build from environment variables with sensible defaults.
    ///
    /// | Variable                | Default                                 |
    /// |-------------------------|-----------------------------------------|
    /// | `AXIOMLAB_LLM_ENDPOINT` | `http://localhost:11434/v1`             |
    /// | `AXIOMLAB_LLM_API_KEY`  | `no-key`                                |
    /// | `AXIOMLAB_LLM_MODEL`    | `qwen2.5-coder:7b`                      |
    pub fn from_env() -> Self {
        let endpoint = std::env::var("AXIOMLAB_LLM_ENDPOINT")
            .unwrap_or_else(|_| "http://localhost:11434/v1".into());
        let api_key = std::env::var("AXIOMLAB_LLM_API_KEY")
            .unwrap_or_else(|_| "no-key".into());
        let model = std::env::var("AXIOMLAB_LLM_MODEL")
            .unwrap_or_else(|_| "qwen2.5-coder:7b".into());
        Self::new(endpoint, api_key, model, 3, Duration::from_secs(30))
    }

    /// Make a single HTTP attempt (no retry).
    async fn attempt(
        &self,
        messages: &[ChatMessage],
        temperature: f64,
    ) -> Result<String, LlmError> {
        let url = format!("{}/chat/completions", self.endpoint);
        let body = ChatRequest { model: &self.model, messages, temperature };

        let response = self.client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    LlmError::Timeout
                } else {
                    LlmError::Http(e)
                }
            })?;

        let status = response.status();
        let status_u16 = status.as_u16();

        if status_u16 == 429 {
            // Parse Retry-After header (seconds or HTTP-date — we only handle seconds).
            let retry_after_secs = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok());
            return Err(LlmError::RateLimit { retry_after_secs });
        }

        if status_u16 == 401 || status_u16 == 403 {
            let body_text = response.text().await.unwrap_or_default();
            return Err(LlmError::AuthError(format!("HTTP {status_u16}: {body_text}")));
        }

        if !status.is_success() {
            let body_text = response.text().await.unwrap_or_default();
            return Err(LlmError::ServerError(status_u16, body_text));
        }

        let parsed: ChatResponse = response
            .json()
            .await
            .map_err(|e| LlmError::ParseError(e.to_string()))?;

        parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or(LlmError::EmptyResponse)
    }

    /// True if this error class is retryable.
    fn is_retryable(err: &LlmError) -> bool {
        match err {
            LlmError::RateLimit { .. }           => true,
            LlmError::Timeout                    => true,
            LlmError::ServerError(code, _)       => *code >= 500,
            LlmError::Http(e)                    => e.is_connect() || e.is_timeout(),
            LlmError::AuthError(_)               => false,
            LlmError::ParseError(_)              => false,
            LlmError::EmptyResponse              => false,
            LlmError::MissingEndpoint            => false,
            LlmError::MissingApiKey              => false,
            LlmError::RetriesExhausted { .. }    => false,
        }
    }

    /// Compute sleep duration for attempt `n` (0-based).
    ///
    /// Returns `None` when the error itself specifies a `Retry-After` delay
    /// (caller should use that value directly); otherwise returns exponential
    /// back-off with jitter.
    fn backoff_duration(err: &LlmError, attempt: u32) -> Duration {
        if let LlmError::RateLimit { retry_after_secs: Some(secs) } = err {
            return Duration::from_secs(*secs);
        }
        // Exponential: 500ms → 1000ms → 2000ms → …
        let base_ms: u64 = 500 * (1u64 << attempt.min(4));
        let jitter_ms: u64 = rand::thread_rng().gen_range(0..=200);
        Duration::from_millis(base_ms + jitter_ms)
    }
}

impl LlmBackend for HttpLlmBackend {
    async fn chat(&self, messages: &[ChatMessage], temperature: f64) -> Result<String, LlmError> {
        let mut last_err = None;
        for attempt in 0..=self.max_retries {
            match self.attempt(messages, temperature).await {
                Ok(text) => return Ok(text),
                Err(err) if Self::is_retryable(&err) => {
                    let delay = Self::backoff_duration(&err, attempt);
                    warn!(
                        attempt,
                        delay_ms = delay.as_millis(),
                        error = %err,
                        "LLM request failed — retrying"
                    );
                    tokio::time::sleep(delay).await;
                    last_err = Some(err);
                }
                Err(err) => return Err(err),
            }
        }
        Err(LlmError::RetriesExhausted {
            attempts:  self.max_retries + 1,
            source:    Box::new(last_err.expect("loop ran at least once")),
        })
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Rate-limit error is retryable; auth error is not.
    #[test]
    fn retryability() {
        assert!(HttpLlmBackend::is_retryable(&LlmError::RateLimit { retry_after_secs: None }));
        assert!(HttpLlmBackend::is_retryable(&LlmError::Timeout));
        assert!(HttpLlmBackend::is_retryable(&LlmError::ServerError(503, "overloaded".into())));
        assert!(!HttpLlmBackend::is_retryable(&LlmError::ServerError(400, "bad req".into())));
        assert!(!HttpLlmBackend::is_retryable(&LlmError::AuthError("denied".into())));
        assert!(!HttpLlmBackend::is_retryable(&LlmError::ParseError("bad json".into())));
        assert!(!HttpLlmBackend::is_retryable(&LlmError::EmptyResponse));
    }

    /// `Retry-After` header takes precedence over exponential back-off.
    #[test]
    fn retry_after_header_used() {
        let err = LlmError::RateLimit { retry_after_secs: Some(42) };
        let dur = HttpLlmBackend::backoff_duration(&err, 0);
        assert_eq!(dur, Duration::from_secs(42));
    }

    /// Exponential back-off grows with attempt number.
    #[test]
    fn backoff_grows_with_attempt() {
        // Without Retry-After, back-off is at least 500ms * 2^attempt.
        let err0 = LlmError::Timeout;
        let d0 = HttpLlmBackend::backoff_duration(&err0, 0).as_millis();
        let d1 = HttpLlmBackend::backoff_duration(&err0, 1).as_millis();
        let d2 = HttpLlmBackend::backoff_duration(&err0, 2).as_millis();
        // Base (without jitter): 500, 1000, 2000 — the jitter range is 0..=200ms
        assert!(d0 >= 500, "attempt 0 base = 500ms, got {d0}");
        assert!(d1 >= 1000, "attempt 1 base = 1000ms, got {d1}");
        assert!(d2 >= 2000, "attempt 2 base = 2000ms, got {d2}");
    }

    /// RetriesExhausted wraps the last error correctly.
    #[test]
    fn retries_exhausted_wraps_last_error() {
        let inner = LlmError::Timeout;
        let outer = LlmError::RetriesExhausted { attempts: 4, source: Box::new(inner) };
        let msg = outer.to_string();
        assert!(msg.contains("4"), "attempt count in message: {msg}");
        assert!(msg.contains("timed out"), "inner error in message: {msg}");
    }
}
