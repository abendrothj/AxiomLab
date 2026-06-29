//! Laboratory state — reagent inventory and vessel contents.
//!
//! `LabState` tracks what reagents are in stock and what has been dispensed into
//! each vessel. It is the physical-world model the gates consult (chemistry
//! compatibility, volume availability). Persisted as JSON; the audit chain
//! remains the authoritative record of *actions*, while this is current *state*.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One dispensing event tracked in a vessel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VesselContribution {
    pub reagent_id: String,
    /// Volume dispensed in this event, in µL.
    pub volume_ul: f64,
    /// Molar concentration of the reagent at dispense time (mol/L).
    pub concentration_m: f64,
}

/// A reagent lot registered in the laboratory inventory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reagent {
    pub id: String,
    pub name: String,
    pub cas_number: Option<String>,
    pub lot_number: String,
    pub concentration: Option<f64>,
    pub concentration_unit: Option<String>,
    /// Current volume in µL.
    pub volume_ul: f64,
    /// Expiry as Unix timestamp (seconds). `None` means no expiry.
    pub expiry_secs: Option<u64>,
    #[serde(default)]
    pub ghs_hazard_codes: Vec<String>,
    pub reference_material_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nominal_ph: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concentration_m: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pka: Option<f64>,
    #[serde(default)]
    pub is_buffer: bool,
}

impl Reagent {
    /// True if the reagent has not expired relative to `now_secs`.
    pub fn is_valid_at(&self, now_secs: u64) -> bool {
        self.expiry_secs.map(|exp| now_secs < exp).unwrap_or(true)
    }
}

/// Current state of the physical laboratory.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LabState {
    /// Reagent inventory: `reagent_id → Reagent`.
    pub reagents: HashMap<String, Reagent>,
    /// Vessel contents: `vessel_id → [VesselContribution, ...]` (insertion order preserved).
    #[serde(default, deserialize_with = "deser_vessel_contents")]
    pub vessel_contents: HashMap<String, Vec<VesselContribution>>,
    /// Vessel capacities in µL: `vessel_id → max_volume_ul`. Consulted by the
    /// `ProofGate`'s verified cumulative-capacity check.
    #[serde(default)]
    pub vessel_capacities: HashMap<String, f64>,
}

/// Default vessel capacities (µL). Mirrors the simulator's vessel set so the
/// cumulative-capacity check is active out of the box; override per deployment.
pub const DEFAULT_VESSEL_CAPACITIES: &[(&str, f64)] = &[
    ("beaker_A", 50_000.0),
    ("beaker_B", 50_000.0),
    ("tube_1", 2_000.0),
    ("tube_2", 2_000.0),
    ("tube_3", 2_000.0),
    ("plate_well_A1", 300.0),
    ("plate_well_B1", 300.0),
    ("reservoir", 200_000.0),
];

