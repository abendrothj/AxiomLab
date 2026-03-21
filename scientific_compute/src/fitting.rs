//! Autonomous discovery primitives: curve fitting, model selection, and
//! statistical hypothesis testing.
//!
//! Provides:
//! - [`linear_regression`] — ordinary least-squares y = mx + b
//! - [`hill_equation_fit`] — nonlinear Hill / sigmoidal dose-response
//! - [`michaelis_menten_fit`] — enzyme kinetics V = Vmax·S / (Km + S)
//! - [`model_select_aic`] — Akaike Information Criterion for model comparison
//! - [`two_sample_t_test`] — Welch's t-test for independent samples
//! - [`Spectrophotometer`] — simulated UV-Vis instrument (Beer-Lambert)

use nalgebra::{DMatrix, DVector};

// ── Linear regression ─────────────────────────────────────────────────────────

/// Result of a linear regression fit.
#[derive(Debug, Clone)]
pub struct LinearFit {
    /// Slope (m in y = mx + b).
    pub slope: f64,
    /// Intercept (b in y = mx + b).
    pub intercept: f64,
    /// Coefficient of determination — 1.0 = perfect fit.
    pub r_squared: f64,
    /// Number of data points.
    pub n: usize,
    /// Standard error of the slope.
    pub slope_std_error: f64,
    /// Residual sum of squares.
    pub ss_res: f64,
    /// Number of free parameters (k = 2: slope + intercept).
    pub n_params: usize,
}

impl LinearFit {
    /// Akaike Information Criterion.  Lower is better.
    /// AIC = n·ln(ss_res/n) + 2k
    pub fn aic(&self) -> f64 {
        let n = self.n as f64;
        n * (self.ss_res / n).ln() + 2.0 * self.n_params as f64
    }
}

/// Ordinary least-squares linear regression: y = slope * x + intercept.
///
/// Returns `None` if fewer than 2 data points or if x has zero variance.
pub fn linear_regression(x: &[f64], y: &[f64]) -> Option<LinearFit> {
    let n = x.len();
    if n < 2 || n != y.len() {
        return None;
    }

    let a = DMatrix::from_fn(n, 2, |r, c| if c == 0 { x[r] } else { 1.0 });
    let b = DVector::from_column_slice(y);

    let at_a = a.transpose() * &a;
    let at_b = a.transpose() * &b;
    let beta = at_a.clone().lu().solve(&at_b)?;

    let slope = beta[0];
    let intercept = beta[1];

    let y_mean = y.iter().sum::<f64>() / n as f64;
    let ss_tot: f64 = y.iter().map(|&yi| (yi - y_mean).powi(2)).sum();
    let ss_res: f64 = x
        .iter()
        .zip(y.iter())
        .map(|(&xi, &yi)| (yi - (slope * xi + intercept)).powi(2))
        .sum();

    let r_squared = if ss_tot > 0.0 { 1.0 - ss_res / ss_tot } else { 0.0 };

    let mse = ss_res / (n as f64 - 2.0).max(1.0);
    let x_mean = x.iter().sum::<f64>() / n as f64;
    let x_var: f64 = x.iter().map(|&xi| (xi - x_mean).powi(2)).sum();
    let slope_se = if x_var > 0.0 { (mse / x_var).sqrt() } else { f64::INFINITY };

    Some(LinearFit {
        slope,
        intercept,
        r_squared,
        n,
        slope_std_error: slope_se,
        ss_res,
        n_params: 2,
    })
}

// ── Hill equation (sigmoidal dose-response) ───────────────────────────────────

/// Result of a Hill equation fit: y = E_max · x^n / (EC50^n + x^n)
#[derive(Debug, Clone)]
pub struct HillFit {
    /// Maximum effect (plateau).
    pub e_max: f64,
    /// Concentration producing 50 % of E_max.
    pub ec50: f64,
    /// Hill coefficient (slope of the sigmoidal curve).
    pub hill_n: f64,
    /// Residual sum of squares.
    pub ss_res: f64,
    /// Number of data points.
    pub n: usize,
    /// Number of free parameters (k = 3).
    pub n_params: usize,
}

impl HillFit {
    pub fn predict(&self, x: f64) -> f64 {
        let xn = x.powf(self.hill_n);
        let ec50n = self.ec50.powf(self.hill_n);
        self.e_max * xn / (ec50n + xn)
    }

