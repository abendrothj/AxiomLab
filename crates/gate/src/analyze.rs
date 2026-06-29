//! `analyze_series` — curve fitting offered to the LLM as a (non-gate) tool.
//!
//! Fits OLS / Hill / Michaelis-Menten, selects the best by AIC, and — when the
//! fit is good enough (R² ≥ 0.80) and an instrument is named — writes a
//! calibration record into the audit chain. That record is what later unlocks
//! the measurement tools at the `CalibrationGate`.

use crate::calibration::record_calibration;
use crate::fitting::{hill_equation_fit, linear_regression, michaelis_menten_fit};
use axiom_audit::{Chain, Signer};
use serde::Deserialize;
use serde_json::{Value, json};

/// Minimum fit quality that produces a calibration record.
pub const CALIBRATION_R2_THRESHOLD: f64 = 0.80;

#[derive(Debug, Deserialize)]
pub struct AnalyzeRequest {
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    /// `"linear"`, `"hill"`, `"mm"`, or `"auto"` (default).
    #[serde(default)]
    pub model: Option<String>,
    /// Instrument to calibrate if the fit clears the threshold.
    #[serde(default)]
    pub instrument: Option<String>,
}

/// Run the analysis. Records a calibration entry when warranted, and returns a
/// JSON summary (best model, parameters, R², calibration id if recorded).
pub fn analyze_series(
    req: &AnalyzeRequest,
    chain: &Chain,
    signer: &dyn Signer,
) -> Result<Value, String> {
    if req.x.len() != req.y.len() || req.x.len() < 2 {
        return Err("analyze_series needs matching x/y arrays of length ≥ 2".into());
    }
    let ss_tot = ss_total(&req.y);
    let want = req.model.as_deref().unwrap_or("auto");

    let mut candidates: Vec<(String, f64, f64, Value)> = Vec::new(); // (model, aic, r2, params)

    if want == "auto" || want == "linear" {
        if let Some(f) = linear_regression(&req.x, &req.y) {
            let r2 = f.r_squared;
            candidates.push((
                "linear".into(),
                f.aic(),
                r2,
                json!({ "slope": f.slope, "intercept": f.intercept }),
            ));
        }
    }
    if want == "auto" || want == "hill" {
        if let Some(f) = hill_equation_fit(&req.x, &req.y) {
            candidates.push((
                "hill".into(),
                f.aic(),
                r2_from_ss(f.ss_res, ss_tot),
                json!({ "e_max": f.e_max, "ec50": f.ec50, "hill_n": f.hill_n }),
            ));
        }
    }
    if want == "auto" || want == "mm" {
        if let Some(f) = michaelis_menten_fit(&req.x, &req.y) {
            candidates.push((
                "mm".into(),
                f.aic(),
                r2_from_ss(f.ss_res, ss_tot),
                json!({ "v_max": f.v_max, "km": f.km }),
            ));
        }
    }

    let (model, _aic, r_squared, params) = candidates
        .into_iter()
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .ok_or_else(|| "no model could be fit to the data".to_string())?;

    let mut result = json!({
        "model": model,
        "r_squared": r_squared,
        "n": req.x.len(),
        "params": params,
        "calibration_recorded": false,
    });

    if r_squared >= CALIBRATION_R2_THRESHOLD {
        if let Some(instrument) = &req.instrument {
            let valid_until = now_secs() + calibration_ttl_secs();
            let id = record_calibration(chain, signer, instrument, valid_until, r_squared)
                .map_err(|e| format!("record calibration: {e}"))?;
            result["calibration_recorded"] = json!(true);
            result["calibration_id"] = json!(id);
            result["calibration_valid_until"] = json!(valid_until);
        }
    }

    Ok(result)
}

fn ss_total(y: &[f64]) -> f64 {
    let mean = y.iter().sum::<f64>() / y.len() as f64;
    y.iter().map(|&yi| (yi - mean).powi(2)).sum()
}
fn r2_from_ss(ss_res: f64, ss_tot: f64) -> f64 {
    if ss_tot > 0.0 { 1.0 - ss_res / ss_tot } else { 0.0 }
}
fn calibration_ttl_secs() -> u64 {
    std::env::var("AXIOMLAB_CALIBRATION_TTL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(86_400)
}
fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calibration::latest_valid_until;
    use axiom_audit::LocalSigner;

    fn setup() -> (Chain, LocalSigner, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (Chain::open(dir.path().join("a.jsonl")), LocalSigner::generate(), dir)
    }

    #[test]
    fn good_linear_fit_records_calibration() {
        let (chain, s, _d) = setup();
        let req = AnalyzeRequest {
            x: vec![1.0, 2.0, 3.0, 4.0, 5.0],
            y: vec![2.0, 4.0, 6.0, 8.0, 10.0],
            model: Some("linear".into()),
            instrument: Some("spectrophotometer".into()),
        };
        let r = analyze_series(&req, &chain, &s).unwrap();
        assert_eq!(r["model"], "linear");
        assert_eq!(r["calibration_recorded"], true);
        assert!(latest_valid_until(&chain, "spectrophotometer").unwrap().is_some());
    }

    #[test]
    fn poor_fit_records_nothing() {
        let (chain, s, _d) = setup();
        let req = AnalyzeRequest {
            x: vec![1.0, 2.0, 3.0, 4.0, 5.0],
            y: vec![5.0, 1.0, 8.0, 2.0, 9.0], // noise, low R²
            model: Some("linear".into()),
            instrument: Some("spectrophotometer".into()),
        };
        let r = analyze_series(&req, &chain, &s).unwrap();
        assert_eq!(r["calibration_recorded"], false);
        assert!(latest_valid_until(&chain, "spectrophotometer").unwrap().is_none());
    }

    #[test]
    fn no_instrument_means_no_calibration() {
        let (chain, s, _d) = setup();
        let req = AnalyzeRequest {
            x: vec![1.0, 2.0, 3.0, 4.0, 5.0],
            y: vec![2.0, 4.0, 6.0, 8.0, 10.0],
            model: Some("linear".into()),
            instrument: None,
        };
        let r = analyze_series(&req, &chain, &s).unwrap();
        assert_eq!(r["calibration_recorded"], false);
    }

    #[test]
    fn rejects_mismatched_arrays() {
        let (chain, s, _d) = setup();
        let req = AnalyzeRequest { x: vec![1.0], y: vec![1.0, 2.0], model: None, instrument: None };
        assert!(analyze_series(&req, &chain, &s).is_err());
    }
}
