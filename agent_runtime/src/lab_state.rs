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
    /// Vessel contents: `vessel_id → [reagent_id, ...]` (insertion order preserved).
    pub vessel_contents: HashMap<String, Vec<String>>,
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

    /// Record that `reagent_id` was dispensed into `vessel_id`.
    pub fn add_to_vessel(&mut self, vessel_id: &str, reagent_id: &str) {
        self.vessel_contents
            .entry(vessel_id.into())
            .or_default()
            .push(reagent_id.into());
    }

    /// Remove a reagent from vessel contents (e.g., after aspiration).
    pub fn remove_from_vessel(&mut self, vessel_id: &str, reagent_id: &str) {
        if let Some(contents) = self.vessel_contents.get_mut(vessel_id) {
            if let Some(pos) = contents.iter().position(|r| r == reagent_id) {
                contents.remove(pos);
            }
        }
    }

    /// Return the reagent names (not IDs) currently in a vessel,
    /// suitable for chemical compatibility checking.
    pub fn vessel_reagent_names(&self, vessel_id: &str) -> Vec<String> {
        self.vessel_contents
            .get(vessel_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.reagents.get(id))
                    .map(|r| r.name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Set vessel contents directly (for the PUT /api/lab/vessels/{id}/contents route).
    pub fn set_vessel_contents(&mut self, vessel_id: &str, reagent_ids: Vec<String>) {
        self.vessel_contents.insert(vessel_id.into(), reagent_ids);
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
        state.add_to_vessel("vessel-1", "r1");
        assert_eq!(state.vessel_reagent_names("vessel-1"), vec!["NaOH"]);
        state.remove_from_vessel("vessel-1", "r1");
        assert!(state.vessel_reagent_names("vessel-1").is_empty());
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
