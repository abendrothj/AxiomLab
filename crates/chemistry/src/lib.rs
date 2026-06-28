//! Reagent compatibility — the `ChemistryGate`'s knowledge base.
//!
//! The incompatibility table is embedded at compile time from
//! `chemistry_table.json` (GHS / NFPA 704 sources). The gate resolves vessel
//! contents to reagent *names* (via `LabState`) and asks whether adding a new
//! reagent would create a [`HazardLevel::Dangerous`] mixture.

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

static TABLE_JSON: &str = include_str!("chemistry_table.json");

#[derive(Deserialize)]
struct RawTable {
    incompatible_pairs: Vec<RawPair>,
}

#[derive(Deserialize)]
struct RawPair {
    a: String,
    b: String,
    reason: String,
}

/// The hazard verdict for combining two reagents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HazardLevel {
    /// No known incompatibility (or one/both reagents are unknown).
    Safe,
    /// A known dangerous combination — the carried string is the reason.
    Dangerous(String),
}

impl HazardLevel {
    pub fn is_dangerous(&self) -> bool {
        matches!(self, HazardLevel::Dangerous(_))
    }
    pub fn reason(&self) -> Option<&str> {
        match self {
            HazardLevel::Dangerous(r) => Some(r),
            HazardLevel::Safe => None,
        }
    }
}

/// The loaded chemical compatibility matrix (`name → [(other_name, reason)]`),
/// lower-cased for case-insensitive matching.
pub struct ChemicalCompatibility {
    index: HashMap<String, Vec<(String, String)>>,
}

impl ChemicalCompatibility {
    /// Load from the bundled `chemistry_table.json`.
    pub fn from_bundled() -> Self {
        let raw: RawTable = serde_json::from_str(TABLE_JSON)
            .expect("chemistry_table.json is invalid — compile-time error");
        let mut index: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for pair in raw.incompatible_pairs {
            let a = pair.a.to_lowercase();
            let b = pair.b.to_lowercase();
            index.entry(a.clone()).or_default().push((b.clone(), pair.reason.clone()));
            index.entry(b).or_default().push((a, pair.reason));
        }
        Self { index }
    }

    /// Verdict for combining two reagents by name. Matching is case-insensitive
    /// and token-aware, so `"conc. HCl"` matches the table entry `"hcl"`.
    pub fn check(&self, reagent_a: &str, reagent_b: &str) -> HazardLevel {
        let a = reagent_a.to_lowercase();
        let b = reagent_b.to_lowercase();
        let tokens_a = tokenize(&a);
        let tokens_b = tokenize(&b);

        for tok_a in &tokens_a {
            if let Some(conflicts) = self.index.get(*tok_a) {
                for (other, reason) in conflicts {
                    if tokens_b.contains(&other.as_str()) || b == *other {
                        return HazardLevel::Dangerous(format!(
                            "chemical incompatibility: '{reagent_a}' + '{reagent_b}' — {reason}"
                        ));
                    }
                }
            }
        }
        for tok_b in &tokens_b {
            if let Some(conflicts) = self.index.get(*tok_b) {
                for (other, reason) in conflicts {
                    if tokens_a.contains(&other.as_str()) || a == *other {
                        return HazardLevel::Dangerous(format!(
                            "chemical incompatibility: '{reagent_b}' + '{reagent_a}' — {reason}"
                        ));
                    }
                }
            }
        }
        HazardLevel::Safe
    }

    /// Verdict for adding `adding` to a vessel that already holds `existing`
    /// (reagent names). Returns the first dangerous pairing found.
    pub fn check_addition(&self, existing: &[String], adding: &str) -> HazardLevel {
        for name in existing {
            let verdict = self.check(name, adding);
            if verdict.is_dangerous() {
                return verdict;
            }
        }
        HazardLevel::Safe
    }
}

fn tokenize(s: &str) -> Vec<&str> {
    s.split([' ', ',', '.', '/', '-']).filter(|t| !t.is_empty()).collect()
}

static COMPAT: OnceLock<ChemicalCompatibility> = OnceLock::new();

/// Reference to the lazily-initialised global compatibility matrix.
pub fn global() -> &'static ChemicalCompatibility {
    COMPAT.get_or_init(ChemicalCompatibility::from_bundled)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cc() -> ChemicalCompatibility {
        ChemicalCompatibility::from_bundled()
    }

    #[test]
    fn acid_base_is_dangerous() {
        assert!(cc().check("HCl", "NaOH").is_dangerous());
    }

    #[test]
    fn cyanide_acid_symmetric() {
        let c = cc();
        assert!(c.check("NaCN", "HCl").is_dangerous());
        assert!(c.check("HCl", "NaCN").is_dangerous());
    }

    #[test]
    fn compatible_pair_safe() {
        assert_eq!(cc().check("NaCl", "H2O"), HazardLevel::Safe);
    }

    #[test]
    fn case_insensitive() {
        assert!(cc().check("hcl", "naoh").is_dangerous());
        assert!(cc().check("H2SO4", "NAOH").is_dangerous());
    }

    #[test]
    fn token_aware_matching() {
        assert!(cc().check("conc. HCl", "NaOH").is_dangerous());
    }

    #[test]
    fn vessel_addition() {
        let c = cc();
        let existing = vec!["HCl".to_string(), "NaCl".to_string()];
        assert!(c.check_addition(&existing, "NaOH").is_dangerous());
        assert_eq!(c.check_addition(&existing, "glucose"), HazardLevel::Safe);
    }

    #[test]
    fn reason_is_populated() {
        let v = cc().check("KMnO4", "glycerol");
        assert!(v.reason().unwrap().contains("ignition"));
    }
}
