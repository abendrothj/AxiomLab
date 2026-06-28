//! Calibration analysis result tests.
//!
//! Verifies that AnalysisResult correctly classifies calibration fit quality.

use agent_runtime::reasoning::AnalysisResult;

#[test]
fn excellent_calibration_fit_is_accepted() {
    let r = AnalysisResult {
        r_squared: 0.998,
        rmse: Some(0.002),
        normal_residuals: Some(true),
        convergence_score: Some(1.0),
        sample_size: 6,
    };
    assert!(r.is_fit_acceptable(), "R²=0.998 should be an acceptable calibration fit");
    assert!(!r.suggests_nonlinearity());
}

#[test]
fn poor_calibration_fit_flags_nonlinearity() {
    let r = AnalysisResult {
        r_squared: 0.74,
        rmse: Some(0.18),
        normal_residuals: Some(false),
        convergence_score: Some(0.60),
        sample_size: 6,
    };
    assert!(!r.is_fit_acceptable());
    assert!(r.suggests_nonlinearity(), "R²=0.74 should suggest a non-linear model");
}

#[test]
fn borderline_fit_is_neither_accepted_nor_flagged() {
    let r = AnalysisResult {
        r_squared: 0.92,
        rmse: Some(0.05),
        normal_residuals: None,
        convergence_score: None,
        sample_size: 8,
    };
    assert!(!r.is_fit_acceptable(), "R²=0.92 is below the 0.95 acceptance threshold");
    assert!(!r.suggests_nonlinearity(), "R²=0.92 is above the 0.90 nonlinearity threshold");
}
