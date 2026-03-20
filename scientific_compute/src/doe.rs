//! Design of Experiments (DoE) — generates structured run matrices.
//!
//! Three designs are provided:
//! - `full_factorial`: 2^k design with all low/high combinations.
//! - `central_composite`: Face-centered CCD for response-surface models.
//! - `latin_hypercube`: Space-filling LHC for screening/optimization.
//!
//! The generated [`DoeDesign`] is serialized to JSON and returned as a tool
//! result so the LLM can use the run matrix to propose a [`Protocol`].
//!
//! # Constraints
//! - `full_factorial`: k ≤ 5 (max 32 runs)
//! - `central_composite`: 2 ≤ k ≤ 4
//! - `latin_hypercube`: any k, n_runs ≤ 200

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Types ──────────────────────────────────────────────────────────────────────

/// The type of experimental design.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DoeType {
    FullFactorial,
    CentralComposite,
    LatinHypercube,
}

/// A single experimental factor (independent variable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Factor {
    /// Variable name (e.g., "temperature_c", "ph", "concentration_mm").
    pub name: String,
    /// Physical unit (e.g., "°C", "pH", "mM").
    pub unit: String,
    /// Low-level value.
    pub low: f64,
    /// High-level value.
    pub high: f64,
    /// Explicit levels for categorical factors.  `None` for continuous.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub levels: Option<Vec<f64>>,
}

impl Factor {
    /// Map a coded value (−1 to +1) to the natural scale.
    pub fn decode(&self, coded: f64) -> f64 {
        let center = (self.high + self.low) / 2.0;
        let half_range = (self.high - self.low) / 2.0;
        center + coded * half_range
    }

    /// Midpoint of the factor range.
    pub fn center(&self) -> f64 {
        (self.high + self.low) / 2.0
    }
}

/// A structured experimental design — run matrix + metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoeDesign {
    pub design_type: DoeType,
    pub factors: Vec<Factor>,
    /// Each row is one experimental run.  Keys are factor names; values are
    /// natural-scale levels.  Replicates are already expanded.
    pub runs: Vec<HashMap<String, f64>>,
    pub replicates: u32,
    /// True when run order has been randomized.
    pub randomized: bool,
    /// Additional metadata (e.g., number of center points).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
}

impl DoeDesign {
    /// Total number of rows in the run matrix (including replicates).
    pub fn n_runs(&self) -> usize {
        self.runs.len()
    }
}

// ── Full factorial ─────────────────────────────────────────────────────────────

/// Generate a 2^k full factorial design (low/high coded as −1/+1).
///
/// # Panics / Errors
/// Returns `Err` if `factors.is_empty()` or `factors.len() > 5`.
pub fn full_factorial(factors: &[Factor]) -> Result<DoeDesign, String> {
    if factors.is_empty() {
        return Err("full_factorial requires at least one factor".into());
    }
    if factors.len() > 5 {
        return Err(format!(
            "full_factorial supports at most 5 factors (got {}); use central_composite or latin_hypercube for k > 5",
            factors.len()
        ));
    }
    let k = factors.len();
    let n = 1usize << k; // 2^k
    let mut runs = Vec::with_capacity(n);
    for run_idx in 0..n {
        let mut row = HashMap::new();
        for (j, factor) in factors.iter().enumerate() {
            let bit = (run_idx >> j) & 1;
            let natural = if bit == 0 { factor.low } else { factor.high };
            row.insert(factor.name.clone(), natural);
        }
        runs.push(row);
    }
    Ok(DoeDesign {
        design_type: DoeType::FullFactorial,
        factors: factors.to_vec(),
        runs,
        replicates: 1,
        randomized: false,
        metadata: [("design_points".into(), serde_json::json!(n))].into_iter().collect(),
    })
}

// ── Central composite design ───────────────────────────────────────────────────

