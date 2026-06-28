//! Protocol run lifecycle state machine.
//!
//! A protocol run progresses through:
//!   Proposed → Executing → Analyzing → Completed
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
    /// The agent is planning the approach.
    Proposed,
    /// The agent is issuing tool calls and collecting data.
    Executing,
    /// Data collected; statistical analysis and evidence recording in progress.
    Analyzing,
    /// Run completed — done signal received.
    Completed,
    /// Run failed at some stage.
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Experiment {
    pub id: String,
    /// The operator-issued directive this run is fulfilling.
    pub directive: String,
    pub stage: Stage,
    /// Raw results (populated at `Completed`).
    pub results: Option<serde_json::Value>,
    /// Error message if `Failed`.
    pub error: Option<String>,
}

impl Experiment {
    pub fn new(id: impl Into<String>, directive: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            directive: directive.into(),
            stage: Stage::Proposed,
            results: None,
            error: None,
        }
    }

    /// Advance to the next stage, enforcing valid transitions.
    ///
    /// Valid path: Proposed → Executing → Analyzing → Completed.
    /// Failed is always reachable from any non-terminal stage.
    pub fn advance(&mut self, to: Stage) -> Result<(), ExperimentError> {
        if to == Stage::Failed {
            self.stage = Stage::Failed;
            return Ok(());
        }
        let valid_next = match self.stage {
            Stage::Proposed   => Stage::Executing,
            Stage::Executing  => Stage::Analyzing,
            Stage::Analyzing  => Stage::Completed,
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
        let mut exp = Experiment::new("exp-001", "Calibrate pH meter with pH 4/7/10 buffers");
        assert_eq!(exp.stage, Stage::Proposed);
        exp.advance(Stage::Executing).unwrap();
        exp.advance(Stage::Analyzing).unwrap();
        exp.advance(Stage::Completed).unwrap();
        assert_eq!(exp.stage, Stage::Completed);
    }

    #[test]
    fn reject_skip_analyzing() {
        let mut exp = Experiment::new("exp-002", "test");
        exp.advance(Stage::Executing).unwrap();
        assert!(exp.advance(Stage::Completed).is_err());
    }

    #[test]
    fn reject_skip_executing() {
        let mut exp = Experiment::new("exp-003", "test");
        assert!(exp.advance(Stage::Completed).is_err());
        assert!(exp.advance(Stage::Analyzing).is_err());
    }

    #[test]
    fn fail_from_any_stage() {
        let mut exp = Experiment::new("exp-004", "test");
        exp.advance(Stage::Executing).unwrap();
        exp.fail("hardware error");
        assert_eq!(exp.stage, Stage::Failed);
        assert!(exp.error.as_ref().unwrap().contains("hardware"));
    }

    #[test]
    fn fail_from_analyzing() {
        let mut exp = Experiment::new("exp-005", "test");
        exp.advance(Stage::Executing).unwrap();
        exp.advance(Stage::Analyzing).unwrap();
        exp.fail("analysis crash");
        assert_eq!(exp.stage, Stage::Failed);
    }

    #[test]
    fn no_forward_transition_from_completed() {
        let mut exp = Experiment::new("exp-006", "test");
        exp.advance(Stage::Executing).unwrap();
        exp.advance(Stage::Analyzing).unwrap();
        exp.advance(Stage::Completed).unwrap();
        assert!(exp.advance(Stage::Executing).is_err());
        assert!(exp.advance(Stage::Analyzing).is_err());
    }
}
