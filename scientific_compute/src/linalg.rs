//! Linear-algebra helpers wrapping `nalgebra` for common lab operations.

use nalgebra::{DMatrix, DVector};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LinalgError {
    #[error("dimension mismatch: expected {expected}, got {got}")]
    DimensionMismatch { expected: usize, got: usize },
    #[error("matrix is singular and cannot be inverted")]
    SingularMatrix,
}

/// Solve `A * x = b` for dense systems, returning an error if `A` is singular.
pub fn solve_linear(a: &DMatrix<f64>, b: &DVector<f64>) -> Result<DVector<f64>, LinalgError> {
    if a.nrows() != b.len() {
        return Err(LinalgError::DimensionMismatch {
            expected: a.nrows(),
            got: b.len(),
        });
    }
    a.clone()
        .lu()
        .solve(b)
        .ok_or(LinalgError::SingularMatrix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nalgebra::{dmatrix, dvector};

    #[test]
    fn solve_identity() {
        let a = dmatrix![1.0, 0.0; 0.0, 1.0];
        let b = dvector![3.0, 7.0];
        let x = solve_linear(&a, &b).unwrap();
        assert!((x[0] - 3.0).abs() < 1e-12);
        assert!((x[1] - 7.0).abs() < 1e-12);
    }

    #[test]
    fn reject_singular() {
        let a = dmatrix![1.0, 0.0; 0.0, 0.0];
        let b = dvector![1.0, 1.0];
        assert!(solve_linear(&a, &b).is_err());
    }

    #[test]
    fn reject_dimension_mismatch() {
        let a = dmatrix![1.0, 0.0; 0.0, 1.0];
        let b = dvector![1.0, 2.0, 3.0];
        assert!(solve_linear(&a, &b).is_err());
    }
}
