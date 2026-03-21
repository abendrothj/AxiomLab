//! Henderson-Hasselbalch pH model for vessel contents.
//!
//! # Classification of reagents
//! | Condition                          | Treatment                              |
//! |------------------------------------|----------------------------------------|
//! | `pka.is_none()`                    | Neutral — no H⁺ contribution           |
//! | `!is_buffer && pka < 2.0`          | Strong acid — fully dissociates        |
//! | `!is_buffer && pka > 12.0`         | Strong base — fully dissociates        |
//! | `is_buffer && 2.0 ≤ pka ≤ 12.0`   | Weak acid — quadratic approximation   |
//!
//! # Algorithm
//! 1. Accumulate Δ[H⁺] (mol/L) from each classified species in the total volume.
//! 2. Solve the net-proton-balance with water autoionisation:
//!    `[H⁺] = Δ/2 + √((Δ/2)² + Kw)` for acidic Δ; symmetric for basic Δ.
//! 3. Return `−log₁₀([H⁺])` clamped to [0, 14].
//! 4. If no pH-active species are present, return 7.0 (neutral).

use std::collections::HashMap;

use crate::lab_state::{Reagent, VesselContribution};

const KW: f64 = 1e-14; // water auto-ionisation constant at 25 °C

/// Compute the pH of a vessel from its contribution list and reagent registry.
///
/// Returns `7.0` when the vessel is empty, contains zero volume, or every
/// reagent has no `pka` (neutral species).  Result is clamped to `[0.0, 14.0]`.
pub fn compute_ph(
    contributions: &[VesselContribution],
    reagents: &HashMap<String, Reagent>,
) -> f64 {
    if contributions.is_empty() {
        return 7.0;
    }

    // Total solution volume in litres.
    let total_vol_l: f64 = contributions.iter().map(|c| c.volume_ul / 1_000_000.0).sum();
    if total_vol_l < 1e-15 {
        return 7.0;
    }

    // Δ[H⁺] in mol/L: positive = net acid, negative = net base.
    let mut delta_h  = 0.0_f64;
    let mut has_active = false;

    for c in contributions {
        let Some(r) = reagents.get(&c.reagent_id) else { continue };
        let Some(pka) = r.pka else { continue };

        let moles = c.concentration_m * (c.volume_ul / 1_000_000.0);
        if moles < 1e-30 { continue; }

        // Concentration in the *total* vessel volume.
        let conc = moles / total_vol_l;
        has_active = true;

        if !r.is_buffer && pka < 2.0 {
            // Strong acid: all protons released.
            delta_h += conc;
        } else if !r.is_buffer && pka > 12.0 {
            // Strong base: all protons consumed.
            delta_h -= conc;
        } else if r.is_buffer {
            // Weak acid (Henderson-Hasselbalch / quadratic approximation):
            //   [H⁺]² + Ka·[H⁺] − Ka·C = 0
            //   [H⁺] = (−Ka + √(Ka² + 4·Ka·C)) / 2
            let ka = 10f64.powf(-pka);
            let h  = (-ka + (ka * ka + 4.0 * ka * conc).sqrt()) / 2.0;
            delta_h += h;
        }
        // Neutral (no pKa) → no contribution.
    }

    if !has_active {
        return 7.0;
    }

    // Net proton balance with water autoionisation.
    //   Acidic: [H⁺] = Δ/2 + √((Δ/2)² + Kw)
    //   Basic:  [OH⁻] = |Δ|/2 + √((|Δ|/2)² + Kw)  →  [H⁺] = Kw / [OH⁻]
    let h_conc = if delta_h >= 0.0 {
        let half = delta_h / 2.0;
        half + (half * half + KW).sqrt()
    } else {
        let half = -delta_h / 2.0;
        let oh   = half + (half * half + KW).sqrt();
        KW / oh
    };

    (-h_conc.log10()).clamp(0.0, 14.0)
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn reagent(id: &str, pka: Option<f64>, is_buffer: bool) -> (String, Reagent) {
        (
            id.to_string(),
            Reagent {
                id:               id.into(),
                name:             id.into(),
                cas_number:       None,
                lot_number:       "L001".into(),
                concentration:    None,
                concentration_unit: None,
                volume_ul:        10_000.0,
                expiry_secs:      None,
                ghs_hazard_codes: vec![],
                reference_material_id: None,
                nominal_ph:       None,
                concentration_m:  None,
                pka,
                is_buffer,
            },
        )
    }

    fn contribution(reagent_id: &str, volume_ul: f64, conc_m: f64) -> VesselContribution {
        VesselContribution { reagent_id: reagent_id.into(), volume_ul, concentration_m: conc_m }
    }

    // ── Neutral / edge cases ───────────────────────────────────────────────────

    #[test]
    fn empty_vessel_returns_neutral() {
        let map: HashMap<String, Reagent> = HashMap::new();
        assert_eq!(compute_ph(&[], &map), 7.0);
    }

    #[test]
    fn all_neutral_reagents_return_7() {
        let map: HashMap<String, Reagent> = [reagent("water", None, false)].into();
        let c = [contribution("water", 1_000_000.0, 0.01)];
        assert_eq!(compute_ph(&c, &map), 7.0);
    }

    // ── Strong acid ────────────────────────────────────────────────────────────

    /// 0.01 mol/L HCl → pH = 2.0
    #[test]
    fn strong_acid_01m_gives_ph2() {
        let map: HashMap<String, Reagent> = [reagent("hcl", Some(-1.0), false)].into();
        // 10 mL of 0.01 M HCl → total vol = 0.01 L, moles = 1e-4 → [H+]=0.01
        let c = [contribution("hcl", 10_000.0, 0.01)];
        let ph = compute_ph(&c, &map);
        assert!((ph - 2.0).abs() < 0.01, "pH = {ph:.3}, expected 2.0");
    }

    /// 0.001 mol/L HCl → pH ≈ 3.0
    #[test]
    fn strong_acid_dilution() {
        let map: HashMap<String, Reagent> = [reagent("hcl", Some(-1.0), false)].into();
        let c = [contribution("hcl", 10_000.0, 0.001)];
        let ph = compute_ph(&c, &map);
        assert!((ph - 3.0).abs() < 0.01, "pH = {ph:.3}, expected 3.0");
    }

    // ── Strong base ────────────────────────────────────────────────────────────

    /// 0.01 mol/L NaOH → pH = 12.0
    #[test]
    fn strong_base_01m_gives_ph12() {
        let map: HashMap<String, Reagent> = [reagent("naoh", Some(14.5), false)].into();
        let c = [contribution("naoh", 10_000.0, 0.01)];
        let ph = compute_ph(&c, &map);
        assert!((ph - 12.0).abs() < 0.01, "pH = {ph:.3}, expected 12.0");
    }

    // ── Weak acid (buffer) ─────────────────────────────────────────────────────

    /// 0.1 mol/L acetic acid (pKa=4.76) → pH ≈ 2.88
    ///
    /// Standard result: pH = ½(pKa − log C) = ½(4.76 + 1) = 2.88
    #[test]
    fn weak_acid_01m_acetate() {
        let map: HashMap<String, Reagent> = [reagent("acoh", Some(4.76), true)].into();
        let c = [contribution("acoh", 1_000_000.0, 0.1)];
        let ph = compute_ph(&c, &map);
        // Exact quadratic gives pH ≈ 2.875; accept ±0.05
        assert!((ph - 2.88).abs() < 0.05, "pH = {ph:.3}, expected ≈ 2.88");
    }

    // ── Mixtures ──────────────────────────────────────────────────────────────

    /// Strong acid + buffer → lower pH than pure buffer.
    #[test]
    fn acid_plus_buffer_mixture() {
        let map: HashMap<String, Reagent> = [
            reagent("hcl",  Some(-1.0), false),
            reagent("acoh", Some(4.76), true),
        ].into();
        // 0.001 M HCl and 0.1 M acetic acid in equal volumes
        let c = [
            contribution("hcl",  500_000.0, 0.001),
            contribution("acoh", 500_000.0, 0.1),
        ];
        let ph_mix    = compute_ph(&c, &map);
        let c_buf_only = [contribution("acoh", 1_000_000.0, 0.1)];
        let ph_buf_only = compute_ph(&c_buf_only, &map);
        assert!(ph_mix < ph_buf_only, "adding HCl must lower pH");
        assert!(ph_mix < 7.0, "mixture must be acidic");
    }

    /// Acid + base neutralisation moves pH toward neutral.
    #[test]
    fn strong_acid_base_neutralisation() {
        let map: HashMap<String, Reagent> = [
            reagent("hcl",  Some(-1.0), false),
            reagent("naoh", Some(14.5), false),
        ].into();
        // Equal moles (0.01 M each, equal volume) → near neutral
        let c = [
            contribution("hcl",  1_000_000.0, 0.01),
            contribution("naoh", 1_000_000.0, 0.01),
        ];
        let ph = compute_ph(&c, &map);
        assert!((ph - 7.0).abs() < 0.1, "equimolar acid+base → neutral, got pH={ph:.2}");
    }

    /// pH clamped to [0, 14].
    #[test]
    fn ph_clamped_to_range() {
        let map: HashMap<String, Reagent> = [reagent("hcl", Some(-1.0), false)].into();
        // Extreme: 12 M HCl (battery acid) — far beyond physical pH scale
        let c = [contribution("hcl", 1_000_000.0, 12.0)];
        let ph = compute_ph(&c, &map);
        assert!(ph >= 0.0 && ph <= 14.0, "pH must be clamped, got {ph}");
    }
}
