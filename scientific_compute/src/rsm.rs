//! Response Surface Methodology (RSM) for AxiomLab.
//!
//! Provides:
//! - [`fit_rsm`] — fit a full quadratic response surface model to DoE data
//! - [`canonical_analysis`] — find the stationary point and classify it
//! - [`tukey_hsd`] — Tukey Honest Significant Difference post-hoc test
//!
//! # Design matrix column order (k factors)
//! `[1 | x₁…xₖ | x₁²…xₖ² | x₁x₂, x₁x₃, …, x_{k-1}xₖ]`
//!
//! All factor levels are converted to coded units (−1 to +1) internally.
//! Eigenvalues for canonical analysis use Jacobi iteration (≤ 50 sweeps).
//! The studentized range critical value uses double numerical quadrature
//! over the chi and standard-normal distributions.

use serde::{Deserialize, Serialize};

use crate::doe::DoeDesign;
use crate::stats::{
    dot, f_cdf_upper_tail, gauss_solve, log_gamma, mat_mul_t, mat_xt_vec, AnovaResult, AnovaRow2,
};

// ── Public types ───────────────────────────────────────────────────────────────

/// A single term in the quadratic RSM model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RsmTerm {
    Intercept,
    Linear(usize),
    Quadratic(usize),
    Interaction(usize, usize),
}

/// Fitted response surface model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RsmModel {
    pub terms:         Vec<RsmTerm>,
    /// OLS coefficients in the same order as `terms`.
    pub coefficients:  Vec<f64>,
    pub factor_names:  Vec<String>,
    pub r_squared:     f64,
    pub adj_r_squared: f64,
    /// Mean squared error (MS_error).
    pub ms_error:      f64,
    /// ANOVA table: [Regression | Error | Total].
    pub anova_table:   Vec<AnovaRow2>,
    pub n_obs:         usize,
}

/// Nature of the stationary point identified by canonical analysis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StationaryKind {
    Maximum,
    Minimum,
    Saddle,
}

/// Result of a canonical analysis on a fitted RSM surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalResult {
    /// Stationary point in coded units.
    pub stationary_point_coded:   Vec<f64>,
    /// Stationary point in natural (physical) units.
    pub stationary_point_natural: Vec<f64>,
    /// Predicted response at the stationary point.
    pub predicted_response:       f64,
    /// Eigenvalues of the quadratic matrix B (sign pattern determines `kind`).
    pub eigenvalues:              Vec<f64>,
    pub kind:                     StationaryKind,
}

/// One pairwise comparison in a Tukey HSD test.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TukeyComparison {
    pub group_i:     usize,
    pub group_j:     usize,
    /// |mean_i − mean_j|
    pub mean_diff:   f64,
    /// HSD threshold for this comparison.
    pub hsd:         f64,
    pub significant: bool,
}

/// Result of a Tukey HSD post-hoc test.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TukeyResult {
    pub comparisons: Vec<TukeyComparison>,
    pub alpha:       f64,
    pub ms_within:   f64,
    pub n_harmonic:  f64,
}

// ── fit_rsm ───────────────────────────────────────────────────────────────────

