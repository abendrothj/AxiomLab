//! Unit conversion helpers for capability bounds checking and measurement validation.
//!
//! All `to_base_*` functions convert a value expressed in the given unit string
//! into the canonical base unit used by [`crate::capabilities::CapabilityPolicy`] limits:
//! - Volume  → microlitres (µL)
//! - Length  → millimetres (mm)
//!
//! [`KNOWN_UNITS`] lists accepted QUDT / SI / common symbols for [`crate::discovery`]
//! `Measurement` validation.

/// Convert `value` expressed in `unit` to microlitres (µL).
///
/// Returns `Err` if the unit string is unrecognised.
pub fn to_base_ul(value: f64, unit: &str) -> Result<f64, String> {
    match unit {
        "µL" | "uL" | "ul" | "microliter" | "microlitre" => Ok(value),
        "mL" | "ml" | "milliliter" | "millilitre" => Ok(value * 1_000.0),
        "L" | "l" | "liter" | "litre" => Ok(value * 1_000_000.0),
        "nL" | "nl" | "nanoliter" | "nanolitre" => Ok(value / 1_000.0),
        _ => Err(format!("unknown volume unit: '{unit}'")),
    }
}

/// Convert `value` expressed in `unit` to millimetres (mm).
///
/// Returns `Err` if the unit string is unrecognised.
pub fn to_base_mm(value: f64, unit: &str) -> Result<f64, String> {
    match unit {
        "mm" | "millimeter" | "millimetre" => Ok(value),
        "cm" | "centimeter" | "centimetre" => Ok(value * 10.0),
        "m" | "meter" | "metre" => Ok(value * 1_000.0),
        "µm" | "um" | "micrometer" | "micrometre" => Ok(value / 1_000.0),
        "in" | "inch" | "inches" => Ok(value * 25.4),
        _ => Err(format!("unknown length unit: '{unit}'")),
    }
}

/// Attempt to convert `value` (declared in `unit`) to the canonical base for `param_name`.
///
/// The canonical base is inferred from the parameter name:
/// - `*_ul`             → µL (via [`to_base_ul`])
/// - `x`, `y`, `z`, `*_mm` → mm (via [`to_base_mm`])
/// - anything else      → no conversion, value returned as-is
///
/// On conversion error a warning is logged and the raw value is returned.
pub fn to_canonical(value: f64, unit: &str, param_name: &str) -> f64 {
    if param_name.ends_with("_ul") {
        match to_base_ul(value, unit) {
            Ok(v) => return v,
            Err(e) => tracing::warn!(param = param_name, %e, "unit conversion failed — using raw value"),
        }
    } else if param_name.ends_with("_mm")
        || matches!(param_name, "x" | "y" | "z")
    {
        match to_base_mm(value, unit) {
            Ok(v) => return v,
            Err(e) => tracing::warn!(param = param_name, %e, "unit conversion failed — using raw value"),
        }
    }
    value
}

/// Known QUDT / SI / common unit symbols accepted in `Measurement`.
///
/// Validated at construction time — unknown symbols trigger a `warn!` and the
/// unit is replaced with `"?"`.
pub static KNOWN_UNITS: &[&str] = &[
    // Dimensionless
    "",
    "AU",
    "OD",
    "OD600",
    "RFU",
    "RLU",
    "%",
    "ratio",
    // Volume
    "µL", "uL", "ul", "mL", "ml", "L", "l", "nL", "nl",
    // Length / distance
    "mm", "cm", "m", "µm", "um",
    // Concentration (molar)
    "µM", "uM", "mM", "M", "nM", "pM",
    // Concentration (mass/volume)
    "µg/mL", "ug/mL", "mg/mL", "g/L", "mg/L", "ng/mL",
    // Mass
    "g", "mg", "µg", "ug", "kg",
    // Temperature
    "°C", "K", "°F",
    // Time
    "s", "ms", "min", "h",
    // pH / electrochemistry
    "pH",
    "mS/cm",
    "µS/cm", "uS/cm",
    // Pressure
    "Pa", "kPa", "MPa", "bar", "mbar", "psi",
    // Flow rate
    "µL/min", "uL/min", "mL/min", "L/min",
    // Spectroscopy
    "Abs", "nm",
    // Misc
    "rpm",
    "W", "mW",
    "kJ/mol", "kcal/mol",
];

/// Returns `true` if `unit` is a recognised symbol.
pub fn is_known_unit(unit: &str) -> bool {
    KNOWN_UNITS.contains(&unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_conversions() {
        assert!((to_base_ul(1.0, "mL").unwrap() - 1_000.0).abs() < 1e-9);
        assert!((to_base_ul(1.0, "L").unwrap() - 1_000_000.0).abs() < 1e-9);
        assert!((to_base_ul(500.0, "nL").unwrap() - 0.5).abs() < 1e-9);
        assert!((to_base_ul(100.0, "µL").unwrap() - 100.0).abs() < 1e-9);
    }

    #[test]
    fn length_conversions() {
        assert!((to_base_mm(1.0, "cm").unwrap() - 10.0).abs() < 1e-9);
        assert!((to_base_mm(1.0, "m").unwrap() - 1_000.0).abs() < 1e-9);
        assert!((to_base_mm(1.0, "in").unwrap() - 25.4).abs() < 1e-9);
    }

    #[test]
    fn canonical_infers_from_param_name() {
        // volume_ul param + mL unit → converts to µL
        assert!((to_canonical(0.5, "mL", "volume_ul") - 500.0).abs() < 1e-9);
        // x param + cm unit → converts to mm
        assert!((to_canonical(10.0, "cm", "x") - 100.0).abs() < 1e-9);
        // unknown param → passthrough
        assert!((to_canonical(42.0, "rpm", "speed") - 42.0).abs() < 1e-9);
    }

    #[test]
    fn unknown_volume_unit_errors() {
        assert!(to_base_ul(1.0, "gallon").is_err());
    }

    #[test]
    fn known_units_coverage() {
        assert!(is_known_unit("µL"));
        assert!(is_known_unit("pH"));
        assert!(is_known_unit("°C"));
        assert!(!is_known_unit("furlong"));
    }
}
