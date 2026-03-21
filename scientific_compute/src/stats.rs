//! Basic statistical functions for AxiomLab.
//!
//! Provides:
//! - One-way ANOVA
//! - Linear regression (OLS)
//! - GUM uncertainty propagation
//!
//! All functions are pure Rust — no C dependencies.

use serde::{Deserialize, Serialize};

// ── One-way ANOVA ─────────────────────────────────────────────────────────────

/// Result of a one-way ANOVA.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnovaResult {
    /// Between-groups degrees of freedom (k−1).
    pub df_between: f64,
    /// Within-groups degrees of freedom (N−k).
    pub df_within: f64,
    /// Mean square between groups.
    pub ms_between: f64,
    /// Mean square within groups (error variance).
    pub ms_within: f64,
    /// F-statistic.
    pub f_statistic: f64,
    /// Approximate p-value (via F-distribution CDF).
    pub p_value: f64,
    /// Eta-squared (proportion of variance explained): SS_between / SS_total.
    pub eta_squared: f64,
    /// Grand mean across all observations.
    pub grand_mean: f64,
}

/// Perform one-way ANOVA on `groups` (each group is a `Vec<f64>` of observations).
///
/// Returns `Err` if fewer than 2 groups are provided, or any group has < 2 observations.
pub fn anova_one_way(groups: &[Vec<f64>]) -> Result<AnovaResult, String> {
    if groups.len() < 2 {
        return Err("anova_one_way requires at least 2 groups".into());
    }
    for (i, g) in groups.iter().enumerate() {
        if g.len() < 2 {
            return Err(format!("group {i} has fewer than 2 observations"));
        }
    }

    let k = groups.len() as f64;
    let n: f64 = groups.iter().map(|g| g.len() as f64).sum();

    let grand_mean: f64 = groups.iter().flat_map(|g| g.iter()).sum::<f64>() / n;

    let ss_between: f64 = groups.iter().map(|g| {
        let gm = g.iter().sum::<f64>() / g.len() as f64;
        g.len() as f64 * (gm - grand_mean).powi(2)
    }).sum();

    let ss_within: f64 = groups.iter().map(|g| {
        let gm = g.iter().sum::<f64>() / g.len() as f64;
        g.iter().map(|&x| (x - gm).powi(2)).sum::<f64>()
    }).sum();

    let ss_total = ss_between + ss_within;
    let df_between = k - 1.0;
    let df_within  = n - k;
    let ms_between = ss_between / df_between;
    let ms_within  = ss_within  / df_within;
    let f_stat     = ms_between / ms_within;
    let eta_sq     = if ss_total > 0.0 { ss_between / ss_total } else { 0.0 };
    let p_value    = f_cdf_upper_tail(f_stat, df_between, df_within);

    Ok(AnovaResult {
        df_between,
        df_within,
        ms_between,
        ms_within,
        f_statistic: f_stat,
        p_value,
        eta_squared: eta_sq,
        grand_mean,
    })
}

// ── Linear regression (OLS) ───────────────────────────────────────────────────

/// Result of a linear regression y ~ X·β.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegressionResult {
    /// Fitted coefficients [β₀, β₁, …].
    pub coefficients: Vec<f64>,
    /// R² (coefficient of determination).
    pub r_squared: f64,
    /// Adjusted R² = 1 − (1 − R²) × (n−1)/(n−p−1).
    pub adj_r_squared: f64,
    /// Residual standard error.
    pub residual_std_error: f64,
    /// Number of observations.
    pub n: usize,
    /// Number of predictors (excluding intercept).
    pub n_predictors: usize,
}