/// Fit a full quadratic RSM model to a DoE design and response vector.
///
/// # Arguments
/// - `design`    — experimental design (CCD or factorial, k ≥ 2 factors)
/// - `responses` — observed response for each run (same length as `design.runs`)
///
/// # Returns
/// `Err` if k < 2, run count mismatches, or the design matrix is singular.
pub fn fit_rsm(design: &DoeDesign, responses: &[f64]) -> Result<RsmModel, String> {
    let k = design.factors.len();
    if k < 2 {
        return Err("fit_rsm requires at least 2 factors".into());
    }
    if design.runs.len() != responses.len() {
        return Err(format!(
            "design has {} runs but {} responses were provided",
            design.runs.len(),
            responses.len()
        ));
    }
    let n = responses.len();
    // 1 intercept + k linear + k quadratic + k(k-1)/2 interactions
    let n_params = 1 + 2 * k + k * (k - 1) / 2;
    if n < n_params + 1 {
        return Err(format!(
            "need at least {} observations for a {k}-factor RSM model (have {n})",
            n_params + 1
        ));
    }

    // Build ordered term list.
    let mut terms: Vec<RsmTerm> = Vec::with_capacity(n_params);
    terms.push(RsmTerm::Intercept);
    for i in 0..k { terms.push(RsmTerm::Linear(i)); }
    for i in 0..k { terms.push(RsmTerm::Quadratic(i)); }
    for i in 0..k {
        for j in (i + 1)..k {
            terms.push(RsmTerm::Interaction(i, j));
        }
    }

    // Build design matrix (n × n_params) in coded units.
    let x_mat: Vec<Vec<f64>> = design.runs.iter().map(|run| {
        let coded: Vec<f64> = design.factors.iter().map(|f| {
            let nat      = run.get(&f.name).copied().unwrap_or_else(|| f.center());
            let half     = (f.high - f.low) / 2.0;
            if half.abs() > 1e-14 { (nat - f.center()) / half } else { 0.0 }
        }).collect();
        build_row(&coded, &terms)
    }).collect();

    // Normal equations: (X'X) β = X'y.
    let xtx  = mat_mul_t(&x_mat, n_params);
    let xty  = mat_xt_vec(&x_mat, responses);
    let beta = gauss_solve(&xtx, &xty)
        .ok_or_else(|| "design matrix X'X is singular (check for duplicate or collinear runs)".to_string())?;

    // Fitted values, SS, and model metrics.
    let y_hat: Vec<f64> = x_mat.iter().map(|row| dot(row, &beta)).collect();
    let y_mean   = responses.iter().sum::<f64>() / n as f64;
    let ss_res:  f64 = responses.iter().zip(&y_hat).map(|(y, yh)| (y - yh).powi(2)).sum();
    let ss_tot:  f64 = responses.iter().map(|y| (y - y_mean).powi(2)).sum();
    let ss_reg         = ss_tot - ss_res;
    let df_reg         = (n_params - 1) as f64;
    let df_err         = (n - n_params) as f64;
    let ms_reg         = ss_reg / df_reg;
    let ms_err         = ss_res / df_err;
    let f_reg          = ms_reg / ms_err;
    let p_reg          = f_cdf_upper_tail(f_reg, df_reg, df_err);
    let r2             = if ss_tot > 1e-14 { 1.0 - ss_res / ss_tot } else { 1.0 };
    let adj_r2         = 1.0 - (1.0 - r2) * (n as f64 - 1.0) / df_err;

    let anova_table = vec![
        AnovaRow2 { source: "Regression".into(), ss: ss_reg, df: df_reg, ms: ms_reg,
                    f_statistic: Some(f_reg), p_value: Some(p_reg) },
        AnovaRow2 { source: "Error".into(),      ss: ss_res, df: df_err, ms: ms_err,
                    f_statistic: None, p_value: None },
        AnovaRow2 { source: "Total".into(),       ss: ss_tot, df: (n - 1) as f64, ms: f64::NAN,
                    f_statistic: None, p_value: None },
    ];

    Ok(RsmModel {
        terms,
        coefficients:  beta,
        factor_names:  design.factors.iter().map(|f| f.name.clone()).collect(),
        r_squared:     r2,
        adj_r_squared: adj_r2,
        ms_error:      ms_err,
        anova_table,
        n_obs:         n,
    })
}

// ── canonical_analysis ────────────────────────────────────────────────────────

