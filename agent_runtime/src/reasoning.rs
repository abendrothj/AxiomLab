//! Statistical analysis result types for calibration verification.

use serde::{Deserialize, Serialize};

/// Captures key fit quality metrics from a calibration or analysis run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult {
    /// Coefficient of determination (fit quality: 0..1)
    pub r_squared: f64,
    /// Root mean squared error
    pub rmse: Option<f64>,
    /// Whether the residuals appear normally distributed
    pub normal_residuals: Option<bool>,
    /// Convergence metric
    pub convergence_score: Option<f64>,
    /// Number of data points used
    pub sample_size: usize,
}

impl AnalysisResult {
    /// Is the calibration fit acceptable (R² ≥ 0.95)?
    pub fn is_fit_acceptable(&self) -> bool {
        self.r_squared > 0.95
    }

    /// Does the fit quality suggest a non-linear calibration model is needed?
    pub fn suggests_nonlinearity(&self) -> bool {
        self.r_squared < 0.90
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_thresholds() {
        let good = AnalysisResult { r_squared: 0.98, rmse: None, normal_residuals: None, convergence_score: None, sample_size: 6 };
        assert!(good.is_fit_acceptable());
        assert!(!good.suggests_nonlinearity());

        let poor = AnalysisResult { r_squared: 0.72, ..good.clone() };
        assert!(!poor.is_fit_acceptable());
        assert!(poor.suggests_nonlinearity());
    }
}
