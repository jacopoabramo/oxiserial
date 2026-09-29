use tokio_serial::{
    ClearBuffer, DataBits, FlowControl, SerialPort, SerialPortBuilderExt, SerialStream,
};

use crate::backend::Backend;
use crate::errors::SerialError;
use crate::runtime::runtime;
use crate::settings::{Parity, Settings, StopBits};

pub fn open(port: &str, settings: &Settings) -> Result<Box<dyn Backend>, SerialError> {
    // The stream registers with the reactor of whichever runtime is current.
    let _context = runtime()?.enter();
    let mut stream = tokio_serial::new(port, settings.baudrate)
        .open_native_async()
        .map_err(|err| SerialError::open_failed(port, err))?;
    #[cfg(unix)]
    stream.set_exclusive(settings.exclusive == Some(true))?;
    stream.configure(settings)?;
    Ok(Box::new(stream))
}

impl Backend for SerialStream {
    fn configure(&mut self, settings: &Settings) -> Result<(), SerialError> {
        self.set_baud_rate(settings.baudrate)?;
        self.set_data_bits(match settings.bytesize {
            5 => DataBits::Five,
            6 => DataBits::Six,
            7 => DataBits::Seven,
            _ => DataBits::Eight,
        })?;
        self.set_parity(match settings.parity {
            Parity::Even => tokio_serial::Parity::Even,
            Parity::Odd => tokio_serial::Parity::Odd,
            _ => tokio_serial::Parity::None,
        })?;
        self.set_stop_bits(match settings.stopbits {
            StopBits::One => tokio_serial::StopBits::One,
            _ => tokio_serial::StopBits::Two,
        })?;
        // ponytail: serialport has one flow-control mode, so xonxoff together with rtscts keeps only hardware flow control
        self.set_flow_control(if settings.rtscts {
            FlowControl::Hardware
        } else if settings.xonxoff {
            FlowControl::Software
        } else {
            FlowControl::None
        })?;
        platform::apply(self, settings)
    }

    fn set_rts(&mut self, level: bool) -> Result<(), SerialError> {
        Ok(self.write_request_to_send(level)?)
    }

    fn set_dtr(&mut self, level: bool) -> Result<(), SerialError> {
        Ok(self.write_data_terminal_ready(level)?)
    }

    fn cts(&mut self) -> Result<bool, SerialError> {
        Ok(self.read_clear_to_send()?)
    }

    fn dsr(&mut self) -> Result<bool, SerialError> {
        Ok(self.read_data_set_ready()?)
    }

    fn ri(&mut self) -> Result<bool, SerialError> {
        Ok(self.read_ring_indicator()?)
    }

    fn cd(&mut self) -> Result<bool, SerialError> {
        Ok(self.read_carrier_detect()?)
    }

    fn in_waiting(&self) -> Result<usize, SerialError> {
        Ok(self.bytes_to_read()? as usize)
    }

    fn out_waiting(&self) -> Result<usize, SerialError> {
        Ok(self.bytes_to_write()? as usize)
    }

    fn clear_buffers(&self, input: bool, output: bool) -> Result<(), SerialError> {
        let which = match (input, output) {
            (true, true) => ClearBuffer::All,
            (true, false) => ClearBuffer::Input,
            (false, true) => ClearBuffer::Output,
            (false, false) => return Ok(()),
        };
        Ok(SerialPort::clear(self, which)?)
    }

    fn set_break_state(&self, on: bool) -> Result<(), SerialError> {
        if on {
            Ok(SerialPort::set_break(self)?)
        } else {
            Ok(SerialPort::clear_break(self)?)
        }
    }

