//! Calibration records, stored in (and read from) the audit chain.
//!
//! There is no separate calibration database. A calibration is an audit entry
//! with `action = "calibration"` whose reason JSON carries the instrument, an
//! expiry (`valid_until`, **mandatory**), and the fit quality that produced it.
//! The `CalibrationGate` reads these back; `analyze_series` writes them.

use axiom_audit::{Chain, ChainError, EntryData, Signer};
use serde_json::json;

/// The measurement tools that require a valid calibration before they may run.
pub fn measurement_instrument(tool: &str) -> Option<&'static str> {
    match tool {
        "read_absorbance" => Some("spectrophotometer"),
        "read_ph" => Some("ph_meter"),
        "read_temperature" => Some("thermal_controller"),
        _ => None,
    }
}

/// A calibration that has met the metrological criteria and is awaiting operator
/// approval before it is recorded. Produced by `analyze_series`.
#[derive(Debug, Clone)]
pub struct ProposedCalibration {
    pub instrument: String,
    pub valid_until: u64,
    pub r_squared: f64,
    pub model: String,
    /// The registered reference-material IDs whose certified values formed the
    /// x-axis — the provenance that makes this calibration traceable.
    pub standard_ids: Vec<String>,
    pub n_levels: usize,
}

/// Append an approved calibration to the chain, with full provenance.
///
/// `approver` is the operator who signed off (calibration unlocks measurement,
/// so it requires approval). The standards, level count, and model are recorded
/// so the calibration is traceable and tamper-evident.
pub fn record_calibration(
    chain: &Chain,
    signer: &dyn Signer,
    cal: &ProposedCalibration,
    approver: &str,
) -> Result<String, ChainError> {
    let calibration_id = uuid::Uuid::new_v4().to_string();
    let reason = json!({
        "calibration_id": calibration_id,
        "instrument": cal.instrument,
        "valid_until": cal.valid_until,
        "r_squared": cal.r_squared,
        "model": cal.model,
        "standard_ids": cal.standard_ids,
        "n_levels": cal.n_levels,
        "approved_by": approver,
    })
    .to_string();
    chain.append(EntryData::new("calibration", "allow", reason, true), signer)?;
    Ok(calibration_id)
}

/// The latest still-valid `valid_until` for `instrument`, scanning the chain.
/// Returns `None` if there is no calibration or the most recent one is expired.
pub fn latest_valid_until(chain: &Chain, instrument: &str) -> Result<Option<u64>, ChainError> {
    let mut latest: Option<u64> = None;
    for e in chain.entries()? {
        if e.action != "calibration" {
            continue;
        }
        let Ok(details) = serde_json::from_str::<serde_json::Value>(&e.reason) else { continue };
        if details.get("instrument").and_then(|v| v.as_str()) != Some(instrument) {
            continue;
        }
        if let Some(vu) = details.get("valid_until").and_then(|v| v.as_u64()) {
            // Entries are append-ordered; later entries supersede earlier ones.
            latest = Some(vu);
        }
    }
    Ok(latest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axiom_audit::LocalSigner;

    fn now() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
    }

    #[test]
    fn instrument_mapping() {
        assert_eq!(measurement_instrument("read_absorbance"), Some("spectrophotometer"));
        assert_eq!(measurement_instrument("dispense"), None);
    }

    fn sample_cal(instrument: &str, valid_until: u64) -> ProposedCalibration {
        ProposedCalibration {
            instrument: instrument.into(),
            valid_until,
            r_squared: 0.99,
            model: "linear".into(),
            standard_ids: vec!["std-0".into(), "std-1".into()],
            n_levels: 5,
        }
    }

    #[test]
    fn record_then_read_latest_with_provenance() {
        let dir = tempfile::tempdir().unwrap();
        let chain = Chain::open(dir.path().join("a.jsonl"));
        let s = LocalSigner::generate();
        let future = now() + 3600;
        record_calibration(&chain, &s, &sample_cal("spectrophotometer", future), "alice").unwrap();
        assert_eq!(latest_valid_until(&chain, "spectrophotometer").unwrap(), Some(future));
        assert_eq!(latest_valid_until(&chain, "ph_meter").unwrap(), None);
        // Provenance is in the signed entry.
        let entry = &chain.entries().unwrap()[0];
        assert!(entry.reason.contains("approved_by"));
        assert!(entry.reason.contains("standard_ids"));
    }
}