/// Ordinary least-squares linear regression.
///
/// `x[i]` is the predictor vector for observation i; `y[i]` is the response.
/// An intercept column is prepended automatically.
///
/// Returns `Err` if there are fewer observations than predictors+1 or if X'X is singular.
pub fn linear_regression(x: &[Vec<f64>], y: &[f64]) -> Result<RegressionResult, String> {
    if x.len() != y.len() {
        return Err(format!("x and y must have the same length ({} vs {})", x.len(), y.len()));
    }
    let n = x.len();
    if n == 0 {
        return Err("no observations".into());
    }
    let p = x[0].len(); // number of predictors (without intercept)
    if n < p + 2 {
        return Err(format!(
            "need at least {} observations for {p} predictors + intercept (got {n})",
            p + 2
        ));
    }

    // Build design matrix X̃ = [1 | X] (n × (p+1)).
    let pp1 = p + 1;
    let mut x_aug: Vec<Vec<f64>> = x.iter().map(|row| {
        let mut r = vec![1.0];
        r.extend_from_slice(row);
        r
    }).collect();
    if x_aug.iter().any(|r| r.len() != pp1) {
        return Err("all predictor vectors must have the same length".into());
    }

    // Normal equations: β = (X'X)⁻¹ X'y via Cholesky (positive-definite).
    // Build X'X and X'y.
    let xtx = mat_mul_t(&x_aug, pp1);  // (p+1 × p+1)
    let xty = mat_xt_vec(&x_aug, y);   // (p+1)

    // Solve via Gaussian elimination with partial pivoting.
    let beta = gauss_solve(&xtx, &xty)
        .ok_or_else(|| "X'X is singular (check for collinear predictors)".to_string())?;

    // Fitted values and residuals.
    let y_hat: Vec<f64> = x_aug.iter().map(|row| dot(row, &beta)).collect();
    let residuals: Vec<f64> = y.iter().zip(&y_hat).map(|(yi, yhi)| yi - yhi).collect();

    let y_mean = y.iter().sum::<f64>() / n as f64;
    let ss_res: f64 = residuals.iter().map(|r| r * r).sum();
    let ss_tot: f64 = y.iter().map(|yi| (yi - y_mean).powi(2)).sum();

    let r2 = if ss_tot > 0.0 { 1.0 - ss_res / ss_tot } else { 1.0 };
    let adj_r2 = 1.0 - (1.0 - r2) * (n as f64 - 1.0) / (n as f64 - p as f64 - 1.0);
    let rse = (ss_res / (n - pp1) as f64).sqrt();

    Ok(RegressionResult {
        coefficients: beta,
        r_squared: r2,
        adj_r_squared: adj_r2,
        residual_std_error: rse,
        n,
        n_predictors: p,
    })
}

// ── GUM uncertainty propagation ───────────────────────────────────────────────

/// GUM combined standard uncertainty from a list of independent components.
///
/// Each component is `(u_i, sensitivity_coeff_i)`.
/// Returns `sqrt(Σ (c_i × u_i)²)`.
pub fn propagate_uncertainty(components: &[(f64, f64)]) -> f64 {
    components.iter().map(|(u, c)| (c * u).powi(2)).sum::<f64>().sqrt()
}

// ── Internal numerics ─────────────────────────────────────────────────────────

/// X'X (matrix multiply X-transpose by X).
pub(crate) fn mat_mul_t(x: &[Vec<f64>], cols: usize) -> Vec<Vec<f64>> {
    let mut result = vec![vec![0.0; cols]; cols];
    for row in x {
        for i in 0..cols {
            for j in 0..cols {
                result[i][j] += row[i] * row[j];
            }
        }
    }
    result
}

/// X'y vector.
pub(crate) fn mat_xt_vec(x: &[Vec<f64>], y: &[f64]) -> Vec<f64> {
    let cols = x[0].len();
    let mut result = vec![0.0; cols];
    for (row, &yi) in x.iter().zip(y) {
        for (j, &xij) in row.iter().enumerate() {
            result[j] += xij * yi;
        }
    }
    result
}

pub(crate) fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(ai, bi)| ai * bi).sum()
}

/// Gaussian elimination with partial pivoting.  Solves A·x = b in-place.
pub(crate) fn gauss_solve(a: &[Vec<f64>], b: &[f64]) -> Option<Vec<f64>> {
    let n = b.len();
    let mut m: Vec<Vec<f64>> = a.iter()
        .zip(b)
        .map(|(row, &bi)| { let mut r = row.clone(); r.push(bi); r })
        .collect();

    for col in 0..n {
        // Partial pivot.
        let max_row = (col..n).max_by(|&i, &j| {
            m[i][col].abs().partial_cmp(&m[j][col].abs()).unwrap()
        })?;
        m.swap(col, max_row);

        let pivot = m[col][col];
        if pivot.abs() < 1e-14 { return None; } // Singular.

        for row in (col + 1)..n {
            let factor = m[row][col] / pivot;
            for k in col..=n {
                let val = m[col][k];
                m[row][k] -= factor * val;
            }
        }
    }

    // Back-substitution.
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        x[i] = m[i][n];
        for j in (i + 1)..n {
            x[i] -= m[i][j] * x[j];
        }
        x[i] /= m[i][i];
    }
    Some(x)
}

