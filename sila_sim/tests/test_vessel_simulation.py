"""
SiLA 2 vessel simulation integration tests.

Every test in this file starts the real AxiomLab SiLA 2 server in-process and
drives it exclusively through the SiLA 2 protocol via the generated Python
client.  No physics layer code is imported or called directly — every assertion
reflects an actual SiLA 2 gRPC round-trip through the full server stack:

    test → Client.LiquidHandler.Dispense / Spectrophotometer.ReadAbsorbance
         → gRPC → Server → feature implementation → VesselRegistry
         → Beer-Lambert formula → gRPC response → assertion

Run (inside Docker or any environment with sila2 installed):
    cd sila_sim
    python -m pytest tests/test_vessel_simulation.py -v
  or:
    python -m unittest tests.test_vessel_simulation -v
"""

import math
import time
import unittest

# The entire module requires sila2.  Skip gracefully when it is not installed
# (e.g. on a host machine without the Docker environment).
try:
    from axiomlab_sim.server import Server
    from axiomlab_sim.generated.client import Client
    _SILA2_AVAILABLE = True
except ImportError:
    _SILA2_AVAILABLE = False

# Keep in sync with spectrophotometer_impl constants.
# Used only to compute expected ranges for assertion bounds — not to call
# implementation code.
_PEAK_NM = 500.0
_SIGMA_NM = 150.0
_BASELINE_AU = 0.001
_NOISE = 0.02   # ±2 % instrument noise

# Dedicated port so these tests can coexist with the Rust integration test
# server running on the default 50052.
_PORT = 50053


def _wl_factor(nm: float) -> float:
    return math.exp(-0.5 * ((nm - _PEAK_NM) / _SIGMA_NM) ** 2)


def _abs_bounds(epsilon: float, fill: float, path_len: float, wl: float):
    """Return (lo, hi) AU bounds that a SiLA 2 ReadAbsorbance call must land in."""
    a_det = max(_BASELINE_AU, epsilon * fill * path_len * _wl_factor(wl))
    return a_det * (1 - _NOISE), a_det * (1 + _NOISE)


