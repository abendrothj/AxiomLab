//! Lab data ingestion and playback for AxiomLab.
//!
//! Reads real or simulated sensor data from CSV files, enabling the agent
//! to run discovery on actual laboratory measurements rather than synthetic data.
//!
//! ## CSV Format
//!
//! The simplest supported format (header required):
//! ```csv
//! timestamp_s,channel_id,value,unit,label
//! 0.0,0,0.10,absorbance,concentration_0.1mM
//! 1.0,0,0.22,absorbance,concentration_0.2mM
//! ```
//!
//! Or for spectroscopy (x = wavelength, y = absorbance or intensity):
//! ```csv
//! wavelength_nm,absorbance
//! 400,0.10
//! 450,0.22
//! 500,0.35
//! ```

use std::io::{self, BufRead};
use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DataError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("parse error at line {line}: {reason}")]
    Parse { line: usize, reason: String },
    #[error("no data: empty dataset")]
    Empty,
    #[error("header error: missing expected column '{0}'")]
    MissingColumn(String),
    #[error("mismatched column count at line {line}: expected {expected}, got {actual}")]
    MismatchedColumns { line: usize, expected: usize, actual: usize },
}

/// A single sensor reading.
#[derive(Debug, Clone)]
pub struct Reading {
    /// Time offset from experiment start (seconds).
    pub timestamp_s: f64,
    /// Hardware channel (0 = first sensor).
    pub channel_id: u32,
    /// Measured value.
    pub value: f64,
    /// Physical unit (e.g. "absorbance", "pH", "celsius").
    pub unit: String,
    /// Optional label / annotation.
    pub label: Option<String>,
}

/// A pair of measurements suitable for regression (independent variable, dependent variable).
#[derive(Debug, Clone)]
pub struct DataPair {
    pub x: f64,
    pub y: f64,
    pub x_label: String,
    pub y_label: String,
}

/// Parsed lab dataset.
#[derive(Debug)]
pub struct LabDataset {
    pub name: String,
    pub readings: Vec<Reading>,
}

impl LabDataset {
    /// Create a dataset from a vector of readings.
    pub fn from_readings(name: impl Into<String>, readings: Vec<Reading>) -> Self {
        Self { name: name.into(), readings }
    }

    /// Export to (x, y) pairs for regression; `x_channel` is the independent variable.
    pub fn to_xy_pairs(&self, x_channel: u32, y_channel: u32) -> Vec<DataPair> {
        // Build matching index: timestamp → channel values
        let mut _map: std::collections::HashMap<u64, f64> = std::collections::HashMap::new();
        let x_readings: Vec<_> = self
            .readings
            .iter()
            .filter(|r| r.channel_id == x_channel)
            .collect();
        let y_readings: Vec<_> = self
            .readings
            .iter()
            .filter(|r| r.channel_id == y_channel)
            .collect();

        x_readings
            .iter()
            .zip(y_readings.iter())
            .map(|(x_r, y_r)| DataPair {
                x: x_r.value,
                y: y_r.value,
                x_label: x_r.unit.clone(),
                y_label: y_r.unit.clone(),
            })
            .collect()
    }

    /// Convenience: extract all values from one channel as a vector.
    pub fn channel_values(&self, channel_id: u32) -> Vec<f64> {
        self.readings
            .iter()
            .filter(|r| r.channel_id == channel_id)
            .map(|r| r.value)
            .collect()
    }
}

/// Parse a simple two-column CSV (x_header, y_header) from a string.
///
/// This is the simplest supported format, e.g.:
/// ```csv
/// wavelength_nm,absorbance
/// 400,0.10
/// 450,0.22
/// ```
pub fn parse_xy_csv(data: &str) -> Result<(Vec<f64>, Vec<f64>), DataError> {
    let mut lines = data.lines().enumerate();

    // Read header
    let (_header_line, header) = lines
        .next()
        .ok_or(DataError::Empty)?;

    let headers: Vec<&str> = header.split(',').map(str::trim).collect();
    if headers.len() < 2 {
        return Err(DataError::MissingColumn("y column".to_string()));
    }

    let mut xs = Vec::new();
    let mut ys = Vec::new();

    for (line_num, line) in lines {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue; // Skip blanks + comment lines
        }

        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() < 2 {
            return Err(DataError::MismatchedColumns {
                line: line_num + 1,
                expected: 2,
                actual: parts.len(),
            });
        }

        let x = f64::from_str(parts[0].trim()).map_err(|_| DataError::Parse {
            line: line_num + 1,
            reason: format!("'{}' is not a valid number (x column)", parts[0].trim()),
        })?;
        let y = f64::from_str(parts[1].trim()).map_err(|_| DataError::Parse {
            line: line_num + 1,
            reason: format!("'{}' is not a valid number (y column)", parts[1].trim()),
        })?;

        xs.push(x);
        ys.push(y);
    }

    if xs.is_empty() {
        return Err(DataError::Empty);
    }

    Ok((xs, ys))
}