/// Approximate upper-tail CDF of the F distribution using a numerical method.
///
/// Uses a series expansion of the incomplete beta function.
/// Accuracy is sufficient for hypothesis testing (p < 0.05 / p < 0.01 thresholds).
pub(crate) fn f_cdf_upper_tail(f: f64, d1: f64, d2: f64) -> f64 {
    if f <= 0.0 { return 1.0; }
    // x = d1*F / (d1*F + d2) maps F to incomplete beta argument.
    let x = d1 * f / (d1 * f + d2);
    // P(F > f | d1, d2) = I_{1-x}(d2/2, d1/2) ≈ incomplete_beta_upper.
    incomplete_beta(1.0 - x, d2 / 2.0, d1 / 2.0)
}

/// Regularised incomplete beta function I_x(a, b) via continued fraction expansion.
fn incomplete_beta(x: f64, a: f64, b: f64) -> f64 {
    if x <= 0.0 { return 0.0; }
    if x >= 1.0 { return 1.0; }
    // Use continued fraction via Lentz's algorithm (Press et al. §6.4).
    let bt = (log_gamma(a + b) - log_gamma(a) - log_gamma(b)
        + a * x.ln() + b * (1.0 - x).ln()).exp();
    if x < (a + 1.0) / (a + b + 2.0) {
        bt * betacf(x, a, b) / a
    } else {
        1.0 - bt * betacf(1.0 - x, b, a) / b
    }
}

fn betacf(x: f64, a: f64, b: f64) -> f64 {
    const MAXIT: usize = 200;
    const EPS: f64 = 3e-7;
    let qab = a + b;
    let qap = a + 1.0;
    let qam = a - 1.0;
    let mut c = 1.0_f64;
    let mut d = 1.0 - qab * x / qap;
    if d.abs() < f64::MIN_POSITIVE { d = f64::MIN_POSITIVE; }
    d = 1.0 / d;
    let mut h = d;
    for m in 1..=MAXIT {
        let m = m as f64;
        let m2 = 2.0 * m;
        // Even step.
        let aa = m * (b - m) * x / ((qam + m2) * (a + m2));
        d = 1.0 + aa * d;
        if d.abs() < f64::MIN_POSITIVE { d = f64::MIN_POSITIVE; }
        c = 1.0 + aa / c;
        if c.abs() < f64::MIN_POSITIVE { c = f64::MIN_POSITIVE; }
        d = 1.0 / d;
        h *= d * c;
        // Odd step.
        let aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2));
        d = 1.0 + aa * d;
        if d.abs() < f64::MIN_POSITIVE { d = f64::MIN_POSITIVE; }
        c = 1.0 + aa / c;
        if c.abs() < f64::MIN_POSITIVE { c = f64::MIN_POSITIVE; }
        d = 1.0 / d;
        let delta = d * c;
        h *= delta;
        if (delta - 1.0).abs() < EPS { break; }
    }
    h
}

/// Lanczos approximation to ln Γ(z).
pub(crate) fn log_gamma(z: f64) -> f64 {
    let g = 7.0;
    let c: &[f64] = &[
        0.99999999999980993, 676.5203681218851, -1259.1392167224028,
        771.32342877765313, -176.61502916214059, 12.507343278686905,
        -0.13857109526572012, 9.9843695780195716e-6, 1.5056327351493116e-7,
    ];
    let mut z = z;
    if z < 0.5 {
        return std::f64::consts::PI.ln() - (std::f64::consts::PI * z).sin().ln() - log_gamma(1.0 - z);
    }
    z -= 1.0;
    let mut x = c[0];
    for (i, &ci) in c[1..].iter().enumerate() {
        x += ci / (z + i as f64 + 1.0);
    }
    let t = z + g + 0.5;
    0.5 * std::f64::consts::TAU.ln() + (z + 0.5) * t.ln() - t + x.ln()
}

// ── Two-way ANOVA ─────────────────────────────────────────────────────────────

/// One row in a two-way ANOVA table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnovaRow2 {
    pub source:      String,
    pub ss:          f64,
    pub df:          f64,
    pub ms:          f64,
    pub f_statistic: Option<f64>,
    pub p_value:     Option<f64>,
}

/// Result of a balanced two-way ANOVA.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwoWayAnovaResult {
    /// ANOVA table rows: FactorA, FactorB, Interaction, Error, Total.
    pub table:      Vec<AnovaRow2>,
    pub grand_mean: f64,
    /// `cell_means[level_a][level_b]`
    pub cell_means: Vec<Vec<f64>>,
    pub levels_a:   usize,
    pub levels_b:   usize,
    pub n_per_cell: usize,
}

