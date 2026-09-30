use tokio::io::{AsyncRead, AsyncWrite};

use crate::errors::SerialError;
use crate::settings::Settings;

mod loopback;
#[cfg(feature = "test-backend")]
pub mod mock;
pub mod native;
#[cfg(windows)]
mod overlapped;

/// What `PortCore::flush` waits on once the backend lock is released.
pub enum Drain {
    /// Nothing is buffered by the backend.
    Done,
    /// A duplicate of the port descriptor to run `tcdrain` on.
    #[cfg(unix)]
    Fd(std::os::fd::OwnedFd),
    /// Poll `out_waiting` until it reaches zero.
    #[cfg(not(unix))]
    PollOutWaiting,
}

/// An open port: async byte I/O plus the control calls pyserial exposes.
pub trait Backend: AsyncRead + AsyncWrite + Unpin + Send + 'static {
    /// Applies `settings`, leaving RTS and DTR at the given levels where the backend drives them.
    fn configure(&mut self, settings: &Settings, rts: bool, dtr: bool) -> Result<(), SerialError>;
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
    fn drain_handle(&self) -> Result<Drain, SerialError>;

    /// Cancels a write left in progress by an abandoned `poll_write` and returns the bytes it sent.
    fn cancel_write(&mut self) -> Result<usize, SerialError> {
        Ok(0)
    }

    /// Leaves a write that `poll_write` has not finished to complete on its own and returns
    /// the bytes handed to the device, for a non-blocking write.
    fn detach_write(&mut self) -> usize {
        0
    }

    /// Sets the driver's input and output queue sizes; backends without driver queues keep
    /// nothing to size.
    #[cfg(windows)]
    fn set_buffer_size(&mut self, _rx: u32, _tx: u32) -> Result<(), SerialError> {
        Ok(())
    }
}

/// Opens `port` and applies `settings` and the RTS and DTR levels.
///
/// A name starting with `loop://` opens a new loopback port.
pub fn open(
    port: &str,
    settings: &Settings,
    rts: bool,
    dtr: bool,
) -> Result<Box<dyn Backend>, SerialError> {
    if loopback::is_loopback(port) {
        let mut port = loopback::LoopbackPort::default();
        port.configure(settings, rts, dtr)?;
        return Ok(Box::new(port));
    }
    #[cfg(feature = "test-backend")]
    if let Some(mut mock) = mock::lookup(port) {
        mock.configure(settings, rts, dtr)?;
        return Ok(Box::new(mock));
    }
    native::open(port, settings, rts, dtr)
}