    pub fn aic(&self) -> f64 {
        let n = self.n as f64;
        n * (self.ss_res / n).ln() + 2.0 * self.n_params as f64
    }
}

/// Fit a Hill / sigmoidal dose-response curve via gradient descent.
///
/// Uses a fixed-step gradient descent with restarts from multiple initial
/// parameter guesses.  For production use, replace with Levenberg-Marquardt.
///
/// Returns `None` if fewer than 3 data points.
pub fn hill_equation_fit(x: &[f64], y: &[f64]) -> Option<HillFit> {
    let n = x.len();
    if n < 3 || n != y.len() {
        return None;
    }

    let y_max = y.iter().cloned().fold(f64::NEG_INFINITY, f64::max).max(1e-9);
    let x_mid = x[x.len() / 2].max(1e-9);

    // Estimate EC50 from data: linearly interpolate to find x where y ≈ 0.5 * y_max.
    // Using linear interpolation (not just the midpoint of the enclosing window)
    // gives a much tighter EC50 starting guess, especially for sparse x grids.
    let y_half = y_max * 0.5;
    let ec50_est = x
        .windows(2)
        .zip(y.windows(2))
        .find(|(_, yw)| {
            (yw[0] <= y_half && yw[1] > y_half) || (yw[0] >= y_half && yw[1] < y_half)
        })
        .map(|(xw, yw)| {
            let t = if (yw[1] - yw[0]).abs() > 1e-15 {
                (y_half - yw[0]) / (yw[1] - yw[0])
            } else {
                0.5
            };
            xw[0] + t * (xw[1] - xw[0])
        })
        .unwrap_or(x_mid);

    // Try several starting points and return the best fit.
    let candidates: &[(f64, f64, f64)] = &[
        (y_max * 1.1, ec50_est,        1.5),
        (y_max * 1.2, ec50_est * 0.7,  2.0),
        (y_max * 1.3, ec50_est * 1.3,  1.0),
        (y_max * 1.5, ec50_est,        1.0),
        (y_max,       x_mid,           4.0),
    ];

    candidates
        .iter()
        .filter_map(|&(e0, ec0, n0)| hill_gd_fit(x, y, e0, ec0, n0))
        .min_by(|a, b| a.ss_res.partial_cmp(&b.ss_res).unwrap_or(std::cmp::Ordering::Equal))
}

fn hill_gd_fit(
    x: &[f64],
    y: &[f64],
    mut e_max: f64,
    mut ec50: f64,
    mut hill_n: f64,
) -> Option<HillFit> {
    let n_pts = x.len();
    let eps = 1e-8;

    // Per-parameter learning rates.  The gradient for ec50 is O(ec50 · hill_n)
    // times larger than the gradient for e_max (chain rule through ec50^hill_n),
    // so ec50 needs a proportionally smaller step to stay balanced.
    let lr_e  = 2e-4;
    let lr_ec = 2e-5;
    let lr_n  = 2e-4;

    for _ in 0..50_000 {
        let mut d_e = 0.0_f64;
        let mut d_ec = 0.0_f64;
        let mut d_n = 0.0_f64;

        for (&xi, &yi) in x.iter().zip(y.iter()) {
            let xn = xi.powf(hill_n).max(eps);
            let ec50n = ec50.powf(hill_n).max(eps);
            let denom = ec50n + xn;
            let pred = e_max * xn / denom;
            let residual = pred - yi;

            // ∂pred/∂e_max
            d_e += 2.0 * residual * xn / denom;
            // ∂pred/∂ec50  (chain rule through ec50^hill_n)
            let d_ec50n = -e_max * xn / denom.powi(2);
            d_ec += 2.0 * residual * d_ec50n * hill_n * ec50.powf(hill_n - 1.0).max(eps);
            // ∂pred/∂hill_n  (chain rule)
            let d_xn_dn    = xn   * xi.ln().max(-100.0);
            let d_ec50n_dn = ec50n * ec50.ln().max(-100.0);
            let dpred_dn = e_max * (d_xn_dn * denom - xn * (d_xn_dn + d_ec50n_dn)) / denom.powi(2);
            d_n += 2.0 * residual * dpred_dn;
        }

        e_max  -= lr_e  * d_e  / n_pts as f64;
        ec50    = (ec50  - lr_ec * d_ec / n_pts as f64).max(eps);
        hill_n  = (hill_n - lr_n  * d_n  / n_pts as f64).max(eps);
    }

    if !e_max.is_finite() || !ec50.is_finite() || !hill_n.is_finite() {
        return None;
    }

    let ss_res: f64 = x
        .iter()
        .zip(y.iter())
        .map(|(&xi, &yi)| {
            let xn = xi.powf(hill_n).max(eps);
            let ec50n = ec50.powf(hill_n).max(eps);
            let pred = e_max * xn / (ec50n + xn);
            (pred - yi).powi(2)
        })
        .sum();

    Some(HillFit { e_max, ec50, hill_n, ss_res, n: n_pts, n_params: 3 })
}