/// Find the stationary point of the fitted RSM surface and classify it.
///
/// Extracts the linear vector **b** and symmetric quadratic matrix **B** from the
/// model, solves 2**B**x* = −**b** for the stationary point, computes eigenvalues
/// of **B** via Jacobi iteration, and classifies the result.
pub fn canonical_analysis(model: &RsmModel, design: &DoeDesign) -> Result<CanonicalResult, String> {
    let k = design.factors.len();
    let expected = 1 + 2 * k + k * (k - 1) / 2;
    if model.coefficients.len() < expected {
        return Err(format!(
            "model has {} coefficients but {k}-factor RSM requires {expected}",
            model.coefficients.len()
        ));
    }

    // Linear vector b = [β₁, β₂, …, βₖ] (indices 1..=k).
    let b: Vec<f64> = model.coefficients[1..=k].to_vec();

    // Symmetric B matrix.
    // Diagonal: B[i][i] = β_{i+1,i+1} (quadratic), index 1+k+i.
    // Off-diagonal: B[i][j] = β_{ij}/2 (interaction), index starting at 1+2k.
    let mut b_mat = vec![vec![0.0_f64; k]; k];
    for i in 0..k {
        b_mat[i][i] = model.coefficients[1 + k + i];
    }
    let mut idx = 1 + 2 * k;
    for i in 0..k {
        for j in (i + 1)..k {
            let half = model.coefficients[idx] / 2.0;
            b_mat[i][j] = half;
            b_mat[j][i] = half;
            idx += 1;
        }
    }

    // Stationary point: 2B · x* = −b.
    let two_b: Vec<Vec<f64>> = b_mat.iter()
        .map(|row| row.iter().map(|&v| 2.0 * v).collect())
        .collect();
    let neg_b: Vec<f64> = b.iter().map(|&v| -v).collect();
    let x_star = gauss_solve(&two_b, &neg_b)
        .unwrap_or_else(|| vec![0.0; k]); // Degenerate B → center

    // Predicted response at stationary point.
    let x_row = build_row(&x_star, &model.terms);
    let y_star: f64 = x_row.iter().zip(&model.coefficients).map(|(xi, bi)| xi * bi).sum();

    // Eigenvalues of B (classify nature of stationary point).
    let eigenvalues = jacobi_eigenvalues(&b_mat, 50);
    let all_neg = eigenvalues.iter().all(|&e| e < -1e-8);
    let all_pos = eigenvalues.iter().all(|&e| e >  1e-8);
    let kind = if all_neg { StationaryKind::Maximum }
               else if all_pos { StationaryKind::Minimum }
               else { StationaryKind::Saddle };

    // Decode stationary point to natural units.
    let x_natural: Vec<f64> = x_star.iter().enumerate()
        .map(|(i, &xi)| design.factors[i].decode(xi))
        .collect();

    Ok(CanonicalResult {
        stationary_point_coded:   x_star,
        stationary_point_natural: x_natural,
        predicted_response:       y_star,
        eigenvalues,
        kind,
    })
}

// ── tukey_hsd ─────────────────────────────────────────────────────────────────

/// Tukey's Honest Significant Difference (HSD) post-hoc test.
///
/// Compares all pairs of groups after a significant one-way ANOVA.
/// `anova` must be the `AnovaResult` from `anova_one_way` on the same `groups`.
pub fn tukey_hsd(
    groups: &[Vec<f64>],
    anova:  &AnovaResult,
    alpha:  f64,
) -> Result<TukeyResult, String> {
    let k = groups.len();
    if k < 2 {
        return Err("tukey_hsd requires at least 2 groups".into());
    }

    // Harmonic mean of group sizes.
    let n_harmonic = {
        let inv_sum: f64 = groups.iter().map(|g| 1.0 / g.len() as f64).sum();
        k as f64 / inv_sum
    };

    let ms_within = anova.ms_within;
    let df_within = anova.df_within;
    let q_crit    = studentized_range_critical(alpha, k, df_within);
    let hsd       = q_crit * (ms_within / n_harmonic).sqrt();

    let means: Vec<f64> = groups
        .iter()
        .map(|g| g.iter().sum::<f64>() / g.len() as f64)
        .collect();

    let mut comparisons = Vec::new();
    for i in 0..k {
        for j in (i + 1)..k {
            let diff = (means[i] - means[j]).abs();
            comparisons.push(TukeyComparison {
                group_i: i, group_j: j,
                mean_diff:   diff,
                hsd,
                significant: diff > hsd,
            });
        }
    }

    Ok(TukeyResult { comparisons, alpha, ms_within, n_harmonic })
}

// ── Internal helpers ───────────────────────────────────────────────────────────

/// Build one row of the design matrix from coded factor values.
fn build_row(coded: &[f64], terms: &[RsmTerm]) -> Vec<f64> {
    terms.iter().map(|t| match t {
        RsmTerm::Intercept         => 1.0,
        RsmTerm::Linear(i)         => coded[*i],
        RsmTerm::Quadratic(i)      => coded[*i].powi(2),
        RsmTerm::Interaction(i, j) => coded[*i] * coded[*j],
    }).collect()
}

