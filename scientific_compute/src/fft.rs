//! Pure-Rust FFT primitives built on `rustfft`.
//!
//! Provides forward and inverse FFT, plus a convenience function
//! for computing the power spectrum of a real-valued signal.

use num_complex::Complex;
use rustfft::FftPlanner;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FftError {
    #[error("input buffer is empty")]
    EmptyInput,
}

/// Compute the forward FFT of a complex signal **in-place** and return the result.
pub fn forward(signal: &[Complex<f64>]) -> Result<Vec<Complex<f64>>, FftError> {
    if signal.is_empty() {
        return Err(FftError::EmptyInput);
    }
    let mut buf = signal.to_vec();
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(buf.len());
    fft.process(&mut buf);
    Ok(buf)
}

/// Compute the inverse FFT of a frequency-domain signal.
///
/// The output is scaled by `1/N` so that `inverse(forward(x)) ≈ x`.
pub fn inverse(spectrum: &[Complex<f64>]) -> Result<Vec<Complex<f64>>, FftError> {
    if spectrum.is_empty() {
        return Err(FftError::EmptyInput);
    }
    let mut buf = spectrum.to_vec();
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_inverse(buf.len());
    fft.process(&mut buf);

    let n = buf.len() as f64;
    for c in &mut buf {
        *c /= n;
    }
    Ok(buf)
}

/// Compute the power spectral density of a **real-valued** signal.
///
/// Returns `|X[k]|²` for each frequency bin.
pub fn power_spectrum(signal: &[f64]) -> Result<Vec<f64>, FftError> {
    let complex_signal: Vec<Complex<f64>> =
        signal.iter().map(|&r| Complex::new(r, 0.0)).collect();
    let spectrum = forward(&complex_signal)?;
    Ok(spectrum.iter().map(|c| c.norm_sqr()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let original: Vec<Complex<f64>> = (0..64)
            .map(|i| Complex::new(i as f64, 0.0))
            .collect();
        let freq = forward(&original).unwrap();
        let recovered = inverse(&freq).unwrap();
        for (a, b) in original.iter().zip(recovered.iter()) {
            assert!((a.re - b.re).abs() < 1e-9, "real mismatch");
            assert!((a.im - b.im).abs() < 1e-9, "imag mismatch");
        }
    }

    #[test]
    fn power_spectrum_dc() {
        // Constant signal → all energy in bin 0
        let signal = vec![1.0; 8];
        let ps = power_spectrum(&signal).unwrap();
        assert!((ps[0] - 64.0).abs() < 1e-9); // 8^2
        for &p in &ps[1..] {
            assert!(p < 1e-9);
        }
    }

    #[test]
    fn reject_empty() {
        assert!(forward(&[]).is_err());
        assert!(inverse(&[]).is_err());
        assert!(power_spectrum(&[]).is_err());
    }
}