@unittest.skipUnless(_SILA2_AVAILABLE, "sila2 not installed — run inside Docker")
class TestSiLA2VesselSimulation(unittest.TestCase):
    """
    Full-stack vessel simulation tests over the SiLA 2 wire protocol.

    setUpClass starts a real Server (insecure, port 50053) and connects a real
    Client.  tearDownClass stops the server.  Each test method uses a vessel ID
    that is unique within this class so no test observes another's side-effects.
    Pre-registered vessels (plate_well_A1, reservoir) are used only where their
    known capacity or initial volume is required for the assertion.
    """

    server: Server
    client: Client

    @classmethod
    def setUpClass(cls):
        cls.server = Server()
        cls.server.start_insecure("127.0.0.1", _PORT, enable_discovery=False)
        # Allow the gRPC server a moment to begin accepting connections before
        # the Client constructor attempts to open the channel.
        deadline = time.monotonic() + 5.0
        while time.monotonic() < deadline:
            try:
                cls.client = Client("127.0.0.1", _PORT, insecure=True)
                break
            except Exception:
                time.sleep(0.1)
        else:
            raise RuntimeError(f"SiLA 2 server did not become reachable on port {_PORT}")

    @classmethod
    def tearDownClass(cls):
        cls.client = None
        cls.server.stop()

    # ── Helpers ───────────────────────────────────────────────────

    def _dispense(self, vessel: str, vol: float) -> float:
        return self.client.LiquidHandler.Dispense(TargetVessel=vessel, VolumeUl=vol).DispensedVolumeUl

    def _aspirate(self, vessel: str, vol: float) -> float:
        return self.client.LiquidHandler.Aspirate(SourceVessel=vessel, VolumeUl=vol).AspiratedVolumeUl

    def _read(self, vessel: str, wl: float = 500.0) -> float:
        return self.client.Spectrophotometer.ReadAbsorbance(VesselId=vessel, WavelengthNm=wl).Absorbance

    # ── Volume round-trip ─────────────────────────────────────────

    def test_dispense_returns_confirmed_volume_within_hardware_noise(self):
        """Dispense 500 µL; hardware applies ±1 % variance; confirmed volume
        must be within ±5 µL of requested."""
        dispensed = self._dispense("py_v01_dispense_confirm", 500.0)
        self.assertAlmostEqual(dispensed, 500.0, delta=5.0)

    def test_aspirate_returns_confirmed_volume_within_hardware_noise(self):
        self._dispense("py_v02_aspirate_confirm", 800.0)
        aspirated = self._aspirate("py_v02_aspirate_confirm", 400.0)
        self.assertAlmostEqual(aspirated, 400.0, delta=4.0)

    # ── Absorbance baseline: empty vessel ─────────────────────────

    def test_empty_vessel_absorbance_equals_instrument_baseline(self):
        """An unregistered vessel starts at 0 µL → fill fraction = 0 →
        A_base = 0 → server returns the instrument floor of 0.001 AU."""
        abs_val = self._read("py_v03_empty_baseline")
        self.assertGreaterEqual(abs_val, _BASELINE_AU)
        self.assertLessEqual(abs_val, _BASELINE_AU * 1.01,
            f"empty vessel must return baseline ~{_BASELINE_AU} AU, got {abs_val}")

    # ── Single dispense raises absorbance ────────────────────────

    def test_single_dispense_raises_absorbance_above_baseline(self):
        """After dispensing 1 000 µL into a 50 000 µL vessel (fill = 2 %),
        the spectrophotometer must return > baseline.
        Expected A at 500 nm: ε=1.0 × 0.02 × l=1.0 = 0.020 AU ± 2 %."""
        vessel = "py_v04_single_fill"
        before = self._read(vessel)
        self._dispense(vessel, 1000.0)
        after = self._read(vessel)

        self.assertGreater(after, before,
            f"absorbance must rise after dispense: {before:.5f} → {after:.5f}")
        # Auto-registered: ε=1.0, l=1.0, max=50 000 µL → fill=0.02 at 500 nm
        lo, hi = _abs_bounds(epsilon=1.0, fill=0.02, path_len=1.0, wl=500.0)
        self.assertGreaterEqual(after, lo, f"A={after:.5f} below expected lo={lo:.5f}")
        self.assertLessEqual(after, hi, f"A={after:.5f} above expected hi={hi:.5f}")

    # ── Absorbance tracks fill monotonically ─────────────────────

    def test_absorbance_strictly_increases_with_each_dispense(self):
        """Three successive 1 000 µL dispenses must produce three strictly
        ascending absorbance readings.  The signal doubles each step, so
        ±2 % noise cannot reverse the order."""
        vessel = "py_v05_monotonic"
        readings = []
        for _ in range(3):
            self._dispense(vessel, 1000.0)
            readings.append(self._read(vessel))

        self.assertGreater(readings[1], readings[0],
            f"2nd reading must exceed 1st: {readings[0]:.5f} → {readings[1]:.5f}")
        self.assertGreater(readings[2], readings[1],
            f"3rd reading must exceed 2nd: {readings[1]:.5f} → {readings[2]:.5f}")

    # ── Aspirate lowers absorbance ────────────────────────────────

    def test_aspirate_reduces_absorbance(self):
        """Fill a vessel to 40 % (20 × 1 000 µL in a 50 000 µL vessel), record
        absorbance, remove half, assert absorbance drops by roughly half."""
        vessel = "py_v06_aspirate_lowers"
        for _ in range(20):
            self._dispense(vessel, 1000.0)
        a_full = self._read(vessel)

        for _ in range(10):
            self._aspirate(vessel, 1000.0)
        a_half = self._read(vessel)

        self.assertLess(a_half, a_full,
            f"absorbance must drop after aspiration: {a_full:.5f} → {a_half:.5f}")
        ratio = a_half / a_full
        self.assertGreater(ratio, 0.45, f"ratio too low: {ratio:.3f}")
        self.assertLess(ratio, 0.55, f"ratio too high: {ratio:.3f}")

    # ── Complete drain returns to baseline ────────────────────────

    def test_drain_returns_absorbance_to_near_baseline(self):
        """Dispense 500 µL (confirmed: 495–505 µL).  Aspirate a fixed 490 µL
        — guaranteed less than the minimum confirmed dispensed amount after
        ±1 % variance on both operations — leaving at most ~15 µL in the
        vessel.  Remaining fill fraction ≤ 0.03 % in a 50 000 µL vessel →
        A_base ≤ 0.0003, indistinguishable from the 0.001 AU baseline."""
        vessel = "py_v07_drain_baseline"
        self._dispense(vessel, 500.0)
        after_fill = self._read(vessel)
        self.assertGreater(after_fill, _BASELINE_AU)

        # 490 µL request × 1.01 max variance = 494.9 µL removed.
        # Vessel holds at minimum 500 × 0.99 = 495 µL → always 495 > 494.9 ✓
        self._aspirate(vessel, 490.0)
        after_drain = self._read(vessel)

        self.assertLess(after_drain, after_fill,
            f"after drain must be less than after fill: {after_fill:.5f} → {after_drain:.5f}")
        # Remaining volume ≤ 15 µL in 50 000 µL vessel → fill ≤ 0.03 % →
        # A_base ≤ 0.0003, well within baseline territory
        self.assertLessEqual(after_drain, _BASELINE_AU * 1.05,
            f"nearly-drained vessel must read back near baseline: {after_drain:.5f}")

    # ── Wavelength modulates absorbance ──────────────────────────

    def test_wavelength_modulates_absorbance_per_gaussian_curve(self):
        """Fill a vessel to 50 % and read at three wavelengths.
        The Gaussian transfer function peaks at 500 nm, so A(500) > A(350) > A(200)."""
        vessel = "py_v08_wavelength"
        for _ in range(25):
            self._dispense(vessel, 1000.0)   # 25 000 / 50 000 = 50 %

        a_500 = self._read(vessel, 500.0)
        a_350 = self._read(vessel, 350.0)
        a_200 = self._read(vessel, 200.0)

        self.assertGreater(a_500, a_350,
            f"peak (500 nm) must exceed 350 nm: {a_500:.5f} vs {a_350:.5f}")
        self.assertGreater(a_350, a_200,
            f"350 nm must exceed 200 nm: {a_350:.5f} vs {a_200:.5f}")

        # At 200 nm the attenuation factor is exp(-0.5*(300/150)^2) = exp(-2) ≈ 0.135
        ratio = a_200 / a_500
        self.assertGreater(ratio, 0.10, f"UV ratio too low: {ratio:.4f}")
        self.assertLess(ratio, 0.18, f"UV ratio too high: {ratio:.4f}")

    # ── Pre-registered vessel properties are honoured ─────────────

    def test_plate_well_absorbance_uses_registered_epsilon_and_path_length(self):
        """plate_well_B1 is pre-registered with ε=2.0, l=0.5, max=300 µL.
        The hardware applies ±1 % dispense variance, so we compute expected
        bounds from the *actual* confirmed dispensed volume rather than the
        requested volume.  Beer-Lambert: A = ε × (actual/max) × l ± 2 % noise."""
        resp = self.client.LiquidHandler.Dispense(TargetVessel="plate_well_B1", VolumeUl=150.0)
        actual_fill = resp.DispensedVolumeUl / 300.0   # plate_well_B1 max = 300 µL
        a = self._read("plate_well_B1", 500.0)
        lo, hi = _abs_bounds(epsilon=2.0, fill=actual_fill, path_len=0.5, wl=500.0)
        self.assertGreaterEqual(a, lo, f"A={a:.5f} below expected lo={lo:.5f} (fill={actual_fill:.4f})")
        self.assertLessEqual(a, hi, f"A={a:.5f} above expected hi={hi:.5f} (fill={actual_fill:.4f})")

    # ── Overflow rejected by server ───────────────────────────────

    def test_overflow_returns_sila2_error(self):
        """plate_well_A1 has a registered capacity of 300 µL.
        Dispensing 280 µL succeeds; a subsequent 30 µL must raise a SiLA 2
        error from the server — not a Rust-side capability rejection."""
        self.client.LiquidHandler.Dispense(TargetVessel="plate_well_A1", VolumeUl=280.0)
        with self.assertRaises(Exception) as ctx:
            self.client.LiquidHandler.Dispense(TargetVessel="plate_well_A1", VolumeUl=30.0)
        msg = str(ctx.exception).lower()
        self.assertTrue(
            "exceed" in msg or "capacity" in msg,
            f"overflow error must describe the capacity violation: {ctx.exception}"
        )

    # ── Underflow rejected by server ──────────────────────────────

    def test_underflow_from_empty_vessel_returns_sila2_error(self):
        """Aspirating from a vessel that has never been dispensed into must
        raise a SiLA 2 error from the server."""
        with self.assertRaises(Exception) as ctx:
            self.client.LiquidHandler.Aspirate(SourceVessel="py_v11_empty_aspirate", VolumeUl=100.0)
        msg = str(ctx.exception).lower()
        self.assertTrue(
            "available" in msg or "exceed" in msg,
            f"underflow error must describe insufficient volume: {ctx.exception}"
        )

    def test_partial_underflow_returns_sila2_error(self):
        """Aspirating more than is present raises a SiLA 2 error even when
        some liquid exists."""
        self._dispense("py_v12_partial_under", 400.0)
        with self.assertRaises(Exception) as ctx:
            self._aspirate("py_v12_partial_under", 600.0)
        msg = str(ctx.exception).lower()
        self.assertTrue(
            "available" in msg or "exceed" in msg,
            f"partial underflow must raise a descriptive error: {ctx.exception}"
        )

    # ── Reservoir pre-fill observable through the protocol ───────

    def test_reservoir_is_pre_filled_and_aspirable_via_sila2(self):
        """The reservoir is pre-registered with 100 000 µL initial volume.
        10 × 1 000 µL aspirations remove 10 000 µL (10 % of fill), producing a
        Beer-Lambert signal change of Δ ≈ 0.015 AU — well above ±2 % noise."""
        a_before = self._read("reservoir")
        for _ in range(10):
            self._aspirate("reservoir", 1000.0)
        a_after = self._read("reservoir")
        self.assertLess(a_after, a_before,
            f"reservoir absorbance must drop after aspiration: {a_before:.6f} → {a_after:.6f}")

    # ── Cross-instrument state coupling ──────────────────────────

    def test_liquid_handler_state_immediately_visible_to_spectrophotometer(self):
        """The LiquidHandler and Spectrophotometer share the same VesselRegistry.
        A dispense via LiquidHandler must be immediately visible to the
        Spectrophotometer — no polling, no lag — because both services run
        in the same server process.

        We request 980 µL (max confirmed ≤ 989.8 µL after ±1 % variance, safely
        under the 1 000 µL hardware aspirate cap) so the exact confirmed amount
        can be drained in a single Aspirate call."""
        vessel = "py_v14_coupling"

        a_empty = self._read(vessel)
        self.assertLessEqual(a_empty, _BASELINE_AU * 1.01)

        # Dispense 980 µL — confirmed amount always in [970.2, 989.8] µL
        self._dispense(vessel, 980.0)
        a_filled = self._read(vessel)
        self.assertGreater(a_filled, a_empty,
            f"spectrophotometer must see the dispensed liquid immediately: "
            f"{a_empty:.5f} → {a_filled:.5f}")

        # Aspirate a fixed 900 µL.  900 × 1.01 = 909 µL max actual, safely
        # below the minimum confirmed dispensed amount (970.2 µL) — no underflow.
        # The vessel drops to ~70–80 µL, clearly reducing absorbance.
        self._aspirate(vessel, 900.0)
        a_partial = self._read(vessel)
        self.assertLess(a_partial, a_filled,
            f"spectrophotometer must see the aspirated liquid immediately: "
            f"{a_filled:.5f} → {a_partial:.5f}")


if __name__ == "__main__":
    unittest.main()
