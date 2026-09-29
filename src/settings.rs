use std::time::Duration;

use crate::errors::SerialError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parity {
    None,
    Even,
    Odd,
    Mark,
    Space,
}

impl Parity {
    pub fn from_name(name: &str) -> Result<Self, SerialError> {
        // Message text matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt).
        match name {
            "N" => Ok(Self::None),
            "E" => Ok(Self::Even),
            "O" => Ok(Self::Odd),
            "M" => Ok(Self::Mark),
            "S" => Ok(Self::Space),
            _ => Err(SerialError::Value(format!("Not a valid parity: '{name}'"))),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::None => "N",
            Self::Even => "E",
            Self::Odd => "O",
            Self::Mark => "M",
            Self::Space => "S",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopBits {
    One,
    OnePointFive,
    Two,
}

impl StopBits {
    pub fn from_value(value: f64) -> Result<Self, SerialError> {
        // Message text matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt).
        if value == 1.0 {
            Ok(Self::One)
        } else if value == 1.5 {
            Ok(Self::OnePointFive)
        } else if value == 2.0 {
            Ok(Self::Two)
        } else {
            Err(SerialError::Value(format!(
                "Not a valid stop bit size: {value:?}"
            )))
        }
    }
}

/// Port configuration, with pyserial's defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub baudrate: u32,
    pub bytesize: u8,
    pub parity: Parity,
    pub stopbits: StopBits,
    pub timeout: Option<f64>,
    pub write_timeout: Option<f64>,
    pub inter_byte_timeout: Option<f64>,
    pub xonxoff: bool,
    pub rtscts: bool,
    pub dsrdtr: bool,
    pub exclusive: Option<bool>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            baudrate: 9600,
            bytesize: 8,
            parity: Parity::None,
            stopbits: StopBits::One,
            timeout: None,
            write_timeout: None,
            inter_byte_timeout: None,
            xonxoff: false,
            rtscts: false,
            dsrdtr: false,
            exclusive: None,
        }
    }
}

pub fn baudrate(value: i64) -> Result<u32, SerialError> {
    // Message text matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt).
    u32::try_from(value).map_err(|_| SerialError::Value(format!("Not a valid baudrate: {value}")))
}

pub fn bytesize(value: i64) -> Result<u8, SerialError> {
    // Message text matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt).
    match value {
        5..=8 => Ok(value as u8),
        _ => Err(SerialError::Value(format!(
            "Not a valid byte size: {value}"
        ))),
    }
}

/// Validates a timeout in seconds; `None` means no timeout.
pub fn seconds(value: Option<f64>) -> Result<Option<f64>, SerialError> {
    // Message text matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt).
    match value {
        Some(v) if v.is_nan() || v < 0.0 => {
            Err(SerialError::Value(format!("Not a valid timeout: {v:?}")))
        }
        _ => Ok(value),
    }
}

/// Converts timeout to Duration; values that overflow become None (no deadline).
pub fn duration(value: Option<f64>) -> Option<Duration> {
    value.and_then(|s| Duration::try_from_secs_f64(s).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_values_pyserial_rejects() {
        assert!(baudrate(-1).is_err());
        assert!(bytesize(9).is_err());
        assert!(Parity::from_name("X").is_err());
        assert!(StopBits::from_value(3.0).is_err());
        assert!(seconds(Some(-0.5)).is_err());
        assert!(seconds(Some(f64::NAN)).is_err());
    }

    #[test]
    fn accepts_every_documented_value() -> Result<(), SerialError> {
        for name in ["N", "E", "O", "M", "S"] {
            assert_eq!(Parity::from_name(name)?.name(), name);
        }
        for value in [1.0, 1.5, 2.0] {
            assert!(StopBits::from_value(value).is_ok());
        }
        for size in 5..=8 {
            assert!(bytesize(size).is_ok());
        }
        Ok(())
    }

    #[test]
    fn huge_and_infinite_timeouts_mean_no_deadline() {
        assert!(seconds(Some(f64::INFINITY)).is_ok());
        assert!(seconds(Some(1e300)).is_ok());
        assert_eq!(duration(Some(f64::INFINITY)), None);
        assert_eq!(duration(Some(1e300)), None);
        assert_eq!(duration(Some(1.5)), Some(Duration::from_millis(1500)));
    }
}