/// Jacobi eigenvalue algorithm for symmetric matrices.
///
/// Repeatedly zeros out the largest off-diagonal element via Givens rotations.
/// Converges in ≤ `max_sweeps` full sweeps (accurate for 2×2–4×4 matrices).
/// Returns eigenvalues (unordered diagonal after convergence).
fn jacobi_eigenvalues(a: &[Vec<f64>], max_sweeps: usize) -> Vec<f64> {
    let n = a.len();
    let mut m: Vec<Vec<f64>> = a.to_vec();

    for _ in 0..max_sweeps {
        // Find largest off-diagonal.
        let (mut p, mut q) = (0, 1);
        let mut max_off = 0.0_f64;
        for i in 0..n {
            for j in (i + 1)..n {
                if m[i][j].abs() > max_off {
                    max_off = m[i][j].abs();
                    p = i;
                    q = j;
                }
            }
        }
        if max_off < 1e-12 { break; }

        // Rotation angle: tan(2θ) = 2·A[p][q] / (A[q][q] - A[p][p]).
        let tau = (m[q][q] - m[p][p]) / (2.0 * m[p][q]);
        let t   = if tau >= 0.0 {
             1.0 / (tau + (1.0 + tau * tau).sqrt())
        } else {
             1.0 / (tau - (1.0 + tau * tau).sqrt())
        };
        let cos = 1.0 / (1.0 + t * t).sqrt();
        let sin = t * cos;

        // Update matrix: A ← G'AG.
        let app = m[p][p];
        let aqq = m[q][q];
        let apq = m[p][q];
        m[p][p] = app - t * apq;
        m[q][q] = aqq + t * apq;
        m[p][q] = 0.0;
        m[q][p] = 0.0;
        for r in 0..n {
            if r != p && r != q {
                let arp = m[r][p];
                let arq = m[r][q];
                m[r][p] = arp * cos - arq * sin;
                m[p][r] = m[r][p];
                m[r][q] = arq * cos + arp * sin;
                m[q][r] = m[r][q];
            }
        }
    }

    (0..n).map(|i| m[i][i]).collect()
}

