//! Proof-synthesis agent loop (skeleton).

use thiserror::Error;
use tracing::info;

#[derive(Debug, Error)]
pub enum SynthError {
    #[error("proof synthesis failed after {attempts} attempts")]
    ExhaustedRetries { attempts: u32 },
}

/// Maximum number of refinement iterations before giving up.
const MAX_RETRIES: u32 = 5;

/// Run proof synthesis on `source_code`, returning the annotated version
/// or an error if verification cannot be achieved within the retry budget.
pub async fn synthesize_proof(source_code: &str) -> Result<String, SynthError> {
    info!(len = source_code.len(), "starting proof synthesis");

    // Stub: in the real implementation each iteration would:
    //   1. Call the LLM to propose Verus annotations.
    //   2. Compile with verus and collect diagnostics.
    //   3. Feed errors back to the LLM for refinement.
    for attempt in 1..=MAX_RETRIES {
        info!(attempt, "proof synthesis iteration (stub)");
    }

    Err(SynthError::ExhaustedRetries {
        attempts: MAX_RETRIES,
    })
}