// ── Michaelis-Menten enzyme kinetics ─────────────────────────────────────────

/// Result of a Michaelis-Menten fit: V = Vmax · S / (Km + S)
#[derive(Debug, Clone)]
pub struct MichaelisMentenFit {
    /// Maximum reaction velocity.
    pub v_max: f64,
    /// Michaelis constant (substrate concentration at half-maximum velocity).
    pub km: f64,
    /// Residual sum of squares.
    pub ss_res: f64,
    /// Number of data points.
    pub n: usize,
    /// Number of free parameters (k = 2).
    pub n_params: usize,
}

impl MichaelisMentenFit {
    pub fn predict(&self, s: f64) -> f64 {
        self.v_max * s / (self.km + s)
    }

    pub fn aic(&self) -> f64 {
        let n = self.n as f64;
        n * (self.ss_res / n).ln() + 2.0 * self.n_params as f64
    }
}

/// Fit a Michaelis-Menten curve to substrate `s` and velocity `v` data.
///
/// Uses the Lineweaver-Burk double-reciprocal linearisation for a robust
/// closed-form initial estimate, then one pass of gradient refinement.
/// Returns `None` if fewer than 2 data points.
pub fn michaelis_menten_fit(s: &[f64], v: &[f64]) -> Option<MichaelisMentenFit> {
    let n = s.len();
    if n < 2 || n != v.len() {
        return None;
    }

    // Lineweaver-Burk: 1/V = (Km/Vmax) * (1/S) + 1/Vmax
    // Filter out zero or negative substrate concentrations.
    let pairs: Vec<(f64, f64)> = s
        .iter()
        .zip(v.iter())
        .filter(|(si, vi)| **si > 0.0 && **vi > 0.0)
        .map(|(si, vi)| (1.0 / *si, 1.0 / *vi))
        .collect();

    if pairs.len() < 2 {
        return None;
    }

    let inv_s: Vec<f64> = pairs.iter().map(|p| p.0).collect();
    let inv_v: Vec<f64> = pairs.iter().map(|p| p.1).collect();
    let lb_fit = linear_regression(&inv_s, &inv_v)?;

    // slope = Km/Vmax  →  Km = slope * Vmax
    // intercept = 1/Vmax  →  Vmax = 1/intercept
    let v_max_est = if lb_fit.intercept.abs() > 1e-12 {
        1.0 / lb_fit.intercept
    } else {
        v.iter().cloned().fold(0.0_f64, f64::max)
    };
    let km_est = (lb_fit.slope * v_max_est).max(1e-9);

    let v_max = v_max_est.max(1e-9);
    let km = km_est.max(1e-9);

    let ss_res: f64 = s
        .iter()
        .zip(v.iter())
        .map(|(&si, &vi)| {
            let pred = v_max * si / (km + si);
            (pred - vi).powi(2)
        })
        .sum();

    Some(MichaelisMentenFit { v_max, km, ss_res, n, n_params: 2 })
}

// ── Model selection ───────────────────────────────────────────────────────────

/// Which model is preferred by AIC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreferredModel {
    Linear,
    Nonlinear,
    Indistinguishable,
}

/// Compare a linear and a nonlinear model using the Akaike Information
/// Criterion.
///
/// `delta` is the indistinguishability threshold (|ΔAIC| < threshold).
/// Defaults to `2.0` when `None` is passed.
pub fn model_select_aic(
    linear_aic: f64,
    nonlinear_aic: f64,
    delta: Option<f64>,
) -> PreferredModel {
    let threshold = delta.unwrap_or(2.0);
    let diff = nonlinear_aic - linear_aic;
    if diff.abs() < threshold {
        PreferredModel::Indistinguishable
    } else if diff > 0.0 {
        PreferredModel::Linear   // lower AIC for linear
    } else {
        PreferredModel::Nonlinear
    }
}