/// Accept both the legacy `Vec<String>` and the current `Vec<VesselContribution>`
/// formats when loading `lab_state.json`.
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
    /// Load from `AXIOMLAB_LAB_STATE_PATH` (default `.artifacts/lab_state.json`),
    /// or return an empty state if the file is missing or invalid.
    pub fn load() -> Self {
        let path = Self::default_path();
        match std::fs::read_to_string(&path) {
            Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Persist to the configured path.
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::default_path();
        if let Some(parent) = std::path::Path::new(&path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::other(format!("serialize LabState: {e}")))?;
        std::fs::write(&path, json)
    }

    fn default_path() -> String {
        std::env::var("AXIOMLAB_LAB_STATE_PATH")
            .unwrap_or_else(|_| ".artifacts/lab_state.json".into())
    }

    // ── Reagent mutations ──

    pub fn register_reagent(&mut self, reagent: Reagent) {
        self.reagents.insert(reagent.id.clone(), reagent);
    }

    pub fn remove_reagent(&mut self, id: &str) -> Option<Reagent> {
        self.reagents.remove(id)
    }

    /// Deduct `volume_ul` from a reagent's remaining stock.
    pub fn deduct_volume(&mut self, reagent_id: &str, volume_ul: f64) -> Result<(), String> {
        let r = self
            .reagents
            .get_mut(reagent_id)
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

    // ── Vessel mutations ──

    /// Record that `volume_ul` of `reagent_id` was dispensed into `vessel_id`.
    pub fn add_to_vessel(&mut self, vessel_id: &str, reagent_id: &str, volume_ul: f64) {
        let concentration_m = self
            .reagents
            .get(reagent_id)
            .and_then(|r| r.concentration_m)
            .unwrap_or(0.0);
        self.vessel_contents
            .entry(vessel_id.into())
            .or_default()
            .push(VesselContribution {
                reagent_id: reagent_id.into(),
                volume_ul,
                concentration_m,
            });
    }

    /// Remove `volume_ul` of `reagent_id` from a vessel (FIFO).
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

    pub fn set_vessel_contents(&mut self, vessel_id: &str, contributions: Vec<VesselContribution>) {
        self.vessel_contents.insert(vessel_id.into(), contributions);
    }

    /// Reagent *names* currently in a vessel, for chemical compatibility checks.
    pub fn vessel_reagent_names(&self, vessel_id: &str) -> Vec<String> {
        self.vessel_contents
            .get(vessel_id)
            .map(|contribs| {
                contribs
                    .iter()
                    .filter_map(|c| self.reagents.get(&c.reagent_id))
                    .map(|r| r.name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Reagent IDs currently in a vessel.
    pub fn vessel_reagent_ids(&self, vessel_id: &str) -> Vec<&str> {
        self.vessel_contents
            .get(vessel_id)
            .map(|v| v.iter().map(|c| c.reagent_id.as_str()).collect())
            .unwrap_or_default()
    }

    /// Contributions currently in a vessel.
    pub fn vessel_contents_of(&self, vessel_id: &str) -> &[VesselContribution] {
        self.vessel_contents.get(vessel_id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Total tracked volume in a vessel (µL) — the sum of its contributions.
    pub fn vessel_volume(&self, vessel_id: &str) -> f64 {
        self.vessel_contents
            .get(vessel_id)
            .map(|cs| cs.iter().map(|c| c.volume_ul).sum())
            .unwrap_or(0.0)
    }

    /// The configured capacity of a vessel (µL), if known.
    pub fn vessel_capacity(&self, vessel_id: &str) -> Option<f64> {
        self.vessel_capacities.get(vessel_id).copied()
    }

    pub fn set_vessel_capacity(&mut self, vessel_id: &str, capacity_ul: f64) {
        self.vessel_capacities.insert(vessel_id.into(), capacity_ul);
    }

    /// Seed [`DEFAULT_VESSEL_CAPACITIES`] for any vessel not already configured.
    pub fn seed_default_vessels(&mut self) {
        for (id, cap) in DEFAULT_VESSEL_CAPACITIES {
            self.vessel_capacities.entry((*id).into()).or_insert(*cap);
        }
    }

    // ── Queries ──

    /// The set of registered reference-material IDs — the certified standards a
    /// calibration's x-axis must be drawn from. Sourced from reagents that
    /// declare a `reference_material_id`.
    pub fn registered_reference_materials(&self) -> std::collections::HashSet<String> {
        self.reagents.values().filter_map(|r| r.reference_material_id.clone()).collect()
    }

    pub fn expired_reagents(&self, now_secs: u64) -> Vec<&Reagent> {
        self.reagents.values().filter(|r| !r.is_valid_at(now_secs)).collect()
    }

    pub fn expiring_soon(&self, now_secs: u64, warn_secs: u64) -> Vec<&Reagent> {
        self.reagents
            .values()
            .filter(|r| {
                r.expiry_secs
                    .map(|exp| exp > now_secs && exp - now_secs <= warn_secs)
                    .unwrap_or(false)
            })
            .collect()
    }
}

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
        assert!(state.deduct_volume("r1", 9999.0).is_err());
    }

    #[test]
    fn vessel_tracking_and_removal() {
        let mut state = LabState::default();
        state.register_reagent(sample_reagent("r1", "NaOH", 200.0));
        state.add_to_vessel("v1", "r1", 100.0);
        assert_eq!(state.vessel_reagent_names("v1"), vec!["NaOH"]);
        state.remove_from_vessel("v1", "r1", 100.0);
        assert!(state.vessel_reagent_names("v1").is_empty());
    }

    #[test]
    fn backward_compat_deser() {
        let json = r#"{"vessel_contents":{"v1":["r1","r2"]},"reagents":{}}"#;
        let state: LabState = serde_json::from_str(json).unwrap();
        assert_eq!(state.vessel_contents["v1"][0].reagent_id, "r1");
        assert_eq!(state.vessel_contents["v1"][1].reagent_id, "r2");
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
}
