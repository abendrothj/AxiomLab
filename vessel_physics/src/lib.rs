//! Formally verified vessel physics for AxiomLab.
//!
//! Volumes are stored as `u64` **nanoliters** internally so that the
//! arithmetic invariants can be expressed as pure integer inequalities and
//! discharged by Verus / Z3 without any floating-point reasoning.
//!
//! The Verus proofs live in `verus_verified/vessel_registry.rs`.  The two
//! core arithmetic operations — `proved_add` and `proved_sub` — are named
//! to signal that their correctness has been machine-checked; the runtime
//! guard (overflow / underflow check) in `dispense` / `aspirate` corresponds
//! exactly to the `requires` clause of the Verus spec.
//!
//! The public Python API is exposed via PyO3.  Build with:
//!   maturin develop --manifest-path vessel_physics/Cargo.toml

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

// ─────────────────────────────────────────────────────────────────────────────
// Core types
// ─────────────────────────────────────────────────────────────────────────────

/// Per-vessel physical state.  All volumes in nanoliters for Verus proofs.
#[derive(Debug, Clone)]
pub struct VesselState {
    /// Current fill level in nanoliters.
    pub volume_nl: u64,
    /// Maximum capacity in nanoliters.
    pub max_volume_nl: u64,
    /// Beer-Lambert molar absorptivity ε (AU per unit concentration per cm).
    pub absorbance_coefficient: f64,
    /// Optical path length through the vessel (cm).
    pub path_length_cm: f64,
}

impl VesselState {
    pub fn volume_ul(&self) -> f64 {
        self.volume_nl as f64 / 1_000.0
    }

    pub fn max_volume_ul(&self) -> f64 {
        self.max_volume_nl as f64 / 1_000.0
    }

