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

nix::ioctl_write_ptr_bad!(tiocsrs485, libc::TIOCSRS485, SerialRs485);
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

/// Turns on the kernel's RS-485 mode; `false` when the driver has none.
pub fn enable(fd: RawFd, r: &Rs485) -> Result<bool, SerialError> {
    // SAFETY: `fd` is an open port, and the struct outlives the call.
    match unsafe { tiocsrs485(fd, &to_kernel(r)) } {
        Ok(_) => Ok(true),
        Err(nix::errno::Errno::ENOTTY | nix::errno::Errno::EINVAL) => Ok(false),
        Err(errno) => Err(std::io::Error::from(errno).into()),
    }
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
    let _ = unsafe { tiocsrs485(fd, &SerialRs485::default()) };
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
}
