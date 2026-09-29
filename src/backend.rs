use tokio::io::{AsyncRead, AsyncWrite};

use crate::errors::SerialError;
use crate::settings::Settings;

#[cfg(feature = "test-backend")]
pub mod mock;
pub mod native;

/// An open port: async byte I/O plus the control calls pyserial exposes.
pub trait Backend: AsyncRead + AsyncWrite + Unpin + Send + 'static {
    fn configure(&mut self, settings: &Settings) -> Result<(), SerialError>;
    fn set_rts(&mut self, level: bool) -> Result<(), SerialError>;
    fn set_dtr(&mut self, level: bool) -> Result<(), SerialError>;
    fn cts(&mut self) -> Result<bool, SerialError>;
    fn dsr(&mut self) -> Result<bool, SerialError>;
    fn ri(&mut self) -> Result<bool, SerialError>;
    fn cd(&mut self) -> Result<bool, SerialError>;
    fn in_waiting(&self) -> Result<usize, SerialError>;
    fn out_waiting(&self) -> Result<usize, SerialError>;
    fn clear_buffers(&self, input: bool, output: bool) -> Result<(), SerialError>;
    fn set_break_state(&self, on: bool) -> Result<(), SerialError>;
    fn fileno(&self) -> Option<i32>;
}

/// Opens `port` and applies `settings`.
pub fn open(port: &str, settings: &Settings) -> Result<Box<dyn Backend>, SerialError> {
    #[cfg(feature = "test-backend")]
    if let Some(mut mock) = mock::lookup(port) {
        mock.configure(settings)?;
        return Ok(Box::new(mock));
    }
    native::open(port, settings)
}
