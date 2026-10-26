//! The kernel's RS-485 mode, `TIOCSRS485`, which switches RTS in the driver.

use std::os::fd::RawFd;

use crate::errors::SerialError;
use crate::settings::Rs485;

// `struct serial_rs485` and its flags, from the kernel's include/uapi/linux/serial.h;
// libc has the ioctl numbers but not these.
#[repr(C)]
#[derive(Default)]
struct SerialRs485 {
    flags: u32,
    delay_rts_before_send: u32,
    delay_rts_after_send: u32,
    padding: [u32; 5],
}

const _: () = assert!(size_of::<SerialRs485>() == 32);

const SER_RS485_ENABLED: u32 = 1;
const SER_RS485_RTS_ON_SEND: u32 = 1 << 1;
const SER_RS485_RTS_AFTER_SEND: u32 = 1 << 2;
const SER_RS485_RX_DURING_TX: u32 = 1 << 4;
const ASKED_FLAGS: u32 =
    SER_RS485_ENABLED | SER_RS485_RTS_ON_SEND | SER_RS485_RTS_AFTER_SEND | SER_RS485_RX_DURING_TX;

// The kernel writes back the configuration it applied.
nix::ioctl_readwrite_bad!(tiocsrs485, libc::TIOCSRS485, SerialRs485);
nix::ioctl_read_bad!(tiocgrs485, libc::TIOCGRS485, SerialRs485);

/// The kernel's form of `r`, with the delays in whole milliseconds, rounded up.
fn to_kernel(r: &Rs485) -> SerialRs485 {
    let millis =
        |d: std::time::Duration| u32::try_from(d.as_micros().div_ceil(1000)).unwrap_or(u32::MAX);
    let flag = |on: bool, bit: u32| if on { bit } else { 0 };
    SerialRs485 {
        flags: SER_RS485_ENABLED
            | flag(r.rts_while_tx, SER_RS485_RTS_ON_SEND)
            | flag(r.rts_while_rx, SER_RS485_RTS_AFTER_SEND)
            | flag(r.loopback, SER_RS485_RX_DURING_TX),
        delay_rts_before_send: millis(r.delay_before_tx),
        delay_rts_after_send: millis(r.delay_after_tx),
        padding: [0; 5],
    }
}

/// Whether the driver applied the flags and delays of `asked` unchanged; drivers drop
/// flags and delays they do not support, and clamp long delays.
fn applied_as_asked(asked: &SerialRs485, applied: &SerialRs485) -> bool {
    applied.flags & ASKED_FLAGS == asked.flags
        && applied.delay_rts_before_send == asked.delay_rts_before_send
        && applied.delay_rts_after_send == asked.delay_rts_after_send
}

/// Turns on the kernel's RS-485 mode; `false`, with the mode off, when the driver has
/// none or cannot apply `r` as given.
pub fn enable(fd: RawFd, r: &Rs485) -> Result<bool, SerialError> {
    let asked = to_kernel(r);
    let mut applied = to_kernel(r);
    // SAFETY: `fd` is an open port, and the struct outlives the call.
    let honoured = match unsafe { tiocsrs485(fd, &mut applied) } {
        Ok(_) => applied_as_asked(&asked, &applied),
        Err(nix::errno::Errno::ENOTTY | nix::errno::Errno::EINVAL) => false,
        Err(errno) => return Err(std::io::Error::from(errno).into()),
    };
    // A refused request leaves an earlier configuration in place.
    if !honoured {
        disable(fd);
    }
    Ok(honoured)
}

/// Whether the kernel's RS-485 mode is on for `fd`.
pub fn enabled(fd: RawFd) -> bool {
    let mut current = SerialRs485::default();
    // SAFETY: as in `enable`.
    unsafe { tiocgrs485(fd, &mut current) }.is_ok_and(|_| current.flags & SER_RS485_ENABLED != 0)
}

/// Turns the kernel's RS-485 mode off, ignoring a driver that refuses.
pub fn disable(fd: RawFd) {
    // SAFETY: as in `enable`.
    let _ = unsafe { tiocsrs485(fd, &mut SerialRs485::default()) };
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::settings::Rs485;

    #[test]
    fn flags_and_delays_follow_the_fields() {
        let k = to_kernel(&Rs485 {
            rts_while_tx: false,
            rts_while_rx: true,
            delay_before_tx: Duration::from_micros(1500),
            delay_after_tx: Duration::ZERO,
            loopback: true,
        });
        assert_eq!(
            k.flags,
            SER_RS485_ENABLED | SER_RS485_RTS_AFTER_SEND | SER_RS485_RX_DURING_TX
        );
        assert_eq!((k.delay_rts_before_send, k.delay_rts_after_send), (2, 0));
    }

    #[test]
    fn a_driver_that_changes_the_request_did_not_apply_it() {
        let asked = to_kernel(&Rs485 {
            delay_after_tx: Duration::from_millis(200),
            loopback: true,
            ..Rs485::default()
        });
        let applied = |flags: u32, after: u32| SerialRs485 {
            flags,
            delay_rts_after_send: after,
            padding: [7; 5],
            ..SerialRs485::default()
        };
        assert!(applied_as_asked(
            &asked,
            &applied(asked.flags | 1 << 5, 200)
        ));
        assert!(!applied_as_asked(
            &asked,
            &applied(asked.flags & !SER_RS485_RX_DURING_TX, 200)
        ));
        assert!(!applied_as_asked(&asked, &applied(asked.flags, 100)));
    }
}
