//! Experiment lifecycle state machine.
//!
//! An experiment progresses through:
//!   Proposed → Executing → Completed
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
    /// The LLM is designing the experimental approach.
    Proposed,
    /// The LLM is issuing tool calls and collecting data.
    Executing,
    /// Experiment completed — done signal received.
    Completed,
    /// Experiment failed at some stage.
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Experiment {
    pub id: String,
    pub hypothesis: String,
    pub stage: Stage,
    /// Raw results (populated at `Completed`).
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
            results: None,
            error: None,
        }
    }

    /// Advance to the next stage, enforcing valid transitions.
    pub fn advance(&mut self, to: Stage) -> Result<(), ExperimentError> {
        if to == Stage::Failed {
            self.stage = Stage::Failed;
            return Ok(());
        }
        let valid_next = match self.stage {
            Stage::Proposed   => Stage::Executing,
            Stage::Executing  => Stage::Completed,
            Stage::Completed | Stage::Failed => {
                return Err(ExperimentError::InvalidTransition { from: self.stage, to });
            }
        };
        if to != valid_next {
            return Err(ExperimentError::InvalidTransition { from: self.stage, to });
        }
        self.stage = to;
        Ok(())
    }

    /// Convenience: mark failed with a reason.
    pub fn fail(&mut self, reason: impl Into<String>) {
        self.error = Some(reason.into());
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
        exp.advance(Stage::Executing).unwrap();
        exp.advance(Stage::Completed).unwrap();
        assert_eq!(exp.stage, Stage::Completed);
    }

    #[test]
    fn reject_skip() {
        let mut exp = Experiment::new("exp-002", "test");
        assert!(exp.advance(Stage::Completed).is_err());
    }

    #[test]
    fn fail_from_any_stage() {
        let mut exp = Experiment::new("exp-003", "test");
        exp.advance(Stage::Executing).unwrap();
        exp.fail("hardware error");
        assert_eq!(exp.stage, Stage::Failed);
        assert!(exp.error.as_ref().unwrap().contains("hardware"));
    }
}
