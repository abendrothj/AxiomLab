//! Simulated spectrophotometer and linear regression for autonomous discovery.
//!
//! Provides:
//! - [`Spectrophotometer`] — simulates a UV-Vis instrument with realistic noise
//! - [`linear_regression`] — ordinary least-squares fit for y = mx + b
//! - [`LinearFit`] — fit result with slope, intercept, R², residuals

use nalgebra::{DMatrix, DVector};

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
}

/// Ordinary least-squares linear regression: y = slope * x + intercept.
///
/// Returns `None` if fewer than 2 data points or if x has zero variance.
pub fn linear_regression(x: &[f64], y: &[f64]) -> Option<LinearFit> {
    let n = x.len();
    if n < 2 || n != y.len() {
        return None;
    }

    // Build the design matrix [x, 1] for y = mx + b
    let a = DMatrix::from_fn(n, 2, |r, c| if c == 0 { x[r] } else { 1.0 });
    let b = DVector::from_column_slice(y);

    // Normal equations: A^T A β = A^T b
    let at_a = a.transpose() * &a;
    let at_b = a.transpose() * &b;

    let beta = at_a.clone().lu().solve(&at_b)?;

    let slope = beta[0];
    let intercept = beta[1];

    // Compute R²
    let y_mean = y.iter().sum::<f64>() / n as f64;
    let ss_tot: f64 = y.iter().map(|&yi| (yi - y_mean).powi(2)).sum();
    let ss_res: f64 = x.iter().zip(y.iter())
        .map(|(&xi, &yi)| (yi - (slope * xi + intercept)).powi(2))
        .sum();

    let r_squared = if ss_tot > 0.0 { 1.0 - ss_res / ss_tot } else { 0.0 };

    // Standard error of slope
    let mse = ss_res / (n as f64 - 2.0).max(1.0);
    let x_var: f64 = x.iter().map(|&xi| (xi - x.iter().sum::<f64>() / n as f64).powi(2)).sum();
    let slope_se = if x_var > 0.0 { (mse / x_var).sqrt() } else { f64::INFINITY };

    Some(LinearFit {
        slope,
        intercept,
        r_squared,
        n,
        slope_std_error: slope_se,
    })
}

/// A simulated UV-Vis spectrophotometer.
///
/// Models Beer-Lambert: A = ε · l · c + noise
/// where:
///   A = absorbance (dimensionless)
///   ε = molar absorptivity (L·mol⁻¹·cm⁻¹)
///   l = path length (cm)
///   c = concentration (mol/L)
pub struct Spectrophotometer {
    /// Molar absorptivity in L·mol⁻¹·cm⁻¹ (the "secret" to discover).
    epsilon: f64,
    /// Cuvette path length in cm.
    path_length_cm: f64,
    /// Measurement noise standard deviation.
    noise_amplitude: f64,
    /// Simple deterministic noise seed (no rand dependency).
    noise_state: u64,
}

impl Spectrophotometer {
    /// Create a new spectrophotometer.
    ///
    /// `epsilon` is the molar absorptivity — the physical constant the
    /// agent should discover from data.
    pub fn new(epsilon: f64, path_length_cm: f64, noise_amplitude: f64) -> Self {
        Self {
            epsilon,
            path_length_cm,
            noise_amplitude,
            noise_state: 12345,
        }
    }

    /// Measure absorbance at a given concentration (mol/L).
    ///
    /// Returns A = ε·l·c + gaussian noise.
    pub fn measure(&mut self, concentration_mol_per_l: f64) -> f64 {
        let true_absorbance = self.epsilon * self.path_length_cm * concentration_mol_per_l;
        let noise = self.next_noise();
        (true_absorbance + noise).max(0.0) // absorbance can't be negative
    }

    /// Simple deterministic pseudo-gaussian noise using xorshift + Box-Muller.
    fn next_noise(&mut self) -> f64 {
        let u1 = self.next_uniform();
        let u2 = self.next_uniform();
        // Box-Muller transform
        let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
        z * self.noise_amplitude
    }

    fn next_uniform(&mut self) -> f64 {
        // xorshift64
        self.noise_state ^= self.noise_state << 13;
        self.noise_state ^= self.noise_state >> 7;
        self.noise_state ^= self.noise_state << 17;
        // Map to (0, 1)
        (self.noise_state as f64 / u64::MAX as f64).abs().max(1e-10)
    }

    /// The true molar absorptivity (for validation — the agent doesn't see this).
    pub fn true_epsilon(&self) -> f64 {
        self.epsilon
    }

    /// The path length in cm.
    pub fn path_length_cm(&self) -> f64 {
        self.path_length_cm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_regression_perfect_fit() {
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let y = vec![2.0, 4.0, 6.0, 8.0, 10.0]; // y = 2x
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
        // Higher concentration should give higher absorbance (with small noise)
        assert!(a2 > a1 * 5.0, "absorbance should scale roughly linearly");
    }
}
