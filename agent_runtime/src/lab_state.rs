//! Laboratory state tracking — reagent inventory and vessel contents.
//!
//! `LabState` maintains:
//! - The reagent inventory (what's in stock, concentrations, expiry)
//! - Vessel contents (which reagents have been dispensed into which vessel)
//!
//! # Persistence
//! `LabState` is persisted to `.artifacts/lab_state.json` and dual-written to
//! the SQLite `reagents` / `vessel_contents` tables (via `server/src/db.rs`).
//! On startup the server reconstructs `LabState` from the JSON file if the DB
//! is empty.
//!
//! # Thread safety
//! Callers wrap `LabState` in `Arc<Mutex<LabState>>`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── VesselContribution ─────────────────────────────────────────────────────────

/// One dispensing event tracked in a vessel.
///
/// Records exactly what was added and how much, enabling the pH model to
/// compute concentration-weighted chemistry rather than a naive average.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VesselContribution {
    pub reagent_id:      String,
    /// Volume dispensed in this event, in µL.
    pub volume_ul:       f64,
    /// Molar concentration of the reagent at the time of dispense (mol/L).
    pub concentration_m: f64,
}

// ── Reagent ────────────────────────────────────────────────────────────────────

/// A reagent lot registered in the laboratory inventory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reagent {
    /// Unique identifier (e.g., "reagent-hcl-001").
    pub id: String,
    /// Human-readable name (e.g., "Hydrochloric acid, conc.").
    pub name: String,
    /// CAS registry number, if known.
    pub cas_number: Option<String>,
    /// Lot number from the supplier.
    pub lot_number: String,
    /// Molar or weight concentration, if applicable.
    pub concentration: Option<f64>,
    /// Unit of concentration (e.g., "mol/L", "mg/mL").
    pub concentration_unit: Option<String>,
    /// Current volume in µL.
    pub volume_ul: f64,
    /// Expiry as Unix timestamp (seconds).  `None` means no expiry.
    pub expiry_secs: Option<u64>,
    /// GHS hazard codes (e.g., ["H290", "H314", "H335"]).
    #[serde(default)]
    pub ghs_hazard_codes: Vec<String>,
    /// Link to a `ReferenceMaterial` record, if this is a certified standard.
    pub reference_material_id: Option<String>,
    /// Nominal pH of the reagent in aqueous solution.  Used by the pH simulator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nominal_ph: Option<f64>,
    /// Molar concentration (mol/L) as prepared.  Used by the Henderson-Hasselbalch pH model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concentration_m: Option<f64>,
    /// Acid dissociation constant (pKa).  `Some` → participates in pH chemistry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pka: Option<f64>,
    /// True → use Henderson-Hasselbalch for pH contribution;
    /// false → treat as strong acid/base (pKa < 2 or > 12) or neutral.
    #[serde(default)]
    pub is_buffer: bool,
}

impl Reagent {
    /// True if the reagent has not expired relative to `now_secs`.
    pub fn is_valid_at(&self, now_secs: u64) -> bool {
        self.expiry_secs.map(|exp| now_secs < exp).unwrap_or(true)
    }
}

// ── LabState ───────────────────────────────────────────────────────────────────

/// Current state of the physical laboratory.
///
/// Tracks the reagent inventory and what has been dispensed into each vessel.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LabState {
    /// Reagent inventory: `reagent_id → Reagent`.
    pub reagents: HashMap<String, Reagent>,
    /// Vessel contents: `vessel_id → [VesselContribution, ...]` (insertion order preserved).
    ///
    /// Backward-compatible: on load from JSON, plain reagent-ID strings are
    /// converted to `VesselContribution { reagent_id, volume_ul: 0, concentration_m: 0 }`.
    #[serde(default, deserialize_with = "deser_vessel_contents")]
    pub vessel_contents: HashMap<String, Vec<VesselContribution>>,
}

// ── Backward-compatible vessel_contents deserializer ─────────────────────────

/// Accept both the old `Vec<String>` (reagent IDs) and the new
/// `Vec<VesselContribution>` formats when loading `lab_state.json`.
fn deser_vessel_contents<'de, D>(
    d: D,
) -> Result<HashMap<String, Vec<VesselContribution>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Entry {
        New(VesselContribution),
        Old(String),
    }

    let raw: HashMap<String, Vec<Entry>> = HashMap::deserialize(d)?;
    Ok(raw
        .into_iter()
        .map(|(vessel, items)| {
            let contribs = items
                .into_iter()
                .map(|e| match e {
                    Entry::New(c) => c,
                    Entry::Old(id) => VesselContribution {
                        reagent_id: id,
                        volume_ul: 0.0,
                        concentration_m: 0.0,
                    },
                })
                .collect();
            (vessel, contribs)
        })
        .collect())
}

