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
    /// `code` is the OS error code: errno on POSIX, the Windows error code on Windows.
    Os {
        code: Option<i32>,
        message: String,
    },
    Timeout(String),
    NotOpen,
    AlreadyOpen,
    NoPort,
    Value(String),
    Cancelled,
    Forked,
}

// Message text matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt).
const DISCONNECTED: &str = "device reports readiness to read but returned no data \
                            (device disconnected or multiple access on port?)";

#[cfg(unix)]
const GONE: [i32; 3] = [libc::EIO, libc::ENXIO, libc::ENODEV];

// ERROR_OPERATION_ABORTED is left out: it is what cancelling our own operation reports.
#[cfg(windows)]
const GONE: [i32; 4] = {
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_BAD_COMMAND, ERROR_DEVICE_REMOVED, ERROR_GEN_FAILURE,
    };
    [
        ERROR_BAD_COMMAND as i32,
        ERROR_DEVICE_REMOVED as i32,
        ERROR_GEN_FAILURE as i32,
        ERROR_ACCESS_DENIED as i32,
    ]
};

impl SerialError {
    /// A read that reported readiness but returned no bytes.
    pub fn disconnected() -> Self {
        Self::Os {
            code: None,
            message: DISCONNECTED.into(),
        }
    }

    /// Whether a read or write on an open port failed because the device is gone.
    pub fn device_gone(&self) -> bool {
        match self {
            Self::Os {
                code: Some(code), ..
            } => GONE.contains(code),
            Self::Os {
                code: None,
                message,
            } => message == DISCONNECTED,
            _ => false,
        }
    }

    /// Wraps a failure to open `port` in pyserial's message, keeping its error code.
    pub fn open_failed(port: &str, err: SerialError) -> Self {
        let code = match &err {
            Self::Os { code, .. } => *code,
            _ => None,
        };
        // Message text matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt), which quotes the name only on Windows.
        let message = match code {
            _ if cfg!(windows) => format!("could not open port '{port}': {err}"),
            Some(errno) => format!("could not open port {port}: [Errno {errno}] {err}"),
            None => format!("could not open port {port}: {err}"),
        };
        Self::Os { code, message }
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
            code: err.raw_os_error(),
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
                code: errno_of(&err.description),
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
                code: Some(code), ..
            } => os_exception(code, message),
            SerialError::Os { code: None, .. }
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

/// `SerialException(errno, message)`, as pyserial raises it.
#[cfg(not(windows))]
fn os_exception(errno: i32, message: String) -> PyErr {
    SerialException::new_err((errno, message))
}

/// `SerialException(None, message, None, winerror)`, which sets `winerror` and the matching
/// `errno` the way `OSError` does for a Windows error.
#[cfg(windows)]
fn os_exception(winerror: i32, message: String) -> PyErr {
    SerialException::new_err((None::<i32>, message, None::<i32>, winerror))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_errors_keep_their_code() {
        let err = SerialError::from(std::io::Error::from_raw_os_error(2));
        assert!(matches!(err, SerialError::Os { code: Some(2), .. }));
    }

    #[test]
    fn device_gone_covers_removal_but_not_a_cancelled_operation() {
        let os = |code| SerialError::from(std::io::Error::from_raw_os_error(code));
        #[cfg(unix)]
        let (gone, other) = (libc::ENODEV, libc::EAGAIN);
        #[cfg(windows)]
        let (gone, other) = (
            windows_sys::Win32::Foundation::ERROR_DEVICE_REMOVED as i32,
            windows_sys::Win32::Foundation::ERROR_OPERATION_ABORTED as i32,
        );
        assert!(os(gone).device_gone());
        assert!(SerialError::disconnected().device_gone());
        assert!(!os(other).device_gone());
        assert!(!SerialError::NotOpen.device_gone());
    }

    #[cfg(unix)]
    #[test]
    fn serialport_errors_recover_the_errno_pyserial_checks() {
        let description = nix::errno::Errno::ENOTTY.desc();
        let err = serialport::Error::new(serialport::ErrorKind::Unknown, description);
        assert_eq!(
            SerialError::from(err),
            SerialError::Os {
                code: Some(libc::ENOTTY),
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
            Some(SerialError::Os { code: Some(libc::ENOENT), message })
                if message.starts_with("could not open port /dev/oxiserial-does-not-exist: [Errno 2]")
        ));
    }
}
