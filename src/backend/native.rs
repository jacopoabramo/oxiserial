use serialport::{ClearBuffer, DataBits, FlowControl, SerialPort};
#[cfg(unix)]
use tokio_serial::{SerialPortBuilderExt, SerialStream};

use crate::backend::{Backend, Drain};
use crate::errors::SerialError;
#[cfg(unix)]
use crate::runtime::runtime;
use crate::settings::{Parity, Settings, StopBits};

#[cfg(unix)]
type Port = SerialStream;
#[cfg(windows)]
use crate::backend::overlapped::Port;

pub fn open(port: &str, settings: &Settings) -> Result<Box<dyn Backend>, SerialError> {
    #[cfg(unix)]
    let mut stream = {
        // The stream registers with the reactor of whichever runtime is current.
        let _context = runtime()?.enter();
        tokio_serial::new(port, settings.baudrate)
            .exclusive(settings.exclusive == Some(true))
            .open_native_async()
            .map_err(|err| SerialError::open_failed(port, err.into()))?
    };
    #[cfg(windows)]
    let mut stream = Port::open(port).map_err(|err| SerialError::open_failed(port, err.into()))?;
    stream.configure(settings)?;
    Ok(Box::new(stream))
}

impl Backend for Port {
    fn configure(&mut self, settings: &Settings) -> Result<(), SerialError> {
        #[cfg(all(unix, not(target_os = "linux")))]
        if matches!(settings.parity, Parity::Mark | Parity::Space) {
            // Message text matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt).
            return Err(SerialError::Value(format!(
                "Invalid parity: '{}'",
                settings.parity.name()
            )));
        }
        self.set_data_bits(match settings.bytesize {
            5 => DataBits::Five,
            6 => DataBits::Six,
            7 => DataBits::Seven,
            _ => DataBits::Eight,
        })?;
        self.set_parity(match settings.parity {
            Parity::Even => serialport::Parity::Even,
            Parity::Odd => serialport::Parity::Odd,
            _ => serialport::Parity::None,
        })?;
        self.set_stop_bits(match settings.stopbits {
            StopBits::One => serialport::StopBits::One,
            // Windows drivers may reject two stop bits with 1.5 requested, so `platform::apply` sets it directly.
            #[cfg(windows)]
            StopBits::OnePointFive => serialport::StopBits::One,
            _ => serialport::StopBits::Two,
        })?;
        // serialport has one flow-control mode, so software flow control is added by `platform::apply`.
        self.set_flow_control(if settings.rtscts {
            FlowControl::Hardware
        } else {
            FlowControl::None
        })?;
        platform::apply(self, settings)?;
        Ok(self.set_baud_rate(settings.baudrate)?)
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
        Ok(self.clear(which)?)
    }