/// Balanced two-way ANOVA.
///
/// `data[i][j]` is the `Vec<f64>` of replicate observations for factor A level `i`
/// and factor B level `j`.  All cells must contain the same number of replicates.
///
/// Returns `Err` if:
/// - fewer than 2 levels for A or B,
/// - any cell has fewer than 2 observations,
/// - the design is unbalanced (cell sizes differ).
pub fn anova_two_way(data: &[Vec<Vec<f64>>]) -> Result<TwoWayAnovaResult, String> {
    let a = data.len();
    if a < 2 {
        return Err("anova_two_way requires at least 2 levels for factor A".into());
    }
    let b = data[0].len();
    if b < 2 {
        return Err("anova_two_way requires at least 2 levels for factor B".into());
    }
    let n_per_cell = data[0][0].len();
    if n_per_cell < 2 {
        return Err("each cell must have at least 2 observations".into());
    }
    for (i, row) in data.iter().enumerate() {
        if row.len() != b {
            return Err(format!(
                "factor A level {i} has {} B-levels, expected {b}",
                row.len()
            ));
        }
        for (j, cell) in row.iter().enumerate() {
            if cell.len() != n_per_cell {
                return Err(format!(
                    "cell [{i}][{j}] has {} observations, expected {n_per_cell} (unbalanced design)",
                    cell.len()
                ));
            }
        }
    }

    // Cell means.
    let cell_means: Vec<Vec<f64>> = data
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| cell.iter().sum::<f64>() / n_per_cell as f64)
                .collect()
        })
        .collect();

    // Grand mean.
    let grand_sum: f64 = data
        .iter()
        .flat_map(|row| row.iter().flat_map(|cell| cell.iter().copied()))
        .sum();
    let n_total = (a * b * n_per_cell) as f64;
    let grand_mean = grand_sum / n_total;

    // Marginal means.
    let row_means: Vec<f64> = cell_means
        .iter()
        .map(|row| row.iter().sum::<f64>() / b as f64)
        .collect();
    let col_means: Vec<f64> = (0..b)
        .map(|j| cell_means.iter().map(|row| row[j]).sum::<f64>() / a as f64)
        .collect();

    let ss_a: f64 = (b * n_per_cell) as f64
        * row_means.iter().map(|&m| (m - grand_mean).powi(2)).sum::<f64>();

    let ss_b: f64 = (a * n_per_cell) as f64
        * col_means.iter().map(|&m| (m - grand_mean).powi(2)).sum::<f64>();

    let ss_ab: f64 = n_per_cell as f64
        * (0..a)
            .map(|i| {
                (0..b)
                    .map(|j| {
                        (cell_means[i][j] - row_means[i] - col_means[j] + grand_mean).powi(2)
                    })
                    .sum::<f64>()
            })
            .sum::<f64>();

    let ss_within: f64 = data
        .iter()
        .enumerate()
        .map(|(i, row)| {
            row.iter()
                .enumerate()
                .map(|(j, cell)| {
                    let cm = cell_means[i][j];
                    cell.iter().map(|&x| (x - cm).powi(2)).sum::<f64>()
                })
                .sum::<f64>()
        })
        .sum();

    let ss_total = ss_a + ss_b + ss_ab + ss_within;

    let df_a      = (a - 1) as f64;
    let df_b      = (b - 1) as f64;
    let df_ab     = df_a * df_b;
    let df_within = (a * b * (n_per_cell - 1)) as f64;
    let df_total  = n_total - 1.0;

    let ms_a      = ss_a      / df_a;
    let ms_b      = ss_b      / df_b;
    let ms_ab     = ss_ab     / df_ab;
    let ms_within = ss_within / df_within;

    let f_a  = ms_a  / ms_within;
    let f_b  = ms_b  / ms_within;
    let f_ab = ms_ab / ms_within;

    let p_a  = f_cdf_upper_tail(f_a,  df_a,  df_within);
    let p_b  = f_cdf_upper_tail(f_b,  df_b,  df_within);
    let p_ab = f_cdf_upper_tail(f_ab, df_ab, df_within);

    let table = vec![
        AnovaRow2 { source: "FactorA".into(),    ss: ss_a,      df: df_a,      ms: ms_a,
                    f_statistic: Some(f_a),  p_value: Some(p_a)  },
        AnovaRow2 { source: "FactorB".into(),    ss: ss_b,      df: df_b,      ms: ms_b,
                    f_statistic: Some(f_b),  p_value: Some(p_b)  },
        AnovaRow2 { source: "Interaction".into(),ss: ss_ab,     df: df_ab,     ms: ms_ab,
                    f_statistic: Some(f_ab), p_value: Some(p_ab) },
        AnovaRow2 { source: "Error".into(),      ss: ss_within, df: df_within, ms: ms_within,
                    f_statistic: None,       p_value: None        },
        AnovaRow2 { source: "Total".into(),      ss: ss_total,  df: df_total,  ms: f64::NAN,
                    f_statistic: None,       p_value: None        },
    ];

    Ok(TwoWayAnovaResult { table, grand_mean, cell_means, levels_a: a, levels_b: b, n_per_cell })
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anova_significant_difference() {
        // Three groups with clearly different means.
        let groups = vec![
            vec![1.0, 1.1, 0.9, 1.0],
            vec![5.0, 4.9, 5.1, 5.0],
            vec![10.0, 9.8, 10.2, 10.0],
        ];
        let r = anova_one_way(&groups).unwrap();
        assert!(r.f_statistic > 100.0, "expected large F for distinct groups");
        assert!(r.p_value < 0.001, "expected significant p-value");
        assert!(r.eta_squared > 0.99);
    }

    #[test]
    fn anova_no_difference() {
        // Same values in all groups → SS_between = 0, F should be 0 or NaN.
        let groups = vec![
            vec![5.0, 5.0, 5.0],
            vec![5.0, 5.0, 5.0],
        ];
        let r = anova_one_way(&groups).unwrap();
        // ms_within is also 0 (no within-group variance), so F = 0/0.
        // Either F is 0 or NaN, and eta_squared is 0.
        assert!(r.f_statistic < 1e-6 || r.f_statistic.is_nan());
        assert!(r.eta_squared < 1e-10);
    }

    #[test]
    fn anova_requires_two_groups() {
        assert!(anova_one_way(&[vec![1.0, 2.0]]).is_err());
    }

    #[test]
    fn linear_regression_perfect_fit() {
        // y = 2 + 3x → coefficients should be [2.0, 3.0].
        let x: Vec<Vec<f64>> = (0..10).map(|i| vec![i as f64]).collect();
        let y: Vec<f64> = x.iter().map(|r| 2.0 + 3.0 * r[0]).collect();
        let r = linear_regression(&x, &y).unwrap();
        assert!((r.coefficients[0] - 2.0).abs() < 1e-8, "intercept");
        assert!((r.coefficients[1] - 3.0).abs() < 1e-8, "slope");
        assert!((r.r_squared - 1.0).abs() < 1e-8);
    }

    #[test]
    fn linear_regression_r_squared_below_one_for_noisy() {
        let x: Vec<Vec<f64>> = (0..20).map(|i| vec![i as f64]).collect();
        let y: Vec<f64> = x.iter().enumerate()
            .map(|(i, r)| r[0] + (i % 3) as f64 * 0.5)
            .collect();
        let r = linear_regression(&x, &y).unwrap();
        assert!(r.r_squared < 1.0 && r.r_squared > 0.9);
    }

    #[test]
    fn uncertainty_propagation_pythagoras() {
        // u_a = 3, u_b = 4, both c=1 → combined = 5.
        let u = propagate_uncertainty(&[(3.0, 1.0), (4.0, 1.0)]);
        assert!((u - 5.0).abs() < 1e-10);
    }

    #[test]
    fn uncertainty_sensitivity_scaling() {
        // u = 2, c = 3 → contribution = 6, u_combined = 6.
        let u = propagate_uncertainty(&[(2.0, 3.0)]);
        assert!((u - 6.0).abs() < 1e-10);
    }

    // ── Two-way ANOVA ─────────────────────────────────────────────────────────

    /// 2×2 balanced design, purely additive (no interaction).
    ///
    /// A1B1=[1,2,3], A1B2=[4,5,6], A2B1=[7,8,9], A2B2=[10,11,12]
    /// Hand-computed: SS_A=108, SS_B=27, SS_AB=0, SS_within=8
    ///   MS_within=1  →  F_A=108, F_B=27, F_AB=0
    #[test]
    fn two_way_anova_additive_design() {
        let data = vec![
            vec![vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]],
            vec![vec![7.0, 8.0, 9.0], vec![10.0, 11.0, 12.0]],
        ];
        let r = anova_two_way(&data).unwrap();

        assert_eq!(r.levels_a, 2);
        assert_eq!(r.levels_b, 2);
        assert_eq!(r.n_per_cell, 3);
        assert!((r.grand_mean - 6.5).abs() < 1e-10, "grand mean");

        let fa  = r.table[0].f_statistic.unwrap();
        let fb  = r.table[1].f_statistic.unwrap();
        let fab = r.table[2].f_statistic.unwrap();
        assert!((fa  - 108.0).abs() < 1e-8, "F_A expected 108, got {fa}");
        assert!((fb  -  27.0).abs() < 1e-8, "F_B expected 27, got {fb}");
        assert!(fab < 1e-10, "F_AB should be ~0 (no interaction), got {fab}");

        assert!(r.table[0].p_value.unwrap() < 0.001, "p_A should be very small");
        assert!(r.table[1].p_value.unwrap() < 0.001, "p_B should be very small");
        assert!(r.table[2].p_value.unwrap() > 0.5,   "p_AB should be large (no interaction)");

        // SS checks.
        assert!((r.table[0].ss - 108.0).abs() < 1e-8, "SS_A");
        assert!((r.table[1].ss -  27.0).abs() < 1e-8, "SS_B");
        assert!(r.table[2].ss < 1e-10, "SS_AB");
        assert!((r.table[3].ss -   8.0).abs() < 1e-8, "SS_within");
    }

    /// 2×2 balanced design with a significant interaction.
    ///
    /// A1B1=[1,2,3], A1B2=[8,9,10], A2B1=[7,8,9], A2B2=[8,9,10]
    /// Hand-computed: SS_A=27, SS_B=48, SS_AB=27, SS_within=8
    ///   MS_within=1  →  F_A=27, F_B=48, F_AB=27
    #[test]
    fn two_way_anova_with_interaction() {
        let data = vec![
            vec![vec![1.0, 2.0, 3.0], vec![8.0, 9.0, 10.0]],
            vec![vec![7.0, 8.0, 9.0], vec![8.0, 9.0, 10.0]],
        ];
        let r = anova_two_way(&data).unwrap();

        let fa  = r.table[0].f_statistic.unwrap();
        let fb  = r.table[1].f_statistic.unwrap();
        let fab = r.table[2].f_statistic.unwrap();
        assert!((fa  - 27.0).abs() < 1e-8, "F_A");
        assert!((fb  - 48.0).abs() < 1e-8, "F_B");
        assert!((fab - 27.0).abs() < 1e-8, "F_AB");

        assert!(r.table[0].p_value.unwrap() < 0.01);
        assert!(r.table[1].p_value.unwrap() < 0.001);
        assert!(r.table[2].p_value.unwrap() < 0.01);
    }

    #[test]
    fn two_way_anova_cell_means_correct() {
        let data = vec![
            vec![vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]],
            vec![vec![7.0, 8.0, 9.0], vec![10.0, 11.0, 12.0]],
        ];
        let r = anova_two_way(&data).unwrap();
        assert!((r.cell_means[0][0] - 2.0).abs()  < 1e-10);
        assert!((r.cell_means[0][1] - 5.0).abs()  < 1e-10);
        assert!((r.cell_means[1][0] - 8.0).abs()  < 1e-10);
        assert!((r.cell_means[1][1] - 11.0).abs() < 1e-10);
    }

    #[test]
    fn two_way_anova_requires_two_a_levels() {
        let data = vec![vec![vec![1.0, 2.0], vec![3.0, 4.0]]];
        assert!(anova_two_way(&data).is_err());
    }

    #[test]
    fn two_way_anova_requires_two_b_levels() {
        let data = vec![
            vec![vec![1.0, 2.0]],
            vec![vec![3.0, 4.0]],
        ];
        assert!(anova_two_way(&data).is_err());
    }

    #[test]
    fn two_way_anova_rejects_unbalanced() {
        let data = vec![
            vec![vec![1.0, 2.0, 3.0], vec![4.0, 5.0]],  // B1=3 obs, B2=2 obs
            vec![vec![7.0, 8.0, 9.0], vec![10.0, 11.0, 12.0]],
        ];
        assert!(anova_two_way(&data).is_err());
    }

    #[test]
    fn two_way_anova_rejects_single_obs_per_cell() {
        let data = vec![
            vec![vec![1.0], vec![4.0]],
            vec![vec![7.0], vec![10.0]],
        ];
        assert!(anova_two_way(&data).is_err());
    }
}