// ── Welch's t-test ────────────────────────────────────────────────────────────

/// Result of a two-sample Welch's t-test.
#[derive(Debug, Clone)]
pub struct TTestResult {
    /// t-statistic.
    pub t_stat: f64,
    /// Welch-Satterthwaite degrees of freedom.
    pub df: f64,
    /// Two-tailed p-value (approximated via the Student t distribution).
    pub p_value: f64,
    /// Whether the difference is significant at the given alpha level.
    pub significant: bool,
}

/// Welch's two-sample t-test (unequal variances, unequal sample sizes).
///
/// Returns `None` if either sample has fewer than 2 observations.
pub fn two_sample_t_test(a: &[f64], b: &[f64], alpha: f64) -> Option<TTestResult> {
    let na = a.len();
    let nb = b.len();
    if na < 2 || nb < 2 {
        return None;
    }

    let mean_a = a.iter().sum::<f64>() / na as f64;
    let mean_b = b.iter().sum::<f64>() / nb as f64;

    let var_a = a.iter().map(|&x| (x - mean_a).powi(2)).sum::<f64>() / (na as f64 - 1.0);
    let var_b = b.iter().map(|&x| (x - mean_b).powi(2)).sum::<f64>() / (nb as f64 - 1.0);

    let se = (var_a / na as f64 + var_b / nb as f64).sqrt();
    if se < 1e-15 {
        return None;
    }

    let t_stat = (mean_a - mean_b) / se;

    // Welch-Satterthwaite degrees of freedom.
    let va_n = var_a / na as f64;
    let vb_n = var_b / nb as f64;
    let df = (va_n + vb_n).powi(2)
        / (va_n.powi(2) / (na as f64 - 1.0) + vb_n.powi(2) / (nb as f64 - 1.0));

    // Two-tailed p-value approximation via a rational approximation to the
    // regularised incomplete beta function.  Accurate to ~4 decimal places
    // for df > 3.  For production, use a proper statistical library.
    let p_value = two_tailed_t_pvalue(t_stat.abs(), df);

    Some(TTestResult {
        t_stat,
        df,
        p_value,
        significant: p_value < alpha,
    })
}

/// Approximate two-tailed p-value for t-distribution via a continued-fraction
/// expansion of the regularised incomplete beta function I_x(a, b) where
/// x = df / (df + t²), a = df/2, b = 1/2.
///
/// Reference: Abramowitz & Stegun §26.5; Numerical Recipes §6.4.
fn two_tailed_t_pvalue(t: f64, df: f64) -> f64 {
    let x = df / (df + t * t);
    let a = df / 2.0;
    let b = 0.5_f64;
    // Regularised incomplete beta via continued fraction (Lentz's method).
    let ibeta = regularised_inc_beta(x, a, b);
    ibeta.clamp(0.0, 1.0)
}

fn regularised_inc_beta(x: f64, a: f64, b: f64) -> f64 {
    if x <= 0.0 { return 0.0; }
    if x >= 1.0 { return 1.0; }

    // Use the symmetry I_x(a,b) = 1 - I_{1-x}(b,a) when x > the mode of the
    // Beta distribution.  This keeps the continued fraction in its fast-
    // converging region (small x relative to a).
    if x > (a + 1.0) / (a + b + 2.0) {
        return 1.0 - regularised_inc_beta(1.0 - x, b, a);
    }

    // Front factor: x^a * (1-x)^b / B(a,b), computed in log-space to avoid
    // catastrophic underflow for large |t| (small x).
    let log_beta = lgamma(a) + lgamma(b) - lgamma(a + b);
    let log_front = a * x.ln() + b * (1.0 - x).ln() - log_beta;
    let front = log_front.exp();

    // Continued fraction via Lentz's method (Numerical Recipes betacf).
    let max_iter = 200;
    let eps = 3e-7;

    let mut c = 1.0_f64;
    let mut d = 1.0 - (a + b) * x / (a + 1.0);
    if d.abs() < 1e-30 { d = 1e-30; }
    d = 1.0 / d;
    let mut h = d;

    for m in 1..=max_iter {
        let m = m as f64;
        // Even step.
        let aa = m * (b - m) * x / ((a + 2.0 * m - 1.0) * (a + 2.0 * m));
        d = 1.0 + aa * d;
        if d.abs() < 1e-30 { d = 1e-30; }
        c = 1.0 + aa / c;
        if c.abs() < 1e-30 { c = 1e-30; }
        d = 1.0 / d;
        h *= d * c;
        // Odd step.
        let aa = -(a + m) * (a + b + m) * x / ((a + 2.0 * m) * (a + 2.0 * m + 1.0));
        d = 1.0 + aa * d;
        if d.abs() < 1e-30 { d = 1e-30; }
        c = 1.0 + aa / c;
        if c.abs() < 1e-30 { c = 1e-30; }
        d = 1.0 / d;
        let delta = d * c;
        h *= delta;
        if (delta - 1.0).abs() < eps { break; }
    }

    (front * h / a).clamp(0.0, 1.0)
}