    fn set_break_state(&self, on: bool) -> Result<(), SerialError> {
        if on {
            Ok(self.set_break()?)
        } else {
            Ok(self.clear_break()?)
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

    fn drain_handle(&self) -> Result<Drain, SerialError> {
        #[cfg(unix)]
        {
            use std::os::fd::{AsRawFd, BorrowedFd};

            // SAFETY: the descriptor belongs to `self`, which outlives this borrow.
            let fd = unsafe { BorrowedFd::borrow_raw(self.as_raw_fd()) };
            Ok(Drain::Fd(fd.try_clone_to_owned()?))
        }
        #[cfg(not(unix))]
        {
            Ok(Drain::PollOutWaiting)
        }
    }

    #[cfg(windows)]
    fn cancel_write(&mut self) -> Result<usize, SerialError> {
        Ok(self.abort_write()?)
    }

    #[cfg(windows)]
    fn detach_write(&mut self) -> usize {
        Port::detach_write(self)
    }
}

#[cfg(unix)]
mod platform {
    use std::os::fd::{AsRawFd, BorrowedFd};

    #[cfg(target_os = "macos")]
    use nix::sys::termios::{BaudRate, cfsetspeed};
    use nix::sys::termios::{InputFlags, SetArg, tcgetattr, tcsetattr};
    use tokio_serial::SerialStream;

    use crate::errors::SerialError;
    use crate::settings::Settings;

    /// Applies what serialport does not: raw parity errors, software flow control and, on Linux, mark and space parity.
    pub fn apply(stream: &SerialStream, settings: &Settings) -> Result<(), SerialError> {
        // SAFETY: the descriptor belongs to `stream`, which outlives this call.
        let fd = unsafe { BorrowedFd::borrow_raw(stream.as_raw_fd()) };
        // Matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt): termios flag choices.
        let mut termios = tcgetattr(fd).map_err(std::io::Error::from)?;
        termios
            .input_flags
            .remove(InputFlags::INPCK | InputFlags::ISTRIP);
        termios
            .input_flags
            .set(InputFlags::IXON | InputFlags::IXOFF, settings.xonxoff);
        if !settings.xonxoff {
            termios.input_flags.remove(InputFlags::IXANY);
        }
        #[cfg(target_os = "linux")]
        mark_space(&mut termios.control_flags, settings.parity);
        // IOSSIOSPEED leaves a speed that tcsetattr rejects; the real rate is set afterwards.
        #[cfg(target_os = "macos")]
        cfsetspeed(&mut termios, BaudRate::B9600).map_err(std::io::Error::from)?;
        tcsetattr(fd, SetArg::TCSANOW, &termios).map_err(std::io::Error::from)?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn mark_space(flags: &mut nix::sys::termios::ControlFlags, parity: crate::settings::Parity) {
        use nix::sys::termios::ControlFlags;

        use crate::settings::Parity;

        // Matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt): CMSPAR parity.
        flags.remove(ControlFlags::CMSPAR);
        match parity {
            Parity::Mark => {
                flags.insert(ControlFlags::PARENB | ControlFlags::CMSPAR | ControlFlags::PARODD);
            }
            Parity::Space => {
                flags.insert(ControlFlags::PARENB | ControlFlags::CMSPAR);
                flags.remove(ControlFlags::PARODD);
            }
            _ => {}
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Devices::Communication::{
        DCB, GetCommState, MARKPARITY, ONE5STOPBITS, SPACEPARITY, SetCommState,
    };

    use super::Port;
    use crate::errors::SerialError;
    use crate::settings::{Parity, Settings, StopBits};

    // Bit positions of the DCB flags, which windows-sys exposes only as one packed u32.
    const F_BINARY: u32 = 1;
    const F_PARITY: u32 = 1 << 1;
    const F_OUTX_DSR_FLOW: u32 = 1 << 3;
    const F_DTR_CONTROL_MASK: u32 = 0b11 << 4;
    const F_DTR_CONTROL_ENABLE: u32 = 1 << 4;
    const F_DTR_CONTROL_HANDSHAKE: u32 = 2 << 4;
    const F_DSR_SENSITIVITY: u32 = 1 << 6;
    const F_OUT_X: u32 = 1 << 8;
    const F_IN_X: u32 = 1 << 9;
    const F_ERROR_CHAR: u32 = 1 << 10;
    const F_NULL: u32 = 1 << 11;
    const F_RTS_CONTROL_MASK: u32 = 0b11 << 12;
    const F_RTS_CONTROL_HANDSHAKE: u32 = 2 << 12;
    const F_ABORT_ON_ERROR: u32 = 1 << 14;

    /// Adds binary mode, mark and space parity, 1.5 stop bits, software flow control and DSR/DTR and RTS handshaking.
    pub fn apply(stream: &Port, settings: &Settings) -> Result<(), SerialError> {
        let handle = stream.as_raw_handle();
        let mut dcb = DCB {
            DCBlength: std::mem::size_of::<DCB>() as u32,
            ..DCB::default()
        };
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
        // Matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt): DCB flag choices.
        dcb._bitfield &= !(F_OUTX_DSR_FLOW
            | F_DTR_CONTROL_MASK
            | F_DSR_SENSITIVITY
            | F_OUT_X
            | F_IN_X
            | F_ERROR_CHAR
            | F_NULL
            | F_ABORT_ON_ERROR);
        dcb._bitfield |= F_BINARY;
        dcb.XonChar = 0x11;
        dcb.XoffChar = 0x13;
        dcb._bitfield |= if settings.dsrdtr {
            F_OUTX_DSR_FLOW | F_DTR_CONTROL_HANDSHAKE
        } else {
            F_DTR_CONTROL_ENABLE
        };
        if settings.xonxoff {
            dcb._bitfield |= F_OUT_X | F_IN_X;
        }
        if settings.rtscts {
            dcb._bitfield &= !F_RTS_CONTROL_MASK;
            dcb._bitfield |= F_RTS_CONTROL_HANDSHAKE;
        }
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

    #[cfg_attr(
        target_os = "macos",
        ignore = "serialport sets termios through an ioctl that macOS ptys reject"
    )]
    #[tokio::test]
    async fn configured_pty_pair_carries_data() -> Result<(), Box<dyn std::error::Error>> {
        let (mut a, mut b) = SerialStream::pair()?;
        let settings = Settings {
            baudrate: 115_200,
            stopbits: StopBits::OnePointFive,
            ..Settings::default()
        };
        a.configure(&settings)?;
        b.configure(&settings)?;
        a.write_all(b"ping").await?;
        let mut buf = [0u8; 4];
        b.read_exact(&mut buf).await?;
        assert_eq!(&buf, b"ping");
        Ok(())
    }

    #[cfg_attr(
        not(feature = "test-backend"),
        allow(clippy::infallible_destructuring_match)
    )]
    #[tokio::test]
    async fn drain_handle_gives_a_descriptor_that_drains() -> Result<(), Box<dyn std::error::Error>>
    {
        let (mut a, mut b) = SerialStream::pair()?;
        a.write_all(b"x").await?;
        let fd = match a.drain_handle()? {
            Drain::Fd(fd) => fd,
            #[cfg(feature = "test-backend")]
            Drain::Done => return Err("expected a descriptor".into()),
        };
        nix::sys::termios::tcdrain(&fd)?;
        let mut buf = [0u8; 1];
        b.read_exact(&mut buf).await?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mark_and_space_parity_set_the_termios_bits_on_linux()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::fd::{AsRawFd, BorrowedFd};

        use nix::sys::termios::{ControlFlags, tcgetattr};

        let (mut a, _b) = SerialStream::pair()?;
        // SAFETY: the descriptor belongs to `a`, which outlives this borrow.
        let fd = unsafe { BorrowedFd::borrow_raw(a.as_raw_fd()) };
        let flags = || tcgetattr(fd).map(|attrs| attrs.control_flags);

        a.configure(&Settings {
            parity: Parity::Mark,
            ..Settings::default()
        })?;
        let mark = ControlFlags::PARENB | ControlFlags::CMSPAR | ControlFlags::PARODD;
        assert!(flags()?.contains(mark));

        a.configure(&Settings {
            parity: Parity::Space,
            ..Settings::default()
        })?;
        assert!(flags()?.contains(ControlFlags::PARENB | ControlFlags::CMSPAR));
        assert!(!flags()?.contains(ControlFlags::PARODD));

        a.configure(&Settings::default())?;
        assert!(!flags()?.intersects(ControlFlags::CMSPAR | ControlFlags::PARENB));
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn mark_parity_is_rejected_on_macos() -> Result<(), Box<dyn std::error::Error>> {
        let (mut a, _b) = SerialStream::pair()?;
        let settings = Settings {
            parity: Parity::Mark,
            ..Settings::default()
        };
        assert!(matches!(a.configure(&settings), Err(SerialError::Value(_))));
        Ok(())
    }
}
