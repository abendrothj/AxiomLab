//! Common physical quantities used in autonomous lab workflows.

use uom::si::f64::*;
use uom::si::{length, mass, thermodynamic_temperature, time};

/// Create a length in metres.
pub fn metres(v: f64) -> Length {
    Length::new::<length::meter>(v)
}

/// Create a mass in kilograms.
pub fn kilograms(v: f64) -> Mass {
    Mass::new::<mass::kilogram>(v)
}

/// Create a temperature in kelvins.
pub fn kelvins(v: f64) -> ThermodynamicTemperature {
    ThermodynamicTemperature::new::<thermodynamic_temperature::kelvin>(v)
}

/// Create a duration in seconds.
pub fn seconds(v: f64) -> Time {
    Time::new::<time::second>(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uom::si::length::meter;

    #[test]
    fn add_compatible_units() {
        let a = metres(1.0);
        let b = metres(2.5);
        let sum = a + b;
        assert!((sum.get::<meter>() - 3.5).abs() < 1e-12);
    }

    // Attempting `metres(1.0) + kilograms(2.0)` is a compile-time error –
    // exactly the safety guarantee we need for autonomous agents.
}
