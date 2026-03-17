# AxiomLab vessel state — PyO3 Rust backend with Python fallback.
#
# Attempts to import the formally verified Rust VesselRegistry (built by
# `maturin develop --manifest-path vessel_physics/Cargo.toml`).  If the
# native module is not installed, falls back to the pure-Python
# implementation so the SiLA 2 mock can still be started without a Rust
# toolchain.
#
# BACKEND == "rust"   → physics enforced by proved_add / proved_sub in Rust
# BACKEND == "python" → pure-Python fallback (no Verus guarantees)
from __future__ import annotations

from dataclasses import dataclass


# ─────────────────────────────────────────────────────────────────────────────
# Try Rust backend first
# ─────────────────────────────────────────────────────────────────────────────

try:
    from vessel_physics import VesselRegistry as _RustRegistry  # type: ignore

    @dataclass
    class _VesselProxy:
        """Minimal vessel descriptor for spectrophotometer_impl.py compatibility.

        spectrophotometer_impl.py accesses:
            vessel.absorbance_coefficient
            vessel.path_length_cm
        """
        absorbance_coefficient: float
        path_length_cm: float

    class VesselRegistry:
        """Thin Python adapter over the PyO3 Rust VesselRegistry.

        The underlying Rust implementation stores volumes as u64 nanoliters
        and calls formally verified arithmetic (proved_add / proved_sub) from
        verus_verified/vessel_registry.rs after runtime overflow/underflow
        checks.
        """

        def __init__(self) -> None:
            self._inner = _RustRegistry()

        # ── liquid operations ──────────────────────────────────────────────

        def dispense(self, vessel_id: str, volume_ul: float) -> float:
            """Dispense volume_ul µL into vessel_id.  Raises ValueError on overflow."""
            return self._inner.dispense(vessel_id, volume_ul)

        def aspirate(self, vessel_id: str, volume_ul: float) -> float:
            """Aspirate volume_ul µL from vessel_id.  Raises ValueError on underflow."""
            return self._inner.aspirate(vessel_id, volume_ul)

        # ── state accessors ────────────────────────────────────────────────

        def get_vessel(self, vessel_id: str) -> _VesselProxy:
            """Return a vessel descriptor (absorbance_coefficient, path_length_cm)."""
            return _VesselProxy(
                absorbance_coefficient=self._inner.get_absorbance_coefficient(vessel_id),
                path_length_cm=self._inner.get_path_length_cm(vessel_id),
            )

        def get_fill_fraction(self, vessel_id: str) -> float:
            return self._inner.get_fill_fraction(vessel_id)

        def get_volume(self, vessel_id: str) -> float:
            return self._inner.get_volume_ul(vessel_id)

        def register_vessel(
            self,
            vessel_id: str,
            max_volume_ul: float,
            absorbance_coefficient: float,
            path_length_cm: float,
            initial_volume_ul: float = 0.0,
        ) -> None:
            self._inner.register_vessel(
                vessel_id,
                max_volume_ul,
                absorbance_coefficient,
                path_length_cm,
                initial_volume_ul,
            )

    BACKEND = "rust"

# ─────────────────────────────────────────────────────────────────────────────
# Pure-Python fallback (no Verus guarantees)
# ─────────────────────────────────────────────────────────────────────────────

except ImportError:
    import warnings
    warnings.warn(
        "vessel_physics native module not found — falling back to pure-Python "
        "VesselRegistry (no Verus guarantees).  Build the Rust module with:\n"
        "  maturin develop --manifest-path vessel_physics/Cargo.toml",
        stacklevel=2,
    )
    from ._vessel_state_python import VesselRegistry  # type: ignore  # noqa: F401
    BACKEND = "python"