/// Parse a full multi-channel sensor log CSV.
///
/// Format: `timestamp_s,channel_id,value,unit[,label]`
pub fn parse_sensor_log(data: &str) -> Result<LabDataset, DataError> {
    let mut lines = data.lines().enumerate().peekable();

    // Skip header
    lines.next().ok_or(DataError::Empty)?;

    let mut readings = Vec::new();

    for (line_num, line) in lines {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() < 4 {
            return Err(DataError::MismatchedColumns {
                line: line_num + 1,
                expected: 4,
                actual: parts.len(),
            });
        }

        let timestamp_s = f64::from_str(parts[0].trim()).map_err(|_| DataError::Parse {
            line: line_num + 1,
            reason: format!("'{}' is not a valid timestamp", parts[0].trim()),
        })?;
        let channel_id = u32::from_str(parts[1].trim()).map_err(|_| DataError::Parse {
            line: line_num + 1,
            reason: format!("'{}' is not a valid channel id", parts[1].trim()),
        })?;
        let value = f64::from_str(parts[2].trim()).map_err(|_| DataError::Parse {
            line: line_num + 1,
            reason: format!("'{}' is not a valid measurement", parts[2].trim()),
        })?;
        let unit = parts[3].trim().to_string();
        let label = parts.get(4).map(|s| s.trim().to_string());

        readings.push(Reading { timestamp_s, channel_id, value, unit, label });
    }

    if readings.is_empty() {
        return Err(DataError::Empty);
    }

    Ok(LabDataset::from_readings("sensor_log", readings))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPECTROSCOPY_CSV: &str = "wavelength_nm,absorbance
400,0.105
450,0.162
500,0.218
550,0.274
600,0.330
";

    const CONCENTRATION_SERIES_CSV: &str = "concentration_mM,absorbance
0.10,0.105
0.20,0.210
0.30,0.315
0.40,0.420
0.50,0.525
";

    const SENSOR_LOG_CSV: &str =
"timestamp_s,channel_id,value,unit,label
0.0,0,0.105,absorbance,run1_c0.1mM
1.0,0,0.210,absorbance,run1_c0.2mM
2.0,0,0.315,absorbance,run1_c0.3mM
";

    const NONLINEAR_CSV: &str = "x,y
1.0,1.0
2.0,4.0
3.0,9.0
4.0,16.0
5.0,25.0
";

    #[test]
    fn parse_spectroscopy_csv() {
        let (x, y) = parse_xy_csv(SPECTROSCOPY_CSV).expect("should parse");
        assert_eq!(x.len(), 5);
        assert_eq!(y.len(), 5);
        assert_eq!(x[0], 400.0);
        assert!((y[0] - 0.105).abs() < 1e-10);
    }

    #[test]
    fn parse_concentration_series() {
        let (x, y) = parse_xy_csv(CONCENTRATION_SERIES_CSV).expect("should parse");
        assert_eq!(x.len(), 5);
        // Beer-Lambert should give near-linear relationship
        use crate::discovery::linear_regression;
        let fit = linear_regression(&x, &y).expect("fit should succeed");
        assert!(
            fit.r_squared > 0.9999,
            "Beer-Lambert concentration series should be perfectly linear (R²={:.6})",
            fit.r_squared
        );
        println!("✓ CSV Beer-Lambert fit: R² = {:.6}, slope = {:.4}", fit.r_squared, fit.slope);
    }

    #[test]
    fn parse_sensor_log_csv() {
        let ds = parse_sensor_log(SENSOR_LOG_CSV).expect("should parse");
        assert_eq!(ds.readings.len(), 3);
        assert_eq!(ds.readings[0].channel_id, 0);
        assert_eq!(ds.readings[0].unit, "absorbance");
        assert_eq!(ds.readings[1].value, 0.210);
    }

    #[test]
    fn detect_nonlinearity_from_csv() {
        let (x, y) = parse_xy_csv(NONLINEAR_CSV).expect("should parse");
        use crate::discovery::linear_regression;
        let fit = linear_regression(&x, &y).expect("fit should succeed");
        // Quadratic data: linear fit should be poor
        assert!(
            fit.r_squared < 0.99,
            "Agent should detect nonlinearity from CSV data (R²={:.4})",
            fit.r_squared
        );
        println!("✓ CSV nonlinearity detected: R² = {:.4} (< 0.99)", fit.r_squared);
    }

    #[test]
    fn parse_csv_rejects_empty() {
        let result = parse_xy_csv("");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DataError::Empty));
    }

    #[test]
    fn parse_csv_handles_comments() {
        let csv_with_comments = "x,y\n# This is a comment\n1.0,2.0\n# Another\n3.0,4.0\n";
        let (x, y) = parse_xy_csv(csv_with_comments).expect("should handle comments");
        assert_eq!(x, vec![1.0, 3.0]);
        assert_eq!(y, vec![2.0, 4.0]);
    }

    #[test]
    fn parse_csv_error_on_bad_number() {
        let bad = "x,y\n1.0,two\n";
        let result = parse_xy_csv(bad);
        assert!(matches!(result, Err(DataError::Parse { .. })));
    }
}
