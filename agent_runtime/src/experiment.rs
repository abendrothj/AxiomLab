//! Experiment lifecycle state machine.
//!
//! An experiment progresses through:
//!   Proposed → CodeGenerated → Verified → Executing → Analysing → Completed
//!
//! Any stage can transition to `Failed`.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExperimentError {
    #[error("invalid transition from {from:?} to {to:?}")]
    InvalidTransition { from: Stage, to: Stage },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Stage {
    /// The LLM has proposed a hypothesis and experiment plan.
    Proposed,
    /// Rust code for the experiment has been generated.
    CodeGenerated,
    /// Verus / Aeneas verification succeeded.
    Verified,
    /// The experiment is running on lab hardware.
    Executing,
    /// Data analysis is in progress.
    Analysing,
    /// Experiment completed successfully.
    Completed,
    /// Experiment failed at some stage.
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Experiment {
    pub id: String,
    pub hypothesis: String,
    pub stage: Stage,
    /// Generated Rust source (populated at `CodeGenerated`).
    pub source_code: Option<String>,
    /// Verification proof (populated at `Verified`).
    pub proof: Option<String>,
    /// Raw results (populated at `Analysing`/`Completed`).
    pub results: Option<serde_json::Value>,
    /// Error message if `Failed`.
    pub error: Option<String>,
}

impl Experiment {
    pub fn new(id: impl Into<String>, hypothesis: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            hypothesis: hypothesis.into(),
            stage: Stage::Proposed,
            source_code: None,
            proof: None,
            results: None,
            error: None,
        }
    }

    /// Advance to the next stage, enforcing valid transitions.
    pub fn advance(&mut self, to: Stage) -> Result<(), ExperimentError> {
        if to == Stage::Failed {
            // Any stage can fail.
            self.stage = Stage::Failed;
            return Ok(());
        }
        let valid_next = match self.stage {
            Stage::Proposed => Stage::CodeGenerated,
            Stage::CodeGenerated => Stage::Verified,
            Stage::Verified => Stage::Executing,
            Stage::Executing => Stage::Analysing,
            Stage::Analysing => Stage::Completed,
            Stage::Completed | Stage::Failed => {
                return Err(ExperimentError::InvalidTransition {
                    from: self.stage,
                    to,
                });
            }
        };
        if to != valid_next {
            return Err(ExperimentError::InvalidTransition {
                from: self.stage,
                to,
            });
        }
        self.stage = to;
        Ok(())
    }

    /// Convenience: mark failed with a reason.
    pub fn fail(&mut self, reason: impl Into<String>) {
        self.error = Some(reason.into());
        // advance(Failed) never errors
        let _ = self.advance(Stage::Failed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path() {
        let mut exp = Experiment::new("exp-001", "NaOH + HCl → NaCl + H₂O");
        assert_eq!(exp.stage, Stage::Proposed);

        exp.advance(Stage::CodeGenerated).unwrap();
        exp.advance(Stage::Verified).unwrap();
        exp.advance(Stage::Executing).unwrap();
        exp.advance(Stage::Analysing).unwrap();
        exp.advance(Stage::Completed).unwrap();
        assert_eq!(exp.stage, Stage::Completed);
    }

    #[test]
    fn reject_skip() {
        let mut exp = Experiment::new("exp-002", "test");
        assert!(exp.advance(Stage::Executing).is_err());
    }

    #[test]
    fn fail_from_any_stage() {
        let mut exp = Experiment::new("exp-003", "test");
        exp.advance(Stage::CodeGenerated).unwrap();
        exp.fail("verus rejected proof");
        assert_eq!(exp.stage, Stage::Failed);
        assert!(exp.error.as_ref().unwrap().contains("verus"));
    }
}
