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
    let builder = tokio_serial::new(port, settings.baudrate);
    #[cfg(unix)]
    let builder = builder.exclusive(settings.exclusive == Some(true));
    let mut stream = builder
        .open_native_async()
        .map_err(|err| SerialError::open_failed(port, err))?;
    stream.configure(settings)?;
    Ok(Box::new(stream))
}

#[cfg(not(target_os = "macos"))]
fn set_baud_rate(stream: &mut SerialStream, baudrate: u32) -> Result<(), SerialError> {
    Ok(SerialPort::set_baud_rate(stream, baudrate)?)
}

/// serialport sets every rate through an ioctl that ptys reject; termios handles the standard ones.
#[cfg(target_os = "macos")]
fn set_baud_rate(stream: &mut SerialStream, baudrate: u32) -> Result<(), SerialError> {
    use std::os::fd::{AsRawFd, BorrowedFd};

    use nix::sys::termios::{BaudRate, SetArg, cfsetspeed, tcgetattr, tcsetattr};

    if BaudRate::try_from(baudrate as libc::speed_t).is_err() {
        return Ok(SerialPort::set_baud_rate(stream, baudrate)?);
    }
    // SAFETY: the descriptor belongs to `stream`, which outlives this call.
    let fd = unsafe { BorrowedFd::borrow_raw(stream.as_raw_fd()) };
    let mut termios = tcgetattr(fd).map_err(std::io::Error::from)?;
    cfsetspeed(&mut termios, baudrate).map_err(std::io::Error::from)?;
    tcsetattr(fd, SetArg::TCSANOW, &termios).map_err(std::io::Error::from)?;
    Ok(())
}

impl Backend for SerialStream {
    fn configure(&mut self, settings: &Settings) -> Result<(), SerialError> {
        #[cfg(all(unix, not(target_os = "linux")))]
        if matches!(settings.parity, Parity::Mark | Parity::Space) {
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
            Parity::Even => tokio_serial::Parity::Even,
            Parity::Odd => tokio_serial::Parity::Odd,
            _ => tokio_serial::Parity::None,
        })?;
        self.set_stop_bits(match settings.stopbits {
            StopBits::One => tokio_serial::StopBits::One,
            // Windows drivers may reject two stop bits with 1.5 requested, so `platform::apply` sets it directly.
            #[cfg(windows)]
            StopBits::OnePointFive => tokio_serial::StopBits::One,
            _ => tokio_serial::StopBits::Two,
        })?;
        // serialport has one flow-control mode, so software flow control is added by `platform::apply`.
        self.set_flow_control(if settings.rtscts {
            FlowControl::Hardware
        } else {
            FlowControl::None
        })?;
        platform::apply(self, settings)?;
        set_baud_rate(self, settings.baudrate)
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

#[cfg(unix)]
mod platform {
    use std::os::fd::{AsRawFd, BorrowedFd};

    use nix::sys::termios::{InputFlags, SetArg, tcgetattr, tcsetattr};
    use tokio_serial::SerialStream;

    use crate::errors::SerialError;
    use crate::settings::Settings;

    /// Applies what serialport does not: raw parity errors, software flow control and, on Linux, mark and space parity.
    pub fn apply(stream: &SerialStream, settings: &Settings) -> Result<(), SerialError> {
        // SAFETY: the descriptor belongs to `stream`, which outlives this call.
        let fd = unsafe { BorrowedFd::borrow_raw(stream.as_raw_fd()) };
        let mut termios = tcgetattr(fd).map_err(std::io::Error::from)?;
        termios
            .input_flags
            .remove(InputFlags::INPCK | InputFlags::ISTRIP);
        termios
            .input_flags
            .set(InputFlags::IXON | InputFlags::IXOFF, settings.xonxoff);
        #[cfg(target_os = "linux")]
        mark_space(&mut termios.control_flags, settings.parity);
        tcsetattr(fd, SetArg::TCSANOW, &termios).map_err(std::io::Error::from)?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn mark_space(flags: &mut nix::sys::termios::ControlFlags, parity: crate::settings::Parity) {
        use nix::sys::termios::ControlFlags;

        use crate::settings::Parity;

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
    const F_OUT_X: u32 = 1 << 8;
    const F_IN_X: u32 = 1 << 9;
    const F_RTS_CONTROL_MASK: u32 = 0b11 << 12;
    const F_RTS_CONTROL_HANDSHAKE: u32 = 2 << 12;

    /// Adds mark and space parity, 1.5 stop bits, software flow control and DSR/DTR and RTS handshaking.
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
        dcb._bitfield &= !(F_OUTX_DSR_FLOW | F_DTR_CONTROL_MASK | F_OUT_X | F_IN_X);
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

    #[cfg(target_os = "linux")]
    #[test]
    fn mark_and_space_parity_set_the_termios_bits_on_linux() {
        use std::os::fd::{AsRawFd, BorrowedFd};

        use nix::sys::termios::{ControlFlags, tcgetattr};

        let (mut a, _b) = SerialStream::pair().unwrap();
        // SAFETY: the descriptor belongs to `a`, which outlives this borrow.
        let fd = unsafe { BorrowedFd::borrow_raw(a.as_raw_fd()) };
        let flags = || tcgetattr(fd).unwrap().control_flags;

        a.configure(&Settings {
            parity: Parity::Mark,
            ..Settings::default()
        })
        .unwrap();
        let mark = ControlFlags::PARENB | ControlFlags::CMSPAR | ControlFlags::PARODD;
        assert!(flags().contains(mark));

        a.configure(&Settings {
            parity: Parity::Space,
            ..Settings::default()
        })
        .unwrap();
        assert!(flags().contains(ControlFlags::PARENB | ControlFlags::CMSPAR));
        assert!(!flags().contains(ControlFlags::PARODD));

        a.configure(&Settings::default()).unwrap();
        assert!(!flags().intersects(ControlFlags::CMSPAR | ControlFlags::PARENB));
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
