use pyo3::create_exception;
use pyo3::exceptions::asyncio::CancelledError;
use pyo3::exceptions::{PyOSError, PyValueError};
use pyo3::prelude::*;

create_exception!(oxiserial, SerialException, PyOSError);
create_exception!(oxiserial, SerialTimeoutException, SerialException);
create_exception!(oxiserial, PortNotOpenError, SerialException);

/// Failure of a port operation, convertible to the matching Python exception.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SerialError {
    Os { errno: Option<i32>, message: String },
    Timeout(String),
    NotOpen,
    AlreadyOpen,
    NoPort,
    Value(String),
    Cancelled,
    Forked,
}

impl SerialError {
    /// A read that reported readiness but returned no bytes.
    pub fn disconnected() -> Self {
        Self::Os {
            errno: None,
            message: "device reports readiness to read but returned no data \
                      (device disconnected or multiple access on port?)"
                .into(),
        }
    }

    pub fn open_failed(port: &str, err: tokio_serial::Error) -> Self {
        Self::Os {
            errno: None,
            message: format!("could not open port '{port}': {}", err.description),
        }
    }
}

impl From<std::io::Error> for SerialError {
    fn from(err: std::io::Error) -> Self {
        Self::Os {
            errno: err.raw_os_error(),
            message: err.to_string(),
        }
    }
}

/// Recovers the errno from a serialport error description.
fn errno_of(description: &str) -> Option<i32> {
    if let Some((_, code)) = description.rsplit_once("(os error ") {
        return code.strip_suffix(')')?.parse().ok();
    }
    // Errors that serialport converts from nix keep only the strerror text.
    #[cfg(unix)]
    for errno in [libc::EINVAL, libc::ENOTTY] {
        if std::io::Error::from_raw_os_error(errno)
            .to_string()
            .starts_with(description)
        {
            return Some(errno);
        }
    }
    None
}

impl From<tokio_serial::Error> for SerialError {
    fn from(err: tokio_serial::Error) -> Self {
        match err.kind {
            tokio_serial::ErrorKind::InvalidInput => Self::Value(err.description),
            _ => Self::Os {
                errno: errno_of(&err.description),
                message: err.description,
            },
        }
    }
}

impl From<SerialError> for PyErr {
    fn from(err: SerialError) -> PyErr {
        match err {
            SerialError::Os {
                errno: Some(errno),
                message,
            } => SerialException::new_err((errno, message)),
            SerialError::Os {
                errno: None,
                message,
            } => SerialException::new_err(message),
            SerialError::Timeout(message) => SerialTimeoutException::new_err(message),
            SerialError::NotOpen => {
                PortNotOpenError::new_err("Attempting to use a port that is not open")
            }
            SerialError::AlreadyOpen => SerialException::new_err("Port is already open."),
            SerialError::NoPort => {
                SerialException::new_err("Port must be configured before it can be used.")
            }
            SerialError::Value(message) => PyValueError::new_err(message),
            SerialError::Cancelled => CancelledError::new_err(()),
            SerialError::Forked => SerialException::new_err(
                "oxiserial cannot be used in a child process created by fork()",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_errors_keep_their_errno() {
        let err = SerialError::from(std::io::Error::from_raw_os_error(2));
        assert!(matches!(err, SerialError::Os { errno: Some(2), .. }));
    }

    #[cfg(unix)]
    #[test]
    fn serialport_errors_recover_the_errno_pyserial_checks() {
        let description = std::io::Error::from_raw_os_error(libc::ENOTTY).to_string();
        let description = description.split(" (os error").next().unwrap();
        let err = tokio_serial::Error::new(tokio_serial::ErrorKind::Unknown, description);
        assert_eq!(
            SerialError::from(err),
            SerialError::Os {
                errno: Some(libc::ENOTTY),
                message: description.into()
            }
        );
    }
}