impl LabState {
    /// Load from `.artifacts/lab_state.json`, or return an empty state if the
    /// file does not exist.
    pub fn load() -> Self {
        let path = Self::default_path();
        match std::fs::read_to_string(&path) {
            Ok(data) => serde_json::from_str(&data).unwrap_or_else(|e| {
                tracing::warn!(path = %path, error = %e, "lab_state.json invalid — starting fresh");
                Self::default()
            }),
            Err(_) => {
                tracing::info!(path = %path, "lab_state.json not found — starting with empty inventory");
                Self::default()
            }
        }
    }

    /// Persist to `.artifacts/lab_state.json`.
    pub fn save(&self) {
        let path = Self::default_path();
        if let Some(parent) = std::path::Path::new(&path).parent() {
            std::fs::create_dir_all(parent).ok();
        }
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    tracing::error!(path = %path, error = %e, "Failed to save lab_state.json");
                }
            }
            Err(e) => tracing::error!(error = %e, "Failed to serialize LabState"),
        }
    }

    fn default_path() -> String {
        std::env::var("AXIOMLAB_LAB_STATE_PATH")
            .unwrap_or_else(|_| ".artifacts/lab_state.json".into())
    }

    // ── Reagent mutations ─────────────────────────────────────────────────────

    /// Register or replace a reagent in the inventory.
    pub fn register_reagent(&mut self, reagent: Reagent) {
        self.reagents.insert(reagent.id.clone(), reagent);
    }

    /// Remove a reagent by ID.  Returns the removed reagent, or `None`.
    pub fn remove_reagent(&mut self, id: &str) -> Option<Reagent> {
        self.reagents.remove(id)
    }

    /// Deduct `volume_ul` from a reagent's remaining stock.
    ///
    /// Returns `Err` if the reagent does not exist or there is insufficient volume.
    pub fn deduct_volume(&mut self, reagent_id: &str, volume_ul: f64) -> Result<(), String> {
        let r = self.reagents.get_mut(reagent_id)
            .ok_or_else(|| format!("reagent '{reagent_id}' not in inventory"))?;
        if r.volume_ul < volume_ul {
            return Err(format!(
                "insufficient volume for '{reagent_id}': have {:.1} µL, need {volume_ul:.1} µL",
                r.volume_ul
            ));
        }
        r.volume_ul -= volume_ul;
        Ok(())
    }

    // ── Vessel mutations ──────────────────────────────────────────────────────

    /// Record that `volume_ul` of `reagent_id` was dispensed into `vessel_id`.
    ///
    /// Looks up `Reagent.concentration_m` from the inventory; uses `0.0` if
    /// the reagent is not found or has no concentration.
    pub fn add_to_vessel(&mut self, vessel_id: &str, reagent_id: &str, volume_ul: f64) {
        let concentration_m = self.reagents
            .get(reagent_id)
            .and_then(|r| r.concentration_m)
            .unwrap_or(0.0);
        self.vessel_contents
            .entry(vessel_id.into())
            .or_default()
            .push(VesselContribution { reagent_id: reagent_id.into(), volume_ul, concentration_m });
    }

    /// Remove `volume_ul` of `reagent_id` from vessel contents (FIFO order).
    ///
    /// Older contributions of the same reagent are drained first.  A
    /// contribution is removed entirely when its remaining volume reaches zero.
    pub fn remove_from_vessel(&mut self, vessel_id: &str, reagent_id: &str, volume_ul: f64) {
        let Some(contribs) = self.vessel_contents.get_mut(vessel_id) else { return };
        let mut remaining = volume_ul;
        contribs.retain_mut(|c| {
            if c.reagent_id != reagent_id || remaining <= 0.0 {
                return true;
            }
            if c.volume_ul <= remaining {
                remaining -= c.volume_ul;
                false
            } else {
                c.volume_ul -= remaining;
                remaining = 0.0;
                true
            }
        });
    }

    /// Return the reagent names (not IDs) currently in a vessel,
    /// suitable for chemical compatibility checking.
    pub fn vessel_reagent_names(&self, vessel_id: &str) -> Vec<String> {
        self.vessel_contents
            .get(vessel_id)
            .map(|contribs| {
                contribs.iter()
                    .filter_map(|c| self.reagents.get(&c.reagent_id))
                    .map(|r| r.name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Set vessel contents directly (for the PUT /api/lab/vessels/{id}/contents route).
    pub fn set_vessel_contents(&mut self, vessel_id: &str, contributions: Vec<VesselContribution>) {
        self.vessel_contents.insert(vessel_id.into(), contributions);
    }

    /// Return all reagent IDs currently recorded in a vessel.
    pub fn vessel_reagent_ids(&self, vessel_id: &str) -> Vec<&str> {
        self.vessel_contents
            .get(vessel_id)
            .map(|v| v.iter().map(|c| c.reagent_id.as_str()).collect())
            .unwrap_or_default()
    }

    // ── Queries ───────────────────────────────────────────────────────────────

    /// Return all reagents that have expired as of `now_secs`.
    pub fn expired_reagents(&self, now_secs: u64) -> Vec<&Reagent> {
        self.reagents.values()
            .filter(|r| !r.is_valid_at(now_secs))
            .collect()
    }

    /// Return all reagents that will expire within `warn_secs` seconds.
    pub fn expiring_soon(&self, now_secs: u64, warn_secs: u64) -> Vec<&Reagent> {
        self.reagents.values()
            .filter(|r| {
                if let Some(exp) = r.expiry_secs {
                    exp > now_secs && exp - now_secs <= warn_secs
                } else {
                    false
                }
            })
            .collect()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_reagent(id: &str, name: &str, volume_ul: f64) -> Reagent {
        Reagent {
            id: id.into(),
            name: name.into(),
            cas_number: None,
            lot_number: "L001".into(),
            concentration: None,
            concentration_unit: None,
            volume_ul,
            expiry_secs: None,
            ghs_hazard_codes: vec![],
            reference_material_id: None,
            nominal_ph: None,
            concentration_m: None,
            pka: None,
            is_buffer: false,
        }
    }

    #[test]
    fn register_and_deduct() {
        let mut state = LabState::default();
        state.register_reagent(sample_reagent("r1", "HCl", 500.0));
        assert!(state.deduct_volume("r1", 100.0).is_ok());
        assert_eq!(state.reagents["r1"].volume_ul, 400.0);
    }

    #[test]
    fn deduct_insufficient_volume() {
        let mut state = LabState::default();
        state.register_reagent(sample_reagent("r1", "HCl", 50.0));
        assert!(state.deduct_volume("r1", 100.0).is_err());
    }

    #[test]
    fn vessel_contents_tracking() {
        let mut state = LabState::default();
        state.register_reagent(sample_reagent("r1", "NaOH", 200.0));
        state.add_to_vessel("vessel-1", "r1", 100.0);
        assert_eq!(state.vessel_reagent_names("vessel-1"), vec!["NaOH"]);
        state.remove_from_vessel("vessel-1", "r1", 100.0);
        assert!(state.vessel_reagent_names("vessel-1").is_empty());
    }

    #[test]
    fn vessel_contribution_records_volume_and_concentration() {
        let mut state = LabState::default();
        let mut r = sample_reagent("r1", "HCl", 500.0);
        r.concentration_m = Some(0.1);
        state.register_reagent(r);
        state.add_to_vessel("v1", "r1", 200.0);
        let c = &state.vessel_contents["v1"][0];
        assert_eq!(c.reagent_id, "r1");
        assert!((c.volume_ul - 200.0).abs() < 1e-10);
        assert!((c.concentration_m - 0.1).abs() < 1e-10);
    }

    #[test]
    fn remove_from_vessel_partial_volume() {
        let mut state = LabState::default();
        state.register_reagent(sample_reagent("r1", "HCl", 500.0));
        state.add_to_vessel("v1", "r1", 200.0);
        state.remove_from_vessel("v1", "r1", 80.0);
        let c = &state.vessel_contents["v1"][0];
        assert!((c.volume_ul - 120.0).abs() < 1e-10);
    }

    #[test]
    fn vessel_backward_compat_deser() {
        // Old format: array of strings.
        let json = r#"{"vessel_contents":{"v1":["r1","r2"]},"reagents":{}}"#;
        let state: LabState = serde_json::from_str(json).unwrap();
        let v = &state.vessel_contents["v1"];
        assert_eq!(v[0].reagent_id, "r1");
        assert_eq!(v[0].volume_ul, 0.0);
        assert_eq!(v[1].reagent_id, "r2");
    }

    #[test]
    fn expiry_checking() {
        let mut state = LabState::default();
        let mut r = sample_reagent("r1", "Buffer", 100.0);
        r.expiry_secs = Some(1000);
        state.register_reagent(r);
        assert!(state.reagents["r1"].is_valid_at(999));
        assert!(!state.reagents["r1"].is_valid_at(1001));
        assert_eq!(state.expired_reagents(1001).len(), 1);
    }

    #[test]
    fn unknown_reagent_deduct_errors() {
        let mut state = LabState::default();
        assert!(state.deduct_volume("nonexistent", 10.0).is_err());
    }
}
