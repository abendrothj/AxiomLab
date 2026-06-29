//! `analyze_series` — curve fitting offered to the LLM as a (non-gate) tool.
//!
//! Fits OLS / Hill / Michaelis-Menten and selects the best by AIC. When the
//! caller wants to calibrate an instrument, the analysis is held to real
//! metrological standards: the x-axis must be **certified reference materials**
//! (registered standards), there must be enough distinct levels, and the fit
//! must clear an acceptance threshold. Only then does it propose a calibration —
//! and the proposal is *not* recorded here; the orchestrator routes it through
//! operator approval first (calibration unlocks measurement).
//!
//! This breaks the self-licking loop: an instrument can no longer be calibrated
//! against arbitrary data it produced about unknown samples — the truth axis
//! comes from outside the instrument.

use crate::calibration::ProposedCalibration;
use crate::fitting::{hill_equation_fit, linear_regression, michaelis_menten_fit};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;

/// Minimum fit quality that may produce a calibration.
pub const CALIBRATION_R2_THRESHOLD: f64 = 0.80;
/// Minimum number of distinct standard levels required to calibrate.
pub const MIN_CALIBRATION_LEVELS: usize = 5;

#[derive(Debug, Deserialize)]
pub struct AnalyzeRequest {
    /// Independent variable. For a calibration these are the **certified
    /// concentrations of the reference standards**, not arbitrary values.
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    /// `"linear"`, `"hill"`, `"mm"`, or `"auto"` (default).
    #[serde(default)]
    pub model: Option<String>,
    /// Instrument to calibrate, if this is a calibration run.
    #[serde(default)]
    pub instrument: Option<String>,
    /// Reference-material ID backing each x point — the provenance that makes the
    /// x-axis trustworthy. Required (one per x) to propose a calibration.
    #[serde(default)]
    pub reference_material_ids: Option<Vec<String>>,
}

/// The result of an analysis: a JSON summary, plus an optional calibration that
/// the caller must still get approved and recorded.
pub struct AnalyzeOutcome {
    pub summary: Value,
    pub proposed_calibration: Option<ProposedCalibration>,
}

/// Fit the series and, if a valid calibration is warranted, propose one.
///
/// `registered_standards` is the set of reference-material IDs known to the lab
/// (from `LabState`). A calibration is proposed only when every x point is
/// backed by a registered standard and the metrological criteria are met.
pub fn analyze_series(
    req: &AnalyzeRequest,
    registered_standards: &HashSet<String>,
) -> Result<AnalyzeOutcome, String> {
    if req.x.len() != req.y.len() || req.x.len() < 2 {
        return Err("analyze_series needs matching x/y arrays of length ≥ 2".into());
    }
    let ss_tot = ss_total(&req.y);
    let want = req.model.as_deref().unwrap_or("auto");

    let mut candidates: Vec<(String, f64, f64, Value)> = Vec::new(); // (model, aic, r2, params)
    if want == "auto" || want == "linear" {
        if let Some(f) = linear_regression(&req.x, &req.y) {
            candidates.push(("linear".into(), f.aic(), f.r_squared, json!({ "slope": f.slope, "intercept": f.intercept })));
        }
    }
    if want == "auto" || want == "hill" {
        if let Some(f) = hill_equation_fit(&req.x, &req.y) {
            candidates.push(("hill".into(), f.aic(), r2_from_ss(f.ss_res, ss_tot), json!({ "e_max": f.e_max, "ec50": f.ec50, "hill_n": f.hill_n })));
        }
    }
    if want == "auto" || want == "mm" {
        if let Some(f) = michaelis_menten_fit(&req.x, &req.y) {
            candidates.push(("mm".into(), f.aic(), r2_from_ss(f.ss_res, ss_tot), json!({ "v_max": f.v_max, "km": f.km })));
        }
    }

    let (model, _aic, r_squared, params) = candidates
        .into_iter()
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .ok_or_else(|| "no model could be fit to the data".to_string())?;

    let mut summary = json!({
        "model": model,
        "r_squared": r_squared,
        "n": req.x.len(),
        "params": params,
        "calibration_proposed": false,
    });

    let proposed = match propose_calibration(req, &model, r_squared, registered_standards) {
        Ok(Some(cal)) => {
            summary["calibration_proposed"] = json!(true);
            summary["calibration_levels"] = json!(cal.n_levels);
            Some(cal)
        }
        Ok(None) => None,
        Err(reason) => {
            // Calibration was requested but didn't qualify — non-fatal; the fit
            // is still returned and the reason surfaced to the model.
            summary["calibration_skipped"] = json!(reason);
            None
        }
    };

    Ok(AnalyzeOutcome { summary, proposed_calibration: proposed })
}