    pub fn fill_fraction(&self) -> f64 {
        if self.max_volume_nl == 0 {
            0.0
        } else {
            self.volume_nl as f64 / self.max_volume_nl as f64
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Proved arithmetic — names signal Verus verification in vessel_registry.rs
// ─────────────────────────────────────────────────────────────────────────────

/// Add `delta_nl` to `volume_nl`.
///
/// Corresponds to `proved_add` in `verus_verified/vessel_registry.rs`.
/// Caller MUST have checked `volume_nl + delta_nl <= max_nl` before calling.
#[inline]
fn proved_add(volume_nl: u64, delta_nl: u64) -> u64 {
    volume_nl + delta_nl
}

/// Subtract `delta_nl` from `volume_nl`.
///
/// Corresponds to `proved_sub` in `verus_verified/vessel_registry.rs`.
/// Caller MUST have checked `delta_nl <= volume_nl` before calling.
#[inline]
fn proved_sub(volume_nl: u64, delta_nl: u64) -> u64 {
    volume_nl - delta_nl
}

// ─────────────────────────────────────────────────────────────────────────────
// VesselRegistry
// ─────────────────────────────────────────────────────────────────────────────

const DEFAULT_MAX_VOL_NL: u64 = 50_000_000; // 50 000 µL = 50 mL
const DEFAULT_EPSILON: f64 = 1.0;
const DEFAULT_PATH_LEN: f64 = 1.0;

/// Thread-safe registry of vessel physical states.
///
/// Invariant (maintained by `dispense` and `aspirate`):
///   ∀ v ∈ vessels: v.volume_nl ≤ v.max_volume_nl
pub struct VesselRegistry {
    vessels: HashMap<String, VesselState>,
}

impl VesselRegistry {
    /// Create a new registry with the standard AxiomLab lab vessel set.
    pub fn new() -> Self {
        let mut r = Self {
            vessels: HashMap::new(),
        };
        // Pre-registered lab vessels — mirror sila_mock/axiomlab_mock/vessel_state.py
        r.register("beaker_A",      50_000_000, 1.2, 1.0, 0);
        r.register("beaker_B",      50_000_000, 0.8, 1.0, 0);
        r.register("tube_1",         2_000_000, 1.5, 1.0, 0);
        r.register("tube_2",         2_000_000, 1.5, 1.0, 0);
        r.register("tube_3",         2_000_000, 1.5, 1.0, 0);
        r.register("plate_well_A1",    300_000, 2.0, 0.5, 0);
        r.register("plate_well_B1",    300_000, 2.0, 0.5, 0);
        r.register("reservoir",    200_000_000, 0.3, 1.0, 100_000_000);
        r
    }

    fn register(
        &mut self,
        id: &str,
        max_nl: u64,
        epsilon: f64,
        path_cm: f64,
        initial_nl: u64,
    ) {
        self.vessels.insert(
            id.to_string(),
            VesselState {
                volume_nl: initial_nl,
                max_volume_nl: max_nl,
                absorbance_coefficient: epsilon,
                path_length_cm: path_cm,
            },
        );
    }

    fn get_or_register(&mut self, vessel_id: &str) -> &mut VesselState {
        self.vessels
            .entry(vessel_id.to_string())
            .or_insert_with(|| VesselState {
                volume_nl: 0,
                max_volume_nl: DEFAULT_MAX_VOL_NL,
                absorbance_coefficient: DEFAULT_EPSILON,
                path_length_cm: DEFAULT_PATH_LEN,
            })
    }

    /// Register or re-configure a vessel.
    pub fn register_vessel(
        &mut self,
        vessel_id: &str,
        max_ul: f64,
        epsilon: f64,
        path_cm: f64,
        initial_ul: f64,
    ) {
        let max_nl = (max_ul * 1_000.0).round() as u64;
        let initial_nl = (initial_ul * 1_000.0).round() as u64;
        self.register(vessel_id, max_nl, epsilon, path_cm, initial_nl);
    }

    /// Dispense `volume_ul` µL into `vessel_id`.
    ///
    /// Returns `Err` if the dispense would exceed the vessel's capacity
    /// (mirrors the Python `VesselRegistry.dispense` ValueError).
    pub fn dispense(&mut self, vessel_id: &str, volume_ul: f64) -> Result<f64, String> {
        let delta_nl = (volume_ul * 1_000.0).round() as u64;
        let v = self.get_or_register(vessel_id);

        // Runtime guard — matches `requires` clause in Verus spec
        let new_nl = v.volume_nl.checked_add(delta_nl).ok_or_else(|| {
            format!(
                "Dispense of {:.1} µL into '{}' would overflow u64",
                volume_ul, vessel_id
            )
        })?;
        if new_nl > v.max_volume_nl {
            return Err(format!(
                "Dispense of {:.1} µL into '{}' would exceed capacity \
                 ({:.1} + {:.1} > {:.1} µL)",
                volume_ul,
                vessel_id,
                v.volume_ul(),
                volume_ul,
                v.max_volume_ul(),
            ));
        }

        // Arithmetic verified by Verus — precondition satisfied above
        v.volume_nl = proved_add(v.volume_nl, delta_nl);
        Ok(volume_ul)
    }

    /// Aspirate `volume_ul` µL from `vessel_id`.
    ///
    /// Returns `Err` if the vessel contains less than `volume_ul`.
    pub fn aspirate(&mut self, vessel_id: &str, volume_ul: f64) -> Result<f64, String> {
        let delta_nl = (volume_ul * 1_000.0).round() as u64;
        let v = self.get_or_register(vessel_id);

        if delta_nl > v.volume_nl {
            return Err(format!(
                "Aspirate of {:.1} µL from '{}' exceeds available volume \
                 ({:.1} µL available)",
                volume_ul,
                vessel_id,
                v.volume_ul(),
            ));
        }

        v.volume_nl = proved_sub(v.volume_nl, delta_nl);
        Ok(volume_ul)
    }

    /// Returns `volume_nl / max_volume_nl` in [0.0, 1.0].
    pub fn fill_fraction(&mut self, vessel_id: &str) -> f64 {
        self.get_or_register(vessel_id).fill_fraction()
    }

    pub fn volume_ul(&mut self, vessel_id: &str) -> f64 {
        self.get_or_register(vessel_id).volume_ul()
    }

    pub fn absorbance_coefficient(&mut self, vessel_id: &str) -> f64 {
        self.get_or_register(vessel_id).absorbance_coefficient
    }

    pub fn path_length_cm(&mut self, vessel_id: &str) -> f64 {
        self.get_or_register(vessel_id).path_length_cm
    }
}

impl Default for VesselRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PyO3 bindings
// ─────────────────────────────────────────────────────────────────────────────

/// Python-visible VesselRegistry.  Thread-safe via Arc<Mutex<_>>.
#[pyclass(name = "VesselRegistry")]
struct PyVesselRegistry {
    inner: Arc<Mutex<VesselRegistry>>,
}

#[pymethods]
impl PyVesselRegistry {
    #[new]
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(VesselRegistry::new())),
        }
    }

    /// Dispense `volume_ul` µL into `vessel_id`.  Raises `ValueError` on overflow.
    fn dispense(&self, vessel_id: &str, volume_ul: f64) -> PyResult<f64> {
        self.inner
            .lock()
            .unwrap()
            .dispense(vessel_id, volume_ul)
            .map_err(PyValueError::new_err)
    }

    /// Aspirate `volume_ul` µL from `vessel_id`.  Raises `ValueError` on underflow.
    fn aspirate(&self, vessel_id: &str, volume_ul: f64) -> PyResult<f64> {
        self.inner
            .lock()
            .unwrap()
            .aspirate(vessel_id, volume_ul)
            .map_err(PyValueError::new_err)
    }

