//! Chemical compatibility checker — Stage 0.25 of the tool dispatch pipeline.
//!
//! Prevents the agent from dispensing reagents that are known to be incompatible
//! with each other.  The incompatibility table is embedded at compile-time from
//! `chemistry_table.json` (GHS / NFPA 704 sources).
//!
//! # Graceful degradation
//! Stage 0.25 is a no-op when `LabState` vessel contents are not available
//! (i.e. before Phase 2B is initialised).  It then only checks the two
//! explicit reagent names passed as tool parameters.

use serde::Deserialize;
use std::collections::{HashMap, HashSet};

// ── Static table ──────────────────────────────────────────────────────────────

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

// ── Public API ────────────────────────────────────────────────────────────────

/// Loaded chemical compatibility matrix.
///
/// Maps normalised reagent names to the set of reagents they are incompatible
/// with, along with the reason.  Keys and values are lower-cased for
/// case-insensitive matching.
pub struct ChemicalCompatibility {
    /// `name → [(other_name, reason)]`
    index: HashMap<String, Vec<(String, String)>>,
}

impl ChemicalCompatibility {
    /// Load from the bundled `chemistry_table.json`.
    ///
    /// This is cheap (< 1 µs) — the JSON is already compiled into the binary
    /// and only needs to be deserialized once at server startup.
    pub fn from_bundled() -> Self {
        let raw: RawTable = serde_json::from_str(TABLE_JSON)
            .expect("chemistry_table.json is invalid — this is a compile-time error");
        let mut index: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for pair in raw.incompatible_pairs {
            let a = pair.a.to_lowercase();
            let b = pair.b.to_lowercase();
            let r = pair.reason.clone();
            index.entry(a.clone()).or_default().push((b.clone(), r.clone()));
            index.entry(b).or_default().push((a, r));
        }
        Self { index }
    }

    /// Check whether `reagent_a` and `reagent_b` are compatible.
    ///
    /// Matching is case-insensitive.  An exact match on either the full name or
    /// a space/comma-delimited token within a compound identifier (e.g.
    /// `"conc. HCl"` matches the table entry `"hcl"`).
    ///
    /// Returns `Ok(())` if compatible or unknown, `Err(reason)` if incompatible.
    pub fn check(&self, reagent_a: &str, reagent_b: &str) -> Result<(), String> {
        let a = reagent_a.to_lowercase();
        let b = reagent_b.to_lowercase();
        // Tokenise on common separators so "conc. HCl" → {"conc.", "hcl"}.
        let tokens_a: Vec<&str> = a.split([' ', ',', '.', '/', '-']).filter(|t| !t.is_empty()).collect();
        let tokens_b: Vec<&str> = b.split([' ', ',', '.', '/', '-']).filter(|t| !t.is_empty()).collect();

        // Check every token of `a` against every key in the index.
        for tok_a in &tokens_a {
            if let Some(conflicts) = self.index.get(*tok_a) {
                for (other, reason) in conflicts {
                    if tokens_b.contains(&other.as_str()) || b == *other {
                        return Err(format!(
                            "chemical incompatibility: '{reagent_a}' + '{reagent_b}' — {reason}"
                        ));
                    }
                }
            }
        }
        // Check every token of `b` against the index (symmetric, but tokens differ).
        for tok_b in &tokens_b {
            if let Some(conflicts) = self.index.get(*tok_b) {
                for (other, reason) in conflicts {
                    if tokens_a.contains(&other.as_str()) || a == *other {
                        return Err(format!(
                            "chemical incompatibility: '{reagent_b}' + '{reagent_a}' — {reason}"
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Check whether adding `adding` to a vessel already containing
    /// `vessel_contents` would create an incompatible mixture.
    ///
    /// Returns `Err(reason)` on the first conflict found.
    pub fn check_vessel_addition(
        &self,
        vessel_contents: &[String],
        adding: &str,
    ) -> Result<(), String> {
        for existing in vessel_contents {
            self.check(existing, adding)?;
        }
        Ok(())
    }

    /// Return a snapshot of all known reagent names (for tooling / tests).
    pub fn known_reagents(&self) -> HashSet<&str> {
        self.index.keys().map(String::as_str).collect()
    }
}

// ── Singleton helper ──────────────────────────────────────────────────────────

use std::sync::OnceLock;

static COMPAT: OnceLock<ChemicalCompatibility> = OnceLock::new();

/// Return a reference to the global (lazily-initialised) compatibility matrix.
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
    fn incompatible_acid_base_detected() {
        let c = cc();
        assert!(c.check("HCl", "NaOH").is_err());
    }

    #[test]
    fn cyanide_acid_detected() {
        let c = cc();
        assert!(c.check("NaCN", "HCl").is_err());
        assert!(c.check("HCl", "NaCN").is_err()); // symmetric
    }

    #[test]
    fn compatible_pair_allowed() {
        let c = cc();
        // NaCl + H2O — neither is in the table as incompatible
        assert!(c.check("NaCl", "H2O").is_ok());
    }

    #[test]
    fn case_insensitive_matching() {
        let c = cc();
        assert!(c.check("hcl", "naoh").is_err());
        assert!(c.check("H2SO4", "NAOH").is_err());
    }

    #[test]
    fn vessel_addition_blocked_on_conflict() {
        let c = cc();
        let vessel = vec!["HCl".to_string(), "NaCl".to_string()];
        // Adding NaOH to a vessel that already contains HCl → conflict
        assert!(c.check_vessel_addition(&vessel, "NaOH").is_err());
    }

    #[test]
    fn vessel_addition_allowed_when_no_conflict() {
        let c = cc();
        let vessel = vec!["NaCl".to_string()];
        assert!(c.check_vessel_addition(&vessel, "glucose").is_ok());
    }

    #[test]
    fn symmetric_index() {
        let c = cc();
        // Every entry should be symmetric: a→b implies b→a
        assert!(c.check("KMnO4", "ethanol").is_err());
        assert!(c.check("ethanol", "KMnO4").is_err());
    }
}