/// Critical value q such that P(Q > q | k groups, df error) = alpha.
///
/// Uses binary search over the CDF computed via double quadrature.
fn studentized_range_critical(alpha: f64, k: usize, df: f64) -> f64 {
    let mut lo = 0.0_f64;
    let mut hi = 25.0_f64;
    for _ in 0..64 {
        let mid = (lo + hi) / 2.0;
        if srd_upper_tail(mid, k, df) < alpha {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    (lo + hi) / 2.0
}

#[inline]
fn srd_upper_tail(x: f64, k: usize, df: f64) -> f64 {
    1.0 - srd_cdf(x, k, df)
}

/// CDF P(Q ≤ x | k, ν) of the studentized range distribution.
///
/// Outer integral: chi(ν) density on (0, max_t), 64-point midpoint rule.
/// Inner integral: P(range/σ ≤ x·t | k), 60-point midpoint rule over z.
fn srd_cdf(x: f64, k: usize, nu: f64) -> f64 {
    if x <= 0.0 { return 0.0; }
    const N_OUTER: usize = 64;
    let max_t  = (nu.max(1.0).sqrt() * 4.0).max(7.0);
    let step   = max_t / N_OUTER as f64;
    // Correct formula: Q = W / (chi_ν/√ν), so inner arg is x * (t/√ν).
    let inv_sqrt_nu = 1.0 / nu.sqrt();
    let sum: f64 = (0..N_OUTER).map(|i| {
        let t = (i as f64 + 0.5) * step;
        chi_pdf(t, nu) * srd_inner(x * t * inv_sqrt_nu, k)
    }).sum();
    (sum * step).min(1.0)
}

/// P(range/σ ≤ r | k normal(0,1) obs) = k ∫ φ(z)[Φ(z)−Φ(z−r)]^{k−1} dz
fn srd_inner(r: f64, k: usize) -> f64 {
    if r <= 0.0 { return 0.0; }
    const N: usize = 60;
    let lo   = -6.0_f64;
    let hi   = 6.0 + r;
    let step = (hi - lo) / N as f64;
    let sum: f64 = (0..N).map(|i| {
        let z    = lo + (i as f64 + 0.5) * step;
        let phi  = normal_pdf(z);
        let diff = (normal_cdf(z) - normal_cdf(z - r)).max(0.0);
        phi * diff.powi(k as i32 - 1)
    }).sum();
    (k as f64 * sum * step).min(1.0)
}

/// Chi(ν) distribution PDF: f(t;ν) = 2^(1−ν/2)/Γ(ν/2) · t^(ν−1) · exp(−t²/2).
fn chi_pdf(t: f64, nu: f64) -> f64 {
    if t <= 0.0 { return 0.0; }
    let log_d = (nu - 1.0) * t.ln()
        - t * t / 2.0
        - (nu / 2.0 - 1.0) * std::f64::consts::LN_2
        - log_gamma(nu / 2.0);
    log_d.exp()
}

fn normal_pdf(z: f64) -> f64 {
    (-z * z / 2.0).exp() / (2.0 * std::f64::consts::PI).sqrt()
}

fn normal_cdf(z: f64) -> f64 {
    0.5 * (1.0 + erf_approx(z * std::f64::consts::FRAC_1_SQRT_2))
}

/// Error function via Horner polynomial (Abramowitz & Stegun 7.1.26).
/// Max relative error ≈ 1.5 × 10⁻⁷.
fn erf_approx(x: f64) -> f64 {
    let t = 1.0 / (1.0 + 0.327_591_1 * x.abs());
    let poly = t * (0.254_829_592
        + t * (-0.284_496_736
        + t * ( 1.421_413_741
        + t * (-1.453_152_027
        + t *   1.061_405_429))));
    let sign = if x >= 0.0 { 1.0 } else { -1.0 };
    sign * (1.0 - poly * (-x * x).exp())
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doe::{DoeDesign, DoeType, Factor};
    use crate::stats::anova_one_way;
    use std::collections::HashMap;

    // ── Fixtures ──────────────────────────────────────────────────────────────

    /// Build a 2-factor DoeDesign where natural units = coded units
    /// (low=-1, high=+1, center=0, half=1).
    fn design_2f(runs_coded: &[(f64, f64)]) -> DoeDesign {
        let factors = vec![
            Factor { name: "x1".into(), unit: "".into(), low: -1.0, high: 1.0, levels: None },
            Factor { name: "x2".into(), unit: "".into(), low: -1.0, high: 1.0, levels: None },
        ];
        let runs: Vec<HashMap<String, f64>> = runs_coded.iter().map(|(a, b)| {
            let mut m = HashMap::new();
            m.insert("x1".into(), *a);
            m.insert("x2".into(), *b);
            m
        }).collect();
        DoeDesign {
            design_type: DoeType::CentralComposite,
            factors,
            runs,
            replicates: 1,
            randomized: false,
            metadata: HashMap::new(),
        }
    }

    /// 9-point 3×3 factorial grid: x1, x2 ∈ {−1, 0, +1}.
    fn grid_design_3x3() -> DoeDesign {
        let pts: Vec<(f64, f64)> = vec![
            (-1.0,-1.0),(-1.0,0.0),(-1.0,1.0),
            ( 0.0,-1.0),( 0.0,0.0),( 0.0,1.0),
            ( 1.0,-1.0),( 1.0,0.0),( 1.0,1.0),
        ];
        design_2f(&pts)
    }

    // ── fit_rsm ───────────────────────────────────────────────────────────────

    /// Perfect noise-free quadratic surface must be recovered exactly (R² = 1).
    #[test]
    fn fit_rsm_recovers_known_coefficients() {
        let design = grid_design_3x3();
        // y = 3 + 2x₁ − x₂ + 0.5x₁² + 0.3x₂² − 0.4x₁x₂
        let true_betas = [3.0, 2.0, -1.0, 0.5, 0.3, -0.4_f64];
        let y: Vec<f64> = design.runs.iter().map(|run| {
            let x1 = run["x1"]; let x2 = run["x2"];
            true_betas[0]
                + true_betas[1]*x1 + true_betas[2]*x2
                + true_betas[3]*x1*x1 + true_betas[4]*x2*x2
                + true_betas[5]*x1*x2
        }).collect();

        let m = fit_rsm(&design, &y).unwrap();
        assert!((m.r_squared - 1.0).abs() < 1e-8, "R² should be 1 for exact data");
        for (i, (&got, &want)) in m.coefficients.iter().zip(&true_betas).enumerate() {
            assert!((got - want).abs() < 1e-8,
                "β[{i}]: got {got:.6}, want {want:.6}");
        }
    }

    #[test]
    fn fit_rsm_r_squared_below_one_for_noisy_data() {
        let design = grid_design_3x3();
        // Non-polynomial noise (not in the model column space) → R² < 1.
        // Runs at (-1,-1),(-1,0),(-1,1),(0,-1),(0,0),(0,1),(1,-1),(1,0),(1,1).
        let noise = [0.30_f64, -0.15, 0.20, -0.25, 0.10, -0.20, 0.25, -0.10, 0.15];
        let y: Vec<f64> = design.runs.iter().enumerate().map(|(i, run)| {
            let x1 = run["x1"]; let x2 = run["x2"];
            1.0 + x1 - x2 + 0.5*x1*x1 + noise[i]
        }).collect();

        let m = fit_rsm(&design, &y).unwrap();
        assert!(m.r_squared < 1.0 - 1e-8, "noisy data should give R² < 1");
        assert!(m.r_squared > 0.5, "signal should still dominate noise");
    }

    #[test]
    fn fit_rsm_anova_table_rows() {
        let design = grid_design_3x3();
        let y: Vec<f64> = design.runs.iter().map(|r| r["x1"] + r["x2"]).collect();
        let m = fit_rsm(&design, &y).unwrap();
        assert_eq!(m.anova_table[0].source, "Regression");
        assert_eq!(m.anova_table[1].source, "Error");
        assert_eq!(m.anova_table[2].source, "Total");
        // SS_total = SS_reg + SS_err.
        let ss_check = m.anova_table[0].ss + m.anova_table[1].ss;
        assert!((ss_check - m.anova_table[2].ss).abs() < 1e-8);
    }

    #[test]
    fn fit_rsm_rejects_single_factor() {
        use crate::doe::Factor;
        let f = Factor { name: "x".into(), unit: "".into(), low: -1.0, high: 1.0, levels: None };
        let run: HashMap<String, f64> = [("x".into(), 0.0)].into();
        let d = DoeDesign {
            design_type: DoeType::FullFactorial,
            factors: vec![f],
            runs: vec![run; 5],
            replicates: 1, randomized: false, metadata: HashMap::new(),
        };
        assert!(fit_rsm(&d, &[1.0,2.0,3.0,4.0,5.0]).is_err());
    }

    #[test]
    fn fit_rsm_rejects_too_few_observations() {
        // 2-factor RSM needs ≥ 7 obs (6 params + 1 df for error).
        let pts: Vec<(f64, f64)> = vec![(-1.,-1.),(-1.,1.),(1.,-1.),(1.,1.),(0.,0.)];
        let d = design_2f(&pts);
        assert!(fit_rsm(&d, &[1.,2.,3.,4.,5.]).is_err());
    }

    // ── canonical_analysis ────────────────────────────────────────────────────

    /// y = 5 − x₁² − x₂² has a maximum at the origin.
    #[test]
    fn canonical_maximum_at_origin() {
        let design = grid_design_3x3();
        let y: Vec<f64> = design.runs.iter().map(|r| {
            5.0 - r["x1"].powi(2) - r["x2"].powi(2)
        }).collect();
        let m = fit_rsm(&design, &y).unwrap();
        let ca = canonical_analysis(&m, &design).unwrap();

        for &xi in &ca.stationary_point_coded {
            assert!(xi.abs() < 1e-8, "stationary point should be at origin");
        }
        assert!((ca.predicted_response - 5.0).abs() < 1e-6);
        assert_eq!(ca.kind, StationaryKind::Maximum);
        assert!(ca.eigenvalues.iter().all(|&e| e < 0.0), "all eigenvalues negative");
    }

    /// y = x₁² + x₂² has a minimum at the origin.
    #[test]
    fn canonical_minimum_at_origin() {
        let design = grid_design_3x3();
        let y: Vec<f64> = design.runs.iter().map(|r| {
            r["x1"].powi(2) + r["x2"].powi(2)
        }).collect();
        let m = fit_rsm(&design, &y).unwrap();
        let ca = canonical_analysis(&m, &design).unwrap();
        assert_eq!(ca.kind, StationaryKind::Minimum);
        assert!(ca.eigenvalues.iter().all(|&e| e > 0.0));
    }

    /// y = x₁² − x₂² has a saddle point at the origin.
    #[test]
    fn canonical_saddle_point() {
        let design = grid_design_3x3();
        let y: Vec<f64> = design.runs.iter().map(|r| {
            r["x1"].powi(2) - r["x2"].powi(2)
        }).collect();
        let m = fit_rsm(&design, &y).unwrap();
        let ca = canonical_analysis(&m, &design).unwrap();
        assert_eq!(ca.kind, StationaryKind::Saddle);
    }

    // ── Jacobi eigenvalues ────────────────────────────────────────────────────

    /// [[5, 4], [4, 5]] has eigenvalues 9 and 1.
    #[test]
    fn jacobi_2x2_known_eigenvalues() {
        let a = vec![vec![5.0, 4.0], vec![4.0, 5.0]];
        let mut eigs = jacobi_eigenvalues(&a, 50);
        eigs.sort_by(|x, y| x.partial_cmp(y).unwrap());
        assert!((eigs[0] - 1.0).abs() < 1e-10, "smaller eigenvalue");
        assert!((eigs[1] - 9.0).abs() < 1e-10, "larger eigenvalue");
    }

    /// [[3, 1], [1, 3]] has eigenvalues 4 and 2.
    #[test]
    fn jacobi_2x2_second_case() {
        let a = vec![vec![3.0, 1.0], vec![1.0, 3.0]];
        let mut eigs = jacobi_eigenvalues(&a, 50);
        eigs.sort_by(|x, y| x.partial_cmp(y).unwrap());
        assert!((eigs[0] - 2.0).abs() < 1e-10);
        assert!((eigs[1] - 4.0).abs() < 1e-10);
    }

    // ── tukey_hsd ─────────────────────────────────────────────────────────────

    /// Three groups with very distinct means → all pairwise comparisons significant.
    #[test]
    fn tukey_significant_differences() {
        let groups = vec![
            vec![1.0, 1.1, 0.9, 1.0, 1.05],
            vec![5.0, 4.9, 5.1, 5.0, 4.95],
            vec![9.0, 9.1, 8.9, 9.0, 9.05],
        ];
        let anova  = anova_one_way(&groups).unwrap();
        let result = tukey_hsd(&groups, &anova, 0.05).unwrap();

        assert_eq!(result.comparisons.len(), 3); // C(3,2) = 3
        for cmp in &result.comparisons {
            assert!(cmp.significant,
                "groups {}-{} should be significant (diff={:.2}, hsd={:.2})",
                cmp.group_i, cmp.group_j, cmp.mean_diff, cmp.hsd);
        }
    }

    /// Three identical groups → no significant differences.
    #[test]
    fn tukey_no_significant_differences() {
        let groups = vec![
            vec![5.0, 5.0, 5.0, 5.0],
            vec![5.1, 4.9, 5.0, 5.0],
            vec![5.0, 5.0, 5.1, 4.9],
        ];
        let anova  = anova_one_way(&groups).unwrap();
        let result = tukey_hsd(&groups, &anova, 0.05).unwrap();
        for cmp in &result.comparisons {
            assert!(!cmp.significant,
                "near-identical groups should not be significant");
        }
    }

    /// Harmonic mean is computed correctly for equal-sized groups.
    #[test]
    fn tukey_harmonic_mean_equal_groups() {
        let groups = vec![vec![1.0, 2.0, 3.0]; 3];
        let anova  = anova_one_way(&groups).unwrap();
        let result = tukey_hsd(&groups, &anova, 0.05).unwrap();
        assert!((result.n_harmonic - 3.0).abs() < 1e-10,
            "harmonic mean of equal groups = group size");
    }

    /// Studentized range critical value against published table (α=0.05, k=3, df=20).
    /// Expected q ≈ 3.578; accept ±0.15 tolerance for numerical integration error.
    #[test]
    fn studentized_range_critical_value_vs_table() {
        let q = studentized_range_critical(0.05, 3, 20.0);
        assert!((q - 3.578).abs() < 0.15,
            "q(0.05,k=3,df=20) = {q:.3}, expected ≈ 3.578");
    }

    /// q(α=0.05, k=2, df=20) should be ≈ 2.95 (from standard tables).
    #[test]
    fn studentized_range_critical_k2_df20() {
        let q = studentized_range_critical(0.05, 2, 20.0);
        assert!((q - 2.95).abs() < 0.15,
            "q(0.05,k=2,df=20) = {q:.3}, expected ≈ 2.95");
    }
}