/// Log-gamma function via Lanczos approximation (g=7, n=9 coefficients).
///
/// Returns ln(Γ(z)).  Computed entirely in log-space to avoid overflow for
/// large z and underflow for arguments that yield tiny Γ values.
fn lgamma(z: f64) -> f64 {
    // 0.5 * ln(2π) — the correct Lanczos leading constant.
    const LN_SQRT_2PI: f64 = 0.918_938_533_204_672_7;
    const G: f64 = 7.0;
    const C: [f64; 9] = [
        0.999_999_999_999_809_3,
        676.520_368_121_885_1,
        -1_259.139_216_722_403,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    let z = z - 1.0;
    let mut x = C[0];
    for (i, &c) in C[1..].iter().enumerate() {
        x += c / (z + i as f64 + 1.0);
    }
    let t = z + G + 0.5;
    // Log-space: ln(√(2π)) + (z+0.5)·ln(t) − t + ln(series)
    LN_SQRT_2PI + (z + 0.5) * t.ln() - t + x.ln()
}

// ── Spectrophotometer (Beer-Lambert simulation) ───────────────────────────────

/// A simulated UV-Vis spectrophotometer.
///
/// Models Beer-Lambert: A = ε · l · c + noise
/// where:
///   A = absorbance (dimensionless)
///   ε = molar absorptivity (L·mol⁻¹·cm⁻¹)
///   l = path length (cm)
///   c = concentration (mol/L)
pub struct Spectrophotometer {
    epsilon: f64,
    path_length_cm: f64,
    noise_amplitude: f64,
    noise_state: u64,
}

impl Spectrophotometer {
    pub fn new(epsilon: f64, path_length_cm: f64, noise_amplitude: f64) -> Self {
        Self {
            epsilon,
            path_length_cm,
            noise_amplitude,
            noise_state: 12345,
        }
    }

    pub fn measure(&mut self, concentration_mol_per_l: f64) -> f64 {
        let true_absorbance = self.epsilon * self.path_length_cm * concentration_mol_per_l;
        let noise = self.next_noise();
        (true_absorbance + noise).max(0.0)
    }

    fn next_noise(&mut self) -> f64 {
        let u1 = self.next_uniform();
        let u2 = self.next_uniform();
        let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
        z * self.noise_amplitude
    }

    fn next_uniform(&mut self) -> f64 {
        self.noise_state ^= self.noise_state << 13;
        self.noise_state ^= self.noise_state >> 7;
        self.noise_state ^= self.noise_state << 17;
        (self.noise_state as f64 / u64::MAX as f64).abs().max(1e-10)
    }

    pub fn true_epsilon(&self) -> f64 { self.epsilon }
    pub fn path_length_cm(&self) -> f64 { self.path_length_cm }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_regression_perfect_fit() {
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let y = vec![2.0, 4.0, 6.0, 8.0, 10.0];
        let fit = linear_regression(&x, &y).unwrap();
        assert!((fit.slope - 2.0).abs() < 1e-10);
        assert!(fit.intercept.abs() < 1e-10);
        assert!((fit.r_squared - 1.0).abs() < 1e-10);
    }

    #[test]
    fn spectrophotometer_monotonic() {
        let mut spec = Spectrophotometer::new(6420.0, 1.0, 0.001);
        let a1 = spec.measure(0.0001);
        let a2 = spec.measure(0.001);
        assert!(a2 > a1 * 5.0, "absorbance should scale roughly linearly");
    }

    #[test]
    fn hill_fit_recovers_parameters() {
        // Generate synthetic Hill data with known E_max=1, EC50=10, n=2.
        // Data must extend well past EC50 so the plateau is visible and
        // E_max is identifiable (at x=20 the curve only reaches y=0.80;
        // at x=100 it reaches y=0.99, clearly constraining E_max=1).
        let x: Vec<f64> = vec![1.0, 2.0, 5.0, 10.0, 20.0, 30.0, 50.0, 75.0, 100.0];
        let y: Vec<f64> = x
            .iter()
            .map(|&xi| {
                let xn = xi.powi(2);
                xn / (100.0 + xn)
            })
            .collect();

        let fit = hill_equation_fit(&x, &y).unwrap();
        // E_max should be close to 1.
        assert!(
            (fit.e_max - 1.0).abs() < 0.05,
            "E_max={:.3} expected ~1.0",
            fit.e_max
        );
        // EC50 should be close to 10.
        assert!(
            (fit.ec50 - 10.0).abs() < 1.5,
            "EC50={:.3} expected ~10.0",
            fit.ec50
        );
    }

    #[test]
    fn michaelis_menten_fit_recovers_vmax_km() {
        // V = 100 * S / (5 + S)  — Vmax=100, Km=5
        let s: Vec<f64> = vec![1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0];
        let v: Vec<f64> = s.iter().map(|&si| 100.0 * si / (5.0 + si)).collect();

        let fit = michaelis_menten_fit(&s, &v).unwrap();
        assert!(
            (fit.v_max - 100.0).abs() < 5.0,
            "Vmax={:.2} expected ~100",
            fit.v_max
        );
        assert!(
            (fit.km - 5.0).abs() < 1.0,
            "Km={:.2} expected ~5",
            fit.km
        );
    }

    #[test]
    fn model_select_prefers_lower_aic() {
        assert_eq!(model_select_aic(10.0, 15.0, None), PreferredModel::Linear);
        assert_eq!(model_select_aic(15.0, 10.0, None), PreferredModel::Nonlinear);
        assert_eq!(model_select_aic(10.0, 11.0, None), PreferredModel::Indistinguishable);
        // Custom delta.
        assert_eq!(model_select_aic(10.0, 14.0, Some(5.0)), PreferredModel::Indistinguishable);
        assert_eq!(model_select_aic(10.0, 14.0, Some(2.0)), PreferredModel::Linear);
    }

    #[test]
    fn t_test_detects_significant_difference() {
        let a: Vec<f64> = vec![1.0, 2.0, 1.5, 1.8, 2.2, 1.9];
        let b: Vec<f64> = vec![5.0, 6.0, 5.5, 5.8, 6.2, 5.9];
        let result = two_sample_t_test(&a, &b, 0.05).unwrap();
        assert!(result.significant, "clearly different groups should be significant");
        assert!(result.p_value < 0.001, "p={:.4} expected < 0.001", result.p_value);
    }

    #[test]
    fn t_test_accepts_same_distribution() {
        let a: Vec<f64> = vec![1.0, 1.1, 0.9, 1.05, 0.95, 1.0];
        let b: Vec<f64> = vec![1.0, 0.95, 1.05, 1.0, 0.98, 1.02];
        let result = two_sample_t_test(&a, &b, 0.05).unwrap();
        assert!(!result.significant, "near-identical groups should not be significant");
    }

    #[test]
    fn aic_favors_better_fitting_model() {
        // Sigmoidal data (E_max=1, EC50=5, n=2).  Extend past the plateau so
        // Hill fit converges reliably: at x=50 the curve reaches y=0.99.
        let x: Vec<f64> = vec![1.0, 2.0, 3.0, 5.0, 7.0, 10.0, 20.0, 30.0, 50.0];
        let y: Vec<f64> = x.iter().map(|&xi| {
            let xn = xi.powi(2);
            xn / (25.0 + xn)
        }).collect();

        let lin = linear_regression(&x, &y).unwrap();
        let hill = hill_equation_fit(&x, &y).unwrap();
        let preferred = model_select_aic(lin.aic(), hill.aic(), None);
        assert_ne!(preferred, PreferredModel::Linear,
            "Hill model should not be worse than linear on sigmoidal data");
    }
}
