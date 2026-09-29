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
        // Message text matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt).
        Self::Os {
            errno: None,
            message: "device reports readiness to read but returned no data \
                      (device disconnected or multiple access on port?)"
                .into(),
        }
    }

    /// Wraps a failure to open `port` in pyserial's message, keeping its errno.
    pub fn open_failed(port: &str, err: SerialError) -> Self {
        let errno = match &err {
            Self::Os { errno, .. } => *errno,
            _ => None,
        };
        // Message text matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt), which quotes the name only on Windows.
        let message = match errno {
            _ if cfg!(windows) => format!("could not open port '{port}': {err}"),
            Some(errno) => format!("could not open port {port}: [Errno {errno}] {err}"),
            None => format!("could not open port {port}: {err}"),
        };
        Self::Os { errno, message }
    }
}

impl std::fmt::Display for SerialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Os { message, .. } | Self::Timeout(message) | Self::Value(message) => {
                f.write_str(message)
            }
            // Matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt): the next three messages.
            Self::NotOpen => f.write_str("Attempting to use a port that is not open"),
            Self::AlreadyOpen => f.write_str("Port is already open."),
            Self::NoPort => f.write_str("Port must be configured before it can be used."),
            Self::Cancelled => f.write_str("the operation was cancelled"),
            Self::Forked => {
                f.write_str("oxiserial cannot be used in a child process created by fork()")
            }
        }
    }
}

impl std::error::Error for SerialError {}

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
    // Errors that serialport converts from nix carry only nix's description text.
    #[cfg(unix)]
    {
        use nix::errno::Errno;
        (1..256).find(|&code| {
            let errno = Errno::from_raw(code);
            errno != Errno::UnknownErrno && errno.desc() == description
        })
    }
    #[cfg(not(unix))]
    None
}

impl From<serialport::Error> for SerialError {
    fn from(err: serialport::Error) -> Self {
        match err.kind {
            serialport::ErrorKind::InvalidInput => Self::Value(err.description),
            _ => Self::Os {
                errno: errno_of(&err.description),
                message: err.description,
            },
        }
    }
}

impl From<SerialError> for PyErr {
    fn from(err: SerialError) -> PyErr {
        let message = err.to_string();
        match err {
            SerialError::Os {
                errno: Some(errno), ..
            } => SerialException::new_err((errno, message)),
            SerialError::Os { errno: None, .. }
            | SerialError::AlreadyOpen
            | SerialError::NoPort
            | SerialError::Forked => SerialException::new_err(message),
            SerialError::Timeout(_) => SerialTimeoutException::new_err(message),
            SerialError::NotOpen => PortNotOpenError::new_err(message),
            SerialError::Value(_) => PyValueError::new_err(message),
            SerialError::Cancelled => CancelledError::new_err(()),
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
        let description = nix::errno::Errno::ENOTTY.desc();
        let err = serialport::Error::new(serialport::ErrorKind::Unknown, description);
        assert_eq!(
            SerialError::from(err),
            SerialError::Os {
                errno: Some(libc::ENOTTY),
                message: description.into()
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn opening_a_missing_port_keeps_enoent() {
        let path = "/dev/oxiserial-does-not-exist";
        let result =
            crate::backend::native::open(path, &crate::settings::Settings::default(), true, true);
        assert!(matches!(
            result.err(),
            Some(SerialError::Os { errno: Some(libc::ENOENT), message })
                if message.starts_with("could not open port /dev/oxiserial-does-not-exist: [Errno 2]")
        ));
    }
}
