//! VeruSAGE-inspired proof-synthesis agent loop.
//!
//! Observation → Reasoning → Action cycle:
//! 1. **Observe**: compile candidate with Verus, collect diagnostics.
//! 2. **Reason**: feed diagnostics + source to the LLM, ask for fixes.
//! 3. **Act**: apply the LLM's proposed annotations/edits.
//! Repeat until Verus accepts (success) or the retry budget is exhausted.

use crate::compiler::{invoke_verus, CompileResult};
use crate::diagnostics;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{error, info, warn};

// ── Errors ───────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum SynthError {
    #[error("proof synthesis failed after {attempts} attempts")]
    ExhaustedRetries { attempts: u32 },
    #[error("compiler error: {0}")]
    Compiler(#[from] crate::compiler::CompilerError),
    #[error("LLM error: {0}")]
    Llm(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

// ── Configuration ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SynthConfig {
    /// Maximum refinement iterations.
    pub max_retries: u32,
    /// LLM temperature for proof generation.
    pub temperature: f64,
    /// LLM endpoint (OpenAI-compatible).
    pub llm_endpoint: String,
    /// LLM API key.
    pub llm_api_key: String,
    /// LLM model name.
    pub llm_model: String,
}

impl Default for SynthConfig {
    fn default() -> Self {
        Self {
            max_retries: 5,
            temperature: 0.2,
            llm_endpoint: std::env::var("AXIOMLAB_LLM_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:11434/v1".into()),
            llm_api_key: std::env::var("AXIOMLAB_LLM_API_KEY")
                .unwrap_or_else(|_| "no-key".into()),
            llm_model: std::env::var("AXIOMLAB_LLM_MODEL")
                .unwrap_or_else(|_| "gpt-4o".into()),
        }
    }
}

// ── LLM call types ──────────────────────────────────────────────

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Msg>,
    temperature: f64,
}

#[derive(Serialize, Deserialize, Clone)]
struct Msg {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Msg,
}

// ── Core agent ───────────────────────────────────────────────────

/// Run proof synthesis on `source_code`, returning the annotated
/// version with Verus proof blocks, or an error.
pub async fn synthesize_proof(
    source_code: &str,
    config: &SynthConfig,
) -> Result<String, SynthError> {
    let work_dir = tempfile::tempdir()?;
    let http = reqwest::Client::new();

    let mut current_source = source_code.to_owned();
    let mut history: Vec<Msg> = vec![Msg {
        role: "system".into(),
        content: SYSTEM_PROMPT.into(),
    }];

    for attempt in 1..=config.max_retries {
        info!(attempt, "proof synthesis iteration");

        // ── 1. Observe: compile with Verus ──
        let result: CompileResult = invoke_verus(&current_source, work_dir.path()).await?;

        if result.success {
            info!(attempt, "Verus accepted the proof");
            return Ok(current_source);
        }

        // ── 2. Reason: parse diagnostics, ask LLM ──
        let diags = diagnostics::parse(&result.output);
        let summary = diagnostics::summarize(&diags);
        warn!(attempt, errors = diags.len(), "Verus rejected — refining");

        // Always include full source. We intentionally avoid diff-mode
        // replies because we do not apply unified diffs in this loop.
        let source_lines = current_source.lines().count();
        let source_section =
            format!("\n\n## Current source ({source_lines} lines)\n```rust\n{current_source}\n```");

        let user_msg = format!(
            "Verus verification failed (attempt {attempt}/{max}).\n\
             ## Errors\n{summary}{source_section}\n\n\
                         Fix the proof annotations so Verus accepts.\n\
                         Return ONLY the complete corrected Rust source inside ```rust ... ```.",
            max = config.max_retries,
        );
        history.push(Msg {
            role: "user".into(),
            content: user_msg,
        });

        let reply = llm_chat(&http, config, &history).await?;
        history.push(Msg {
            role: "assistant".into(),
            content: reply.clone(),
        });

        // Trim history to system prompt + the two most recent turns so the
        // context window doesn't grow without bound across retries.
        if history.len() > 5 {
            let system = history[0].clone();
            let recent = history[history.len() - 4..].to_vec();
            history = std::iter::once(system).chain(recent).collect();
        }

        // ── 3. Act: extract corrected source ──
        if let Some(code) = extract_rust_block(&reply) {
            current_source = code;
        } else {
            error!("LLM response did not contain a ```rust block");
        }
    }

    Err(SynthError::ExhaustedRetries {
        attempts: config.max_retries,
    })
}

// ── Internal helpers ─────────────────────────────────────────────

async fn llm_chat(
    http: &reqwest::Client,
    config: &SynthConfig,
    messages: &[Msg],
) -> Result<String, SynthError> {
    let url = format!("{}/chat/completions", config.llm_endpoint);
    let body = ChatRequest {
        model: config.llm_model.clone(),
        messages: messages.to_vec(),
        temperature: config.temperature,
    };

    let resp = http
        .post(&url)
        .bearer_auth(&config.llm_api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| SynthError::Llm(e.to_string()))?
        .error_for_status()
        .map_err(|e| SynthError::Llm(e.to_string()))?
        .json::<ChatResponse>()
        .await
        .map_err(|e| SynthError::Llm(e.to_string()))?;

    resp.choices
        .first()
        .map(|c| c.message.content.clone())
        .ok_or_else(|| SynthError::Llm("empty LLM response".into()))
}

fn extract_rust_block(text: &str) -> Option<String> {
    // Scan all ```rust ... ``` fences and return the *longest* one.
    // LLMs sometimes emit a short illustrative snippet before the main
    // corrected file; grabbing the largest block is more robust than
    // grabbing the first.
    let mut best: Option<String> = None;
    let mut search = text;
    while let Some(fence_start) = search.find("```rust") {
        let code_start = fence_start + 7;
        match search[code_start..].find("```") {
            Some(end) => {
                let code = search[code_start..code_start + end].trim();
                if !code.is_empty()
                    && best.as_ref().map_or(true, |b: &String| code.len() > b.len())
                {
                    best = Some(code.to_owned());
                }
                search = &search[code_start + end + 3..];
            }
            None => break,
        }
    }
    best
}

const SYSTEM_PROMPT: &str = "\
You are a Verus proof engineer. Given Rust source code and Verus \
compiler errors, your task is to add or fix Verus proof annotations \
(requires, ensures, invariant, proof blocks, ghost variables) so that \
the code passes Verus verification. Rules:\n\
- Preserve the original executable logic; only add/fix proof annotations.\n\
- Use `requires(...)` for preconditions, `ensures(...)` for postconditions.\n\
- Use `proof { ... }` blocks for auxiliary lemmas.\n\
- For loop invariants use `invariant(...)` inside the loop body.\n\
- Return the COMPLETE file, not a diff.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_code_block() {
        let text = "Here:\n```rust\nfn f() {}\n```\nDone.";
        assert_eq!(extract_rust_block(text).unwrap(), "fn f() {}");
    }

    #[test]
    fn extract_no_block() {
        assert!(extract_rust_block("nothing here").is_none());
    }
}
