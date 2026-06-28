//! Physical-quantity newtypes.
//!
//! These wrap raw `f64` so a volume cannot be silently passed where a
//! temperature is expected. They carry their canonical base unit in the name:
//! microlitres for volume, degrees Celsius for temperature, dimensionless pH.

use serde::{Deserialize, Serialize};

/// A volume in microlitres (µL) — the canonical base unit for liquid handling.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct VolumeUl(pub f64);

/// A temperature in degrees Celsius.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct TempC(pub f64);

/// A pH value (dimensionless, nominally 0–14).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Ph(pub f64);

macro_rules! quantity_accessors {
    ($($t:ty),*) => {$(
        impl $t {
            /// The underlying value in this quantity's base unit.
            #[inline]
            pub fn value(self) -> f64 { self.0 }
        }
        impl From<f64> for $t {
            #[inline]
            fn from(v: f64) -> Self { Self(v) }
        }
    )*};
}

quantity_accessors!(VolumeUl, TempC, Ph);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newtypes_preserve_value() {
        assert_eq!(VolumeUl(100.0).value(), 100.0);
        assert_eq!(TempC::from(37.0).value(), 37.0);
        assert_eq!(Ph(7.4).value(), 7.4);
    }

    #[test]
    fn ordering_works() {
        assert!(VolumeUl(1.0) < VolumeUl(2.0));
    }
}