    fn fileno(&self) -> Option<i32> {
        #[cfg(unix)]
        {
            Some(std::os::fd::AsRawFd::as_raw_fd(self))
        }
        #[cfg(not(unix))]
        {
            None
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::os::fd::{AsRawFd, BorrowedFd};

    use nix::sys::termios::{ControlFlags, SetArg, tcgetattr, tcsetattr};
    use tokio_serial::SerialStream;

    use crate::errors::SerialError;
    use crate::settings::{Parity, Settings};

    /// Adds mark and space parity, which serialport does not offer.
    pub fn apply(stream: &SerialStream, settings: &Settings) -> Result<(), SerialError> {
        // SAFETY: the descriptor belongs to `stream`, which outlives this call.
        let fd = unsafe { BorrowedFd::borrow_raw(stream.as_raw_fd()) };
        let mut termios = tcgetattr(fd).map_err(std::io::Error::from)?;
        let flags = &mut termios.control_flags;
        flags.remove(ControlFlags::CMSPAR);
        match settings.parity {
            Parity::Mark => {
                flags.insert(ControlFlags::PARENB | ControlFlags::CMSPAR | ControlFlags::PARODD);
            }
            Parity::Space => {
                flags.insert(ControlFlags::PARENB | ControlFlags::CMSPAR);
                flags.remove(ControlFlags::PARODD);
            }
            _ => {}
        }
        tcsetattr(fd, SetArg::TCSANOW, &termios).map_err(std::io::Error::from)?;
        Ok(())
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
mod platform {
    use tokio_serial::SerialStream;

    use crate::errors::SerialError;
    use crate::settings::{Parity, Settings};

    pub fn apply(_stream: &SerialStream, settings: &Settings) -> Result<(), SerialError> {
        match settings.parity {
            Parity::Mark | Parity::Space => Err(SerialError::Value(format!(
                "Invalid parity: '{}'",
                settings.parity.name()
            ))),
            _ => Ok(()),
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::os::windows::io::AsRawHandle;

    use tokio_serial::SerialStream;
    use windows_sys::Win32::Devices::Communication::{
        DCB, GetCommState, MARKPARITY, ONE5STOPBITS, SPACEPARITY, SetCommState,
    };

    use crate::errors::SerialError;
    use crate::settings::{Parity, Settings, StopBits};

    // Bit positions of the DCB flags, which windows-sys exposes only as one packed u32.
    const F_PARITY: u32 = 1 << 1;
    const F_OUTX_DSR_FLOW: u32 = 1 << 3;
    const F_DTR_CONTROL_MASK: u32 = 0b11 << 4;
    const F_DTR_CONTROL_ENABLE: u32 = 1 << 4;
    const F_DTR_CONTROL_HANDSHAKE: u32 = 2 << 4;

    /// Adds mark and space parity, 1.5 stop bits and DSR/DTR flow control.
    // ponytail: turning dsrdtr off sets DTR high until the next dtr assignment; pass the DTR state in if that matters
    pub fn apply(stream: &SerialStream, settings: &Settings) -> Result<(), SerialError> {
        let handle = stream.as_raw_handle();
        // SAFETY: DCB is plain data; zeroed is a valid value before GetCommState fills it.
        let mut dcb: DCB = unsafe { std::mem::zeroed() };
        dcb.DCBlength = std::mem::size_of::<DCB>() as u32;
        // SAFETY: `handle` is the open COM port owned by `stream`.
        if unsafe { GetCommState(handle, &mut dcb) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        match settings.parity {
            Parity::Mark => {
                dcb.Parity = MARKPARITY;
                dcb._bitfield |= F_PARITY;
            }
            Parity::Space => {
                dcb.Parity = SPACEPARITY;
                dcb._bitfield |= F_PARITY;
            }
            _ => {}
        }
        if settings.stopbits == StopBits::OnePointFive {
            dcb.StopBits = ONE5STOPBITS;
        }
        dcb._bitfield &= !(F_OUTX_DSR_FLOW | F_DTR_CONTROL_MASK);
        dcb._bitfield |= if settings.dsrdtr {
            F_OUTX_DSR_FLOW | F_DTR_CONTROL_HANDSHAKE
        } else {
            F_DTR_CONTROL_ENABLE
        };
        // SAFETY: as above.
        if unsafe { SetCommState(handle, &dcb) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_serial::SerialStream;

    use super::*;

    #[tokio::test]
    async fn configured_pty_pair_carries_data() {
        let (mut a, mut b) = SerialStream::pair().unwrap();
        let settings = Settings {
            baudrate: 115_200,
            stopbits: StopBits::OnePointFive,
            ..Settings::default()
        };
        a.configure(&settings).unwrap();
        b.configure(&settings).unwrap();
        a.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        b.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn mark_parity_is_rejected_on_macos() {
        let (mut a, _b) = SerialStream::pair().unwrap();
        let settings = Settings {
            parity: Parity::Mark,
            ..Settings::default()
        };
        assert!(matches!(a.configure(&settings), Err(SerialError::Value(_))));
    }
}