/// Decide whether a calibration may be proposed. `Ok(None)` means "not a
/// calibration run" (no instrument); `Err` means "requested but rejected".
fn propose_calibration(
    req: &AnalyzeRequest,
    model: &str,
    r_squared: f64,
    registered_standards: &HashSet<String>,
) -> Result<Option<ProposedCalibration>, String> {
    let Some(instrument) = &req.instrument else {
        return Ok(None); // not a calibration run
    };

    // 1. Provenance: every x point must be a registered reference material.
    let ids = req.reference_material_ids.as_ref().ok_or_else(|| {
        "calibration requires reference_material_ids (certified standards) for the x-axis".to_string()
    })?;
    if ids.len() != req.x.len() {
        return Err("reference_material_ids must have one entry per x value".into());
    }
    if let Some(unknown) = ids.iter().find(|id| !registered_standards.contains(*id)) {
        return Err(format!("unregistered reference material: '{unknown}'"));
    }

    // 2. Enough distinct standard levels.
    let n_levels = distinct_levels(&req.x);
    if n_levels < MIN_CALIBRATION_LEVELS {
        return Err(format!(
            "calibration needs ≥{MIN_CALIBRATION_LEVELS} distinct standard levels, got {n_levels}"
        ));
    }

    // 3. Fit quality.
    if !r_squared.is_finite() || r_squared < CALIBRATION_R2_THRESHOLD {
        return Err(format!("fit R²={r_squared:.3} below threshold {CALIBRATION_R2_THRESHOLD}"));
    }

    let mut standard_ids: Vec<String> = ids.clone();
    standard_ids.sort();
    standard_ids.dedup();

    Ok(Some(ProposedCalibration {
        instrument: instrument.clone(),
        valid_until: now_secs() + calibration_ttl_secs(),
        r_squared,
        model: model.to_string(),
        standard_ids,
        n_levels,
    }))
}

fn distinct_levels(x: &[f64]) -> usize {
    let mut seen: Vec<f64> = Vec::new();
    for &v in x {
        if !seen.iter().any(|&s| (s - v).abs() < 1e-9) {
            seen.push(v);
        }
    }
    seen.len()
}

fn ss_total(y: &[f64]) -> f64 {
    let mean = y.iter().sum::<f64>() / y.len() as f64;
    y.iter().map(|&yi| (yi - mean).powi(2)).sum()
}
fn r2_from_ss(ss_res: f64, ss_tot: f64) -> f64 {
    if ss_tot > 0.0 { 1.0 - ss_res / ss_tot } else { 0.0 }
}
fn calibration_ttl_secs() -> u64 {
    std::env::var("AXIOMLAB_CALIBRATION_TTL_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(86_400)
}
fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standards(n: usize) -> (Vec<String>, HashSet<String>) {
        let ids: Vec<String> = (0..n).map(|i| format!("std-{i}")).collect();
        let set: HashSet<String> = ids.iter().cloned().collect();
        (ids, set)
    }

    fn linear_req(instrument: Option<&str>, with_ids: bool) -> (AnalyzeRequest, HashSet<String>) {
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let y = vec![2.0, 4.0, 6.0, 8.0, 10.0];
        let (ids, set) = standards(5);
        let req = AnalyzeRequest {
            x,
            y,
            model: Some("linear".into()),
            instrument: instrument.map(String::from),
            reference_material_ids: if with_ids { Some(ids) } else { None },
        };
        (req, set)
    }

    #[test]
    fn good_fit_with_standards_proposes_calibration() {
        let (req, set) = linear_req(Some("spectrophotometer"), true);
        let out = analyze_series(&req, &set).unwrap();
        let cal = out.proposed_calibration.expect("should propose");
        assert_eq!(cal.instrument, "spectrophotometer");
        assert_eq!(cal.n_levels, 5);
        assert_eq!(cal.standard_ids.len(), 5);
        assert_eq!(out.summary["calibration_proposed"], true);
    }

    #[test]
    fn no_instrument_means_no_calibration() {
        let (req, set) = linear_req(None, true);
        assert!(analyze_series(&req, &set).unwrap().proposed_calibration.is_none());
    }

    #[test]
    fn missing_provenance_is_rejected() {
        let (req, set) = linear_req(Some("spectrophotometer"), false);
        let out = analyze_series(&req, &set).unwrap();
        assert!(out.proposed_calibration.is_none());
        assert!(out.summary["calibration_skipped"].as_str().unwrap().contains("reference_material_ids"));
    }

    #[test]
    fn unregistered_standard_is_rejected() {
        let (mut req, _set) = linear_req(Some("spectrophotometer"), true);
        req.reference_material_ids = Some(vec!["forged".into(); 5]);
        let empty = HashSet::new();
        let out = analyze_series(&req, &empty).unwrap();
        assert!(out.proposed_calibration.is_none());
        assert!(out.summary["calibration_skipped"].as_str().unwrap().contains("unregistered"));
    }

    #[test]
    fn too_few_levels_is_rejected() {
        let (ids, set) = standards(3);
        let req = AnalyzeRequest {
            x: vec![1.0, 2.0, 3.0],
            y: vec![2.0, 4.0, 6.0],
            model: Some("linear".into()),
            instrument: Some("spectrophotometer".into()),
            reference_material_ids: Some(ids),
        };
        let out = analyze_series(&req, &set).unwrap();
        assert!(out.proposed_calibration.is_none());
        assert!(out.summary["calibration_skipped"].as_str().unwrap().contains("distinct standard levels"));
    }

    #[test]
    fn poor_fit_is_rejected() {
        let (ids, set) = standards(5);
        let req = AnalyzeRequest {
            x: vec![1.0, 2.0, 3.0, 4.0, 5.0],
            y: vec![5.0, 1.0, 8.0, 2.0, 9.0], // noise
            model: Some("linear".into()),
            instrument: Some("spectrophotometer".into()),
            reference_material_ids: Some(ids),
        };
        let out = analyze_series(&req, &set).unwrap();
        assert!(out.proposed_calibration.is_none());
        assert!(out.summary["calibration_skipped"].as_str().unwrap().contains("below threshold"));
    }

    #[test]
    fn rejects_mismatched_arrays() {
        let req = AnalyzeRequest { x: vec![1.0], y: vec![1.0, 2.0], model: None, instrument: None, reference_material_ids: None };
        assert!(analyze_series(&req, &HashSet::new()).is_err());
    }
}
