//! Autonomous reasoning module for hypothesis generation and decision-making.
//!
//! After each experiment, the agent analyzes metrics and decides:
//! - Is the hypothesis confirmed?
//! - Should we try a different model (e.g., nonlinear)?
//! - Is more data collection needed?
//! - Can we stop?

use serde::{Deserialize, Serialize};
use tracing::info;

/// Captures key metrics from experimental analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult {
    /// Coefficient of determination (fit quality: 0..1)
    pub r_squared: f64,
    /// Root mean squared error
    pub rmse: Option<f64>,
    /// Whether the residuals appear normally distributed
    pub normal_residuals: Option<bool>,
    /// Convergence metric (how stable was the solution?)
    pub convergence_score: Option<f64>,
    /// Number of data points used
    pub sample_size: usize,
}

impl AnalysisResult {
    /// Is the fit good enough to accept the hypothesis?
    pub fn is_fit_acceptable(&self) -> bool {
        self.r_squared > 0.95
    }

    /// Does the fit suggest nonlinearity (poor R²)?
    pub fn suggests_nonlinearity(&self) -> bool {
        self.r_squared < 0.90
    }

    /// Is the fit borderline (ambiguous)?
    pub fn is_fit_ambiguous(&self) -> bool {
        (0.90..=0.95).contains(&self.r_squared)
    }

    /// Is the fit conclusive (either very good or clearly bad)?
    pub fn is_conclusive(&self) -> bool {
        self.is_fit_acceptable() || self.suggests_nonlinearity()
    }
}

/// Recommendation for the next step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NextStep {
    /// The hypothesis is confirmed; no more experiments needed.
    Confirmed,
    /// The fit is poor; recommend trying a nonlinear model.
    TryNonlinear,
    /// The fit is ambiguous; collect more data.
    CollectMore,
    /// Something failed; investigate the cause.
    Debug,
    /// We've exhausted hypotheses; stop.
    Stop,
}

/// Autonomous reasoning engine for hypothesis refinement.
pub struct ReasoningEngine {
    /// How many experiments have been run on this hypothesis family?
    pub experiment_count: u32,
    /// Maximum experiments before giving up on a hypothesis family.
    pub max_experiments: u32,
}

impl Default for ReasoningEngine {
    fn default() -> Self {
        Self {
            experiment_count: 0,
            max_experiments: 5,
        }
    }
}

impl ReasoningEngine {
    /// Analyze results and recommend the next step.
    pub fn decide_next_step(&mut self, analysis: &AnalysisResult) -> (NextStep, String) {
        self.experiment_count += 1;

        let (decision, reason) = if analysis.is_fit_acceptable() {
            (
                NextStep::Confirmed,
                format!(
                    "Excellent fit (R² = {:.4}). Hypothesis confirmed.",
                    analysis.r_squared
                ),
            )
        } else if analysis.suggests_nonlinearity() {
            (
                NextStep::TryNonlinear,
                format!(
                    "Poor linear fit (R² = {:.4}). Try nonlinear or polynomial model.",
                    analysis.r_squared
                ),
            )
        } else if analysis.is_fit_ambiguous() {
            if self.experiment_count < self.max_experiments {
                (
                    NextStep::CollectMore,
                    format!(
                        "Ambiguous fit (R² = {:.4}). Collect more data (attempt {}/{}).",
                        analysis.r_squared, self.experiment_count, self.max_experiments
                    ),
                )
            } else {
                (
                    NextStep::Stop,
                    format!(
                        "Ambiguous fit and max experiments reached (R² = {:.4}).",
                        analysis.r_squared
                    ),
                )
            }
        } else {
            (
                NextStep::Debug,
                "Analysis result is malformed or data collection failed.".to_string(),
            )
        };

        info!(
            r_squared = analysis.r_squared,
            count = self.experiment_count,
            decision = ?decision,
            "reasoning engine decision"
        );

        (decision, reason)
    }

    /// Generate a new hypothesis based on the current analysis and next step.
    pub fn generate_hypothesis(
        &self,
        _current_hypothesis: &str,
        decision: &NextStep,
        analysis: &AnalysisResult,
    ) -> Option<String> {
        match decision {
            NextStep::Confirmed => None, // No new hypothesis needed.

            NextStep::TryNonlinear => Some(format!(
                "The data appears nonlinear (R² = {:.4} from linear fit). \
                 Model the relationship as: y = a·x² + b·x + c (quadratic).",
                analysis.r_squared
            )),

            NextStep::CollectMore => Some(format!(
                "Linear model R² = {:.4} (ambiguous). Collect {} more data points \
                 at different concentrations to improve precision.",
                analysis.r_squared,
                analysis.sample_size / 2
            )),

            NextStep::Stop | NextStep::Debug => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_excellent_fit() {
        let result = AnalysisResult {
            r_squared: 0.99,
            rmse: Some(0.01),
            normal_residuals: Some(true),
            convergence_score: Some(0.95),
            sample_size: 10,
        };

        let mut engine = ReasoningEngine::default();
        let (decision, reason) = engine.decide_next_step(&result);

        assert_eq!(decision, NextStep::Confirmed);
        assert!(reason.contains("Excellent"));
    }

    #[test]
    fn test_poor_fit_suggests_nonlinear() {
        let result = AnalysisResult {
            r_squared: 0.80,
            rmse: Some(0.5),
            normal_residuals: Some(false),
            convergence_score: Some(0.5),
            sample_size: 20,
        };

        let mut engine = ReasoningEngine::default();
        let (decision, reason) = engine.decide_next_step(&result);

        assert_eq!(decision, NextStep::TryNonlinear);
        assert!(reason.contains("nonlinear"));
    }

    #[test]
    fn test_ambiguous_fit_collect_more() {
        let result = AnalysisResult {
            r_squared: 0.92,
            rmse: Some(0.05),
            normal_residuals: Some(true),
            convergence_score: Some(0.80),
            sample_size: 8,
        };

        let mut engine = ReasoningEngine::default();
        let (decision, reason) = engine.decide_next_step(&result);

        assert_eq!(decision, NextStep::CollectMore);
        assert!(reason.contains("more data"));
    }

    #[test]
    fn test_max_experiments_exceeded() {
        let result = AnalysisResult {
            r_squared: 0.93,
            rmse: Some(0.04),
            normal_residuals: Some(true),
            convergence_score: Some(0.85),
            sample_size: 10,
        };

        let mut engine = ReasoningEngine {
            experiment_count: 4,
            max_experiments: 5,
        };

        let (_decision, _) = engine.decide_next_step(&result);
        // After this call, experiment_count = 5, so next call should return Stop
        assert_eq!(engine.experiment_count, 5);

        let (decision2, reason2) = engine.decide_next_step(&result);
        assert_eq!(decision2, NextStep::Stop);
        assert!(reason2.contains("max experiments"));
    }

    #[test]
    fn test_hypothesis_generation() {
        let engine = ReasoningEngine::default();
        let analysis = AnalysisResult {
            r_squared: 0.85,
            rmse: None,
            normal_residuals: None,
            convergence_score: None,
            sample_size: 15,
        };

        let hypothesis =
            engine.generate_hypothesis("Linear model", &NextStep::TryNonlinear, &analysis);

        assert!(hypothesis.is_some());
        let h = hypothesis.unwrap();
        assert!(h.contains("quadratic"));
        assert!(h.contains("0.85"));
    }
}