/// Generate a face-centered central composite design (CCD).
///
/// Face-centered: axial distance α = 1 (axial points at ±1 in coded units,
/// same as factorial points, so no extrapolation outside the factor bounds).
///
/// Requires 2 ≤ k ≤ 4 factors.
pub fn central_composite(factors: &[Factor]) -> Result<DoeDesign, String> {
    let k = factors.len();
    if k < 2 {
        return Err("central_composite requires at least 2 factors".into());
    }
    if k > 4 {
        return Err(format!(
            "central_composite supports at most 4 factors (got {k}); use latin_hypercube for larger designs"
        ));
    }

    let mut runs = Vec::new();

    // Factorial block (2^k).
    let n_fact = 1usize << k;
    for run_idx in 0..n_fact {
        let mut row = HashMap::new();
        for (j, factor) in factors.iter().enumerate() {
            let bit = (run_idx >> j) & 1;
            row.insert(factor.name.clone(), if bit == 0 { factor.low } else { factor.high });
        }
        runs.push(row);
    }

    // Axial (star) points: ±1 in each dimension, 0 in others (face-centered α=1).
    for j in 0..k {
        for &coded in &[-1.0_f64, 1.0_f64] {
            let mut row = HashMap::new();
            for (jj, factor) in factors.iter().enumerate() {
                let val = if jj == j { factor.decode(coded) } else { factor.center() };
                row.insert(factor.name.clone(), val);
            }
            runs.push(row);
        }
    }

    // Center points (3 replicates for pure-error estimation).
    for _ in 0..3 {
        let row: HashMap<_, _> = factors.iter().map(|f| (f.name.clone(), f.center())).collect();
        runs.push(row);
    }

    let n_total = runs.len();
    Ok(DoeDesign {
        design_type: DoeType::CentralComposite,
        factors: factors.to_vec(),
        runs,
        replicates: 1,
        randomized: false,
        metadata: [
            ("factorial_points".into(), serde_json::json!(n_fact)),
            ("axial_points".into(),     serde_json::json!(2 * k)),
            ("center_points".into(),    serde_json::json!(3)),
            ("total_runs".into(),        serde_json::json!(n_total)),
        ].into_iter().collect(),
    })
}

// ── Latin hypercube ────────────────────────────────────────────────────────────

/// Generate a Latin hypercube sample (LHS).
///
/// Each factor is divided into `n_runs` equal-probability strata; one point is
/// chosen uniformly at random from each stratum.  This guarantees that each
/// stratum contains exactly one observation — better space-filling than
/// purely random sampling.
///
/// The random number stream is seeded by `seed` for reproducibility.
///
/// `n_runs` is clamped to [k+1, 200].
pub fn latin_hypercube(factors: &[Factor], n_runs: usize, seed: u64) -> Result<DoeDesign, String> {
    if factors.is_empty() {
        return Err("latin_hypercube requires at least one factor".into());
    }
    let k = factors.len();
    let n = n_runs.clamp(k + 1, 200);

    // Minimal LCG random number generator (avoids external rand dep for reproducibility).
    let mut rng = LcgRng::new(seed);

    let mut runs: Vec<HashMap<String, f64>> = (0..n).map(|_| HashMap::new()).collect();

    for factor in factors {
        // Create a shuffled permutation of [0..n].
        let mut perm: Vec<usize> = (0..n).collect();
        fisher_yates_shuffle(&mut perm, &mut rng);

        for (i, run) in runs.iter_mut().enumerate() {
            // Stratum i gets: U(perm[i]/n, (perm[i]+1)/n) mapped to [low, high].
            let u = (perm[i] as f64 + rng.next_f64()) / n as f64;
            let val = factor.low + u * (factor.high - factor.low);
            run.insert(factor.name.clone(), val);
        }
    }

    Ok(DoeDesign {
        design_type: DoeType::LatinHypercube,
        factors: factors.to_vec(),
        runs,
        replicates: 1,
        randomized: true,
        metadata: [
            ("n_runs".into(), serde_json::json!(n)),
            ("seed".into(),   serde_json::json!(seed)),
        ].into_iter().collect(),
    })
}

// ── Minimal LCG RNG ───────────────────────────────────────────────────────────

struct LcgRng(u64);

impl LcgRng {
    fn new(seed: u64) -> Self { Self(seed ^ 0x1234_5678_9abc_def0) }

