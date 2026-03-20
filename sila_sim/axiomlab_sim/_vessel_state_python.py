from __future__ import annotations
import threading
from dataclasses import dataclass, field


@dataclass
class VesselState:
    volume_ul: float
    max_volume_ul: float
    absorbance_coefficient: float  # ε in Beer-Lambert (AU per unit concentration per cm)
    path_length_cm: float


class VesselRegistry:
    _DEFAULT_MAX_VOL = 50_000.0
    _DEFAULT_EPSILON = 1.0
    _DEFAULT_PATH_LEN = 1.0

    def __init__(self) -> None:
        self._vessels: dict[str, VesselState] = {}
        self._lock = threading.Lock()

        # Pre-registered lab vessels
        self.register_vessel("beaker_A",      50_000.0, 1.2, 1.0)
        self.register_vessel("beaker_B",      50_000.0, 0.8, 1.0)
        self.register_vessel("tube_1",         2_000.0, 1.5, 1.0)
        self.register_vessel("tube_2",         2_000.0, 1.5, 1.0)
        self.register_vessel("tube_3",         2_000.0, 1.5, 1.0)
        self.register_vessel("plate_well_A1",    300.0, 2.0, 0.5)
        self.register_vessel("plate_well_B1",    300.0, 2.0, 0.5)
        self.register_vessel("reservoir",    200_000.0, 0.3, 1.0, initial_volume_ul=100_000.0)

    def register_vessel(
        self,
        vessel_id: str,
        max_volume_ul: float,
        absorbance_coefficient: float,
        path_length_cm: float,
        initial_volume_ul: float = 0.0,
    ) -> None:
        with self._lock:
            self._vessels[vessel_id] = VesselState(
                volume_ul=initial_volume_ul,
                max_volume_ul=max_volume_ul,
                absorbance_coefficient=absorbance_coefficient,
                path_length_cm=path_length_cm,
            )

    def get_vessel(self, vessel_id: str) -> VesselState:
        with self._lock:
            if vessel_id not in self._vessels:
                self._vessels[vessel_id] = VesselState(
                    volume_ul=0.0,
                    max_volume_ul=self._DEFAULT_MAX_VOL,
                    absorbance_coefficient=self._DEFAULT_EPSILON,
                    path_length_cm=self._DEFAULT_PATH_LEN,
                )
            return self._vessels[vessel_id]

    def get_volume(self, vessel_id: str) -> float:
        return self.get_vessel(vessel_id).volume_ul

    def get_fill_fraction(self, vessel_id: str) -> float:
        v = self.get_vessel(vessel_id)
        return v.volume_ul / v.max_volume_ul

    def dispense(self, vessel_id: str, volume_ul: float) -> float:
        with self._lock:
            v = self._vessels.get(vessel_id)
            if v is None:
                v = VesselState(
                    volume_ul=0.0,
                    max_volume_ul=self._DEFAULT_MAX_VOL,
                    absorbance_coefficient=self._DEFAULT_EPSILON,
                    path_length_cm=self._DEFAULT_PATH_LEN,
                )
                self._vessels[vessel_id] = v
            if v.volume_ul + volume_ul > v.max_volume_ul:
                raise ValueError(
                    f"Dispense of {volume_ul:.1f} µL into '{vessel_id}' would exceed capacity "
                    f"({v.volume_ul:.1f} + {volume_ul:.1f} > {v.max_volume_ul:.1f} µL)"
                )
            v.volume_ul = round(v.volume_ul + volume_ul, 4)
            return volume_ul

    def aspirate(self, vessel_id: str, volume_ul: float) -> float:
        with self._lock:
            v = self._vessels.get(vessel_id)
            if v is None:
                v = VesselState(
                    volume_ul=0.0,
                    max_volume_ul=self._DEFAULT_MAX_VOL,
                    absorbance_coefficient=self._DEFAULT_EPSILON,
                    path_length_cm=self._DEFAULT_PATH_LEN,
                )
                self._vessels[vessel_id] = v
            if volume_ul > v.volume_ul:
                raise ValueError(
                    f"Aspirate of {volume_ul:.1f} µL from '{vessel_id}' exceeds available volume "
                    f"({v.volume_ul:.1f} µL available)"
                )
            v.volume_ul = round(v.volume_ul - volume_ul, 4)
            return volume_ul

    def all_volumes(self) -> dict:
        """Return {vessel_id: {"volume_ul": float, "max_volume_ul": float}} for all vessels."""
        with self._lock:
            return {
                vid: {"volume_ul": vs.volume_ul, "max_volume_ul": vs.max_volume_ul}
                for vid, vs in self._vessels.items()
            }
