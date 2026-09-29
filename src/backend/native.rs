use serialport::{ClearBuffer, SerialPort};
#[cfg(unix)]
use serialport::{DataBits, FlowControl};
#[cfg(unix)]
use tokio_serial::{SerialPortBuilderExt, SerialStream};

use crate::backend::{Backend, Drain};
use crate::errors::SerialError;
#[cfg(unix)]
use crate::runtime::runtime;
use crate::settings::Settings;
#[cfg(unix)]
use crate::settings::{Parity, StopBits};

#[cfg(unix)]
type Port = SerialStream;
#[cfg(windows)]
use crate::backend::overlapped::Port;

pub fn open(
    port: &str,
    settings: &Settings,
    rts: bool,
    dtr: bool,
) -> Result<Box<dyn Backend>, SerialError> {
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
    stream.configure(settings, rts, dtr)?;
    Ok(Box::new(stream))
}

impl Backend for Port {
    #[cfg(windows)]
    fn configure(&mut self, settings: &Settings, rts: bool, dtr: bool) -> Result<(), SerialError> {
        platform::configure(self, settings, rts, dtr)
    }

    // termios settings leave the modem lines alone, so the levels need not be written here.
    #[cfg(unix)]
    fn configure(
        &mut self,
        settings: &Settings,
        _rts: bool,
        _dtr: bool,
    ) -> Result<(), SerialError> {
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
    pub(super) fn mark_space(
        flags: &mut nix::sys::termios::ControlFlags,
        parity: crate::settings::Parity,
    ) {
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
        DCB, EVENPARITY, GetCommState, MARKPARITY, NOPARITY, ODDPARITY, ONE5STOPBITS, ONESTOPBIT,
        SPACEPARITY, SetCommState, TWOSTOPBITS,
    };
    use windows_sys::Win32::System::WindowsProgramming::{
        DTR_CONTROL_DISABLE, DTR_CONTROL_ENABLE, DTR_CONTROL_HANDSHAKE, RTS_CONTROL_DISABLE,
        RTS_CONTROL_ENABLE, RTS_CONTROL_HANDSHAKE,
    };

    use super::Port;
    use crate::errors::SerialError;
    use crate::settings::{Parity, Settings, StopBits};

    // Bit positions of the DCB flags, which windows-sys exposes only as one packed u32.
    const F_BINARY: u32 = 1;
    const F_PARITY: u32 = 1 << 1;
    const F_OUTX_CTS_FLOW: u32 = 1 << 2;
    const F_OUTX_DSR_FLOW: u32 = 1 << 3;
    const F_DTR_CONTROL_SHIFT: u32 = 4;
    const F_DSR_SENSITIVITY: u32 = 1 << 6;
    const F_OUT_X: u32 = 1 << 8;
    const F_IN_X: u32 = 1 << 9;
    const F_ERROR_CHAR: u32 = 1 << 10;
    const F_NULL: u32 = 1 << 11;
    const F_RTS_CONTROL_SHIFT: u32 = 12;
    const F_ABORT_ON_ERROR: u32 = 1 << 14;

    /// Applies `settings` and the RTS and DTR levels in a single `SetCommState`, so no line
    /// changes level while the port is reconfigured.
    pub fn configure(
        stream: &Port,
        settings: &Settings,
        rts: bool,
        dtr: bool,
    ) -> Result<(), SerialError> {
        let handle = stream.as_raw_handle();
        let mut dcb = DCB {
            DCBlength: std::mem::size_of::<DCB>() as u32,
            ..DCB::default()
        };
        // SAFETY: `handle` is the open COM port owned by `stream`.
        if unsafe { GetCommState(handle, &mut dcb) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // Matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt): DCB field and flag choices.
        dcb.BaudRate = settings.baudrate;
        dcb.ByteSize = settings.bytesize;
        dcb.Parity = match settings.parity {
            Parity::None => NOPARITY,
            Parity::Even => EVENPARITY,
            Parity::Odd => ODDPARITY,
            Parity::Mark => MARKPARITY,
            Parity::Space => SPACEPARITY,
        };
        dcb.StopBits = match settings.stopbits {
            StopBits::One => ONESTOPBIT,
            StopBits::OnePointFive => ONE5STOPBITS,
            StopBits::Two => TWOSTOPBITS,
        };
        let dtr_control = if settings.dsrdtr {
            DTR_CONTROL_HANDSHAKE
        } else if dtr {
            DTR_CONTROL_ENABLE
        } else {
            DTR_CONTROL_DISABLE
        };
        let rts_control = if settings.rtscts {
            RTS_CONTROL_HANDSHAKE
        } else if rts {
            RTS_CONTROL_ENABLE
        } else {
            RTS_CONTROL_DISABLE
        };
        let mut flags =
            F_BINARY | dtr_control << F_DTR_CONTROL_SHIFT | rts_control << F_RTS_CONTROL_SHIFT;
        if settings.parity != Parity::None {
            flags |= F_PARITY;
        }
        if settings.rtscts {
            flags |= F_OUTX_CTS_FLOW;
        }
        if settings.dsrdtr {
            flags |= F_OUTX_DSR_FLOW;
        }
        if settings.xonxoff {
            flags |= F_OUT_X | F_IN_X;
        }
        let owned = F_BINARY
            | F_PARITY
            | F_OUTX_CTS_FLOW
            | F_OUTX_DSR_FLOW
            | 0b11 << F_DTR_CONTROL_SHIFT
            | F_DSR_SENSITIVITY
            | F_OUT_X
            | F_IN_X
            | F_ERROR_CHAR
            | F_NULL
            | 0b11 << F_RTS_CONTROL_SHIFT
            | F_ABORT_ON_ERROR;
        dcb._bitfield = dcb._bitfield & !owned | flags;
        dcb.XonChar = 0x11;
        dcb.XoffChar = 0x13;
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
        a.configure(&settings, true, true)?;
        b.configure(&settings, true, true)?;
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
    fn mark_space_sets_and_clears_the_parity_bits() {
        use nix::sys::termios::ControlFlags;

        let mut flags = ControlFlags::empty();
        super::platform::mark_space(&mut flags, Parity::Mark);
        assert!(flags.contains(ControlFlags::PARENB | ControlFlags::CMSPAR | ControlFlags::PARODD));

        super::platform::mark_space(&mut flags, Parity::Space);
        assert!(flags.contains(ControlFlags::PARENB | ControlFlags::CMSPAR));
        assert!(!flags.contains(ControlFlags::PARODD));

        super::platform::mark_space(&mut flags, Parity::None);
        assert!(!flags.contains(ControlFlags::CMSPAR));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn mark_parity_is_rejected_on_macos() -> Result<(), Box<dyn std::error::Error>> {
        let (mut a, _b) = SerialStream::pair()?;
        let settings = Settings {
            parity: Parity::Mark,
            ..Settings::default()
        };
        assert!(matches!(
            a.configure(&settings, true, true),
            Err(SerialError::Value(_))
        ));
        Ok(())
    }
}