    fn next_u64(&mut self) -> u64 {
        // Knuth's multiplicative constant + Steele's additive constant.
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0
    }

    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn fisher_yates_shuffle(arr: &mut [usize], rng: &mut LcgRng) {
    let n = arr.len();
    for i in (1..n).rev() {
        let j = (rng.next_u64() as usize) % (i + 1);
        arr.swap(i, j);
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ph_temp_factors() -> Vec<Factor> {
        vec![
            Factor { name: "ph".into(),   unit: "pH".into(), low: 6.0, high: 8.0, levels: None },
            Factor { name: "temp".into(), unit: "°C".into(), low: 25.0, high: 37.0, levels: None },
        ]
    }

    #[test]
    fn full_factorial_2k_correct_run_count() {
        let factors = ph_temp_factors();
        let design = full_factorial(&factors).unwrap();
        assert_eq!(design.n_runs(), 4);   // 2^2 = 4
        assert_eq!(design.design_type, DoeType::FullFactorial);
    }

    #[test]
    fn full_factorial_3k_correct_run_count() {
        let mut f = ph_temp_factors();
        f.push(Factor { name: "conc".into(), unit: "mM".into(), low: 1.0, high: 10.0, levels: None });
        let design = full_factorial(&f).unwrap();
        assert_eq!(design.n_runs(), 8);   // 2^3 = 8
    }

    #[test]
    fn full_factorial_rejects_too_many_factors() {
        let factors: Vec<Factor> = (0..6).map(|i| Factor {
            name: format!("f{i}"), unit: "".into(), low: 0.0, high: 1.0, levels: None
        }).collect();
        assert!(full_factorial(&factors).is_err());
    }

    #[test]
    fn full_factorial_covers_all_combinations() {
        let factors = ph_temp_factors();
        let design = full_factorial(&factors).unwrap();
        // All 4 combinations of low/high should be present.
        let ph_vals: Vec<f64> = design.runs.iter().map(|r| r["ph"]).collect();
        assert!(ph_vals.contains(&6.0));
        assert!(ph_vals.contains(&8.0));
    }

    #[test]
    fn central_composite_run_count() {
        let factors = ph_temp_factors();
        let design = central_composite(&factors).unwrap();
        // 2^2 + 2*2 + 3 center = 4 + 4 + 3 = 11
        assert_eq!(design.n_runs(), 11);
    }

    #[test]
    fn central_composite_rejects_single_factor() {
        let f = vec![Factor { name: "ph".into(), unit: "pH".into(), low: 6.0, high: 8.0, levels: None }];
        assert!(central_composite(&f).is_err());
    }

    #[test]
    fn latin_hypercube_run_count() {
        let design = latin_hypercube(&ph_temp_factors(), 20, 42).unwrap();
        assert_eq!(design.n_runs(), 20);
    }

    #[test]
    fn latin_hypercube_reproducible() {
        let d1 = latin_hypercube(&ph_temp_factors(), 10, 999).unwrap();
        let d2 = latin_hypercube(&ph_temp_factors(), 10, 999).unwrap();
        for (r1, r2) in d1.runs.iter().zip(d2.runs.iter()) {
            assert!((r1["ph"] - r2["ph"]).abs() < 1e-12);
        }
    }

    #[test]
    fn latin_hypercube_values_in_range() {
        let factors = ph_temp_factors();
        let design = latin_hypercube(&factors, 50, 7).unwrap();
        for run in &design.runs {
            assert!(run["ph"]   >= 6.0 && run["ph"]   <= 8.0);
            assert!(run["temp"] >= 25.0 && run["temp"] <= 37.0);
        }
    }

    #[test]
    fn factor_decode_correct() {
        let f = Factor { name: "x".into(), unit: "".into(), low: 10.0, high: 20.0, levels: None };
        assert!((f.decode(-1.0) - 10.0).abs() < 1e-10);
        assert!((f.decode(0.0)  - 15.0).abs() < 1e-10);
        assert!((f.decode(1.0)  - 20.0).abs() < 1e-10);
    }
}