    fn get_fill_fraction(&self, vessel_id: &str) -> f64 {
        self.inner.lock().unwrap().fill_fraction(vessel_id)
    }

    fn get_volume_ul(&self, vessel_id: &str) -> f64 {
        self.inner.lock().unwrap().volume_ul(vessel_id)
    }

    fn get_absorbance_coefficient(&self, vessel_id: &str) -> f64 {
        self.inner.lock().unwrap().absorbance_coefficient(vessel_id)
    }

    fn get_path_length_cm(&self, vessel_id: &str) -> f64 {
        self.inner.lock().unwrap().path_length_cm(vessel_id)
    }

    /// Register or re-configure a vessel (max_ul, epsilon, path_cm, initial_ul).
    #[pyo3(signature = (vessel_id, max_ul, epsilon, path_cm, initial_ul = 0.0))]
    fn register_vessel(
        &self,
        vessel_id: &str,
        max_ul: f64,
        epsilon: f64,
        path_cm: f64,
        initial_ul: f64,
    ) {
        self.inner
            .lock()
            .unwrap()
            .register_vessel(vessel_id, max_ul, epsilon, path_cm, initial_ul);
    }
}

/// The `vessel_physics` Python extension module.
#[pymodule]
fn vessel_physics(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyVesselRegistry>()?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Rust unit tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispense_increases_volume() {
        let mut r = VesselRegistry::new();
        r.dispense("beaker_A", 1_000.0).unwrap();
        assert!((r.volume_ul("beaker_A") - 1_000.0).abs() < 1.0);
    }

    #[test]
    fn aspirate_decreases_volume() {
        let mut r = VesselRegistry::new();
        r.dispense("beaker_A", 5_000.0).unwrap();
        r.aspirate("beaker_A", 2_000.0).unwrap();
        assert!((r.volume_ul("beaker_A") - 3_000.0).abs() < 1.0);
    }

    #[test]
    fn overflow_rejected() {
        let mut r = VesselRegistry::new();
        // plate_well_A1 max = 300 µL
        let err = r.dispense("plate_well_A1", 400.0).unwrap_err();
        assert!(err.contains("exceed capacity"), "expected overflow error: {err}");
    }

    #[test]
    fn underflow_rejected() {
        let mut r = VesselRegistry::new();
        let err = r.aspirate("beaker_A", 1.0).unwrap_err();
        assert!(err.contains("available"), "expected underflow error: {err}");
    }

    #[test]
    fn fill_fraction_range() {
        let mut r = VesselRegistry::new();
        assert_eq!(r.fill_fraction("beaker_A"), 0.0);
        r.dispense("beaker_A", 25_000.0).unwrap();
        let f = r.fill_fraction("beaker_A");
        assert!(f > 0.49 && f < 0.51, "half-fill fraction should be ~0.5: {f}");
    }

    #[test]
    fn reservoir_pre_filled() {
        let mut r = VesselRegistry::new();
        // reservoir starts at 100 000 µL
        assert!((r.volume_ul("reservoir") - 100_000.0).abs() < 1.0);
        assert!((r.fill_fraction("reservoir") - 0.5).abs() < 0.001);
    }

    #[test]
    fn aspirate_inverts_dispense_exactly() {
        let mut r = VesselRegistry::new();
        r.dispense("tube_1", 500.0).unwrap();
        r.aspirate("tube_1", 500.0).unwrap();
        assert_eq!(r.volume_ul("tube_1"), 0.0);
    }

    #[test]
    fn chain_dispenses_monotone() {
        let mut r = VesselRegistry::new();
        for i in 1..=5u32 {
            r.dispense("beaker_B", 1_000.0).unwrap();
            let expected = i as f64 * 1_000.0;
            assert!((r.volume_ul("beaker_B") - expected).abs() < 1.0);
        }
    }

    #[test]
    fn invariant_holds_after_many_ops() {
        let mut r = VesselRegistry::new();
        // Fill tube_1 to max (2 000 µL) in 200 µL steps
        for _ in 0..10 {
            r.dispense("tube_1", 200.0).unwrap();
        }
        assert!((r.volume_ul("tube_1") - 2_000.0).abs() < 1.0);
        // One more must fail
        assert!(r.dispense("tube_1", 1.0).is_err());
        // Drain completely
        for _ in 0..10 {
            r.aspirate("tube_1", 200.0).unwrap();
        }
        assert_eq!(r.volume_ul("tube_1"), 0.0);
        // Aspirating from empty must fail
        assert!(r.aspirate("tube_1", 0.1).is_err());
    }
}
