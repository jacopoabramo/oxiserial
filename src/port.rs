use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::ReadBuf;
use tokio::sync::Notify;
use tokio::task::coop::unconstrained;
use tokio::time::{Instant, timeout_at};

use crate::backend::{self, Backend};
use crate::errors::SerialError;
use crate::lock;
use crate::settings::{self, Settings};

struct State {
    port: Option<String>,
    settings: Settings,
    rts: bool,
    dtr: bool,
    break_on: bool,
}

/// A serial port shared by the sync and async Python classes.
pub struct PortCore {
    state: Mutex<State>,
    backend: Mutex<Option<Box<dyn Backend>>>,
    read_turn: tokio::sync::Mutex<()>,
    write_turn: tokio::sync::Mutex<()>,
    closed: Notify,
}

/// Clears the break condition when `send_break` ends, including by cancellation.
struct BreakGuard<'a>(&'a PortCore);

impl Drop for BreakGuard<'_> {
    fn drop(&mut self) {
        let _ = self.0.set_break_condition(false);
    }
}

/// On POSIX, control lines of a pty raise EINVAL or ENOTTY; pyserial ignores those on open.
#[cfg(unix)]
fn ignore_unsupported(result: Result<(), SerialError>) -> Result<(), SerialError> {
    match result {
        Err(SerialError::Os { errno: Some(e), .. }) if e == libc::EINVAL || e == libc::ENOTTY => {
            Ok(())
        }
        other => other,
    }
}

#[cfg(not(unix))]
fn ignore_unsupported(result: Result<(), SerialError>) -> Result<(), SerialError> {
    result
}

/// Writes the stored DTR and RTS levels, except for a line the flow control setting drives.
///
/// Backends may reset the lines when the port is opened or reconfigured.
fn restore_lines(
    port: &mut dyn Backend,
    settings: &Settings,
    rts: bool,
    dtr: bool,
) -> Result<(), SerialError> {
    if !settings.dsrdtr {
        ignore_unsupported(port.set_dtr(dtr))?;
    }
    if !settings.rtscts {
        ignore_unsupported(port.set_rts(rts))?;
    }
    Ok(())
}

/// With an inter-byte timeout and data already received, the deadline moves to one gap from now.
fn next_deadline(
    overall: Option<Instant>,
    gap: Option<Duration>,
    have_data: bool,
) -> Option<Instant> {
    match gap {
        Some(gap) if have_data => match Instant::now().checked_add(gap) {
            Some(gap_end) => Some(overall.map_or(gap_end, |d| d.min(gap_end))),
            None => overall,
        },
        _ => overall,
    }
}

/// Polls `poll` once and returns `None` if it is pending.
///
/// Used for an expired deadline: the cooperative budget could otherwise turn a ready
/// poll into a pending one, and a timer would round the wait up to a millisecond.
async fn poll_once<T>(mut poll: impl FnMut(&mut Context<'_>) -> Poll<T>) -> Option<T> {
    unconstrained(poll_fn(|cx| match poll(cx) {
        Poll::Ready(value) => Poll::Ready(Some(value)),
        Poll::Pending => Poll::Ready(None),
    }))
    .await
}

impl PortCore {
    pub fn new(port: Option<String>, settings: Settings) -> Self {
        Self {
            state: Mutex::new(State {
                port,
                settings,
                rts: true,
                dtr: true,
                break_on: false,
            }),
            backend: Mutex::new(None),
            read_turn: tokio::sync::Mutex::new(()),
            write_turn: tokio::sync::Mutex::new(()),
            closed: Notify::new(),
        }
    }

    pub fn port(&self) -> Option<String> {
        lock(&self.state).port.clone()
    }

    pub fn settings(&self) -> Settings {
        lock(&self.state).settings.clone()
    }

    pub fn rts(&self) -> bool {
        lock(&self.state).rts
    }

    pub fn dtr(&self) -> bool {
        lock(&self.state).dtr
    }

    pub fn break_condition(&self) -> bool {
        lock(&self.state).break_on
    }

    pub fn is_open(&self) -> bool {
        lock(&self.backend).is_some()
    }

    pub fn open(&self) -> Result<(), SerialError> {
        let (port, settings, rts, dtr) = {
            let state = lock(&self.state);
            let port = state.port.clone().ok_or(SerialError::NoPort)?;
            (port, state.settings.clone(), state.rts, state.dtr)
        };
        let mut slot = lock(&self.backend);
        if slot.is_some() {
            return Err(SerialError::AlreadyOpen);
        }
        let mut opened = backend::open(&port, &settings)?;
        restore_lines(opened.as_mut(), &settings, rts, dtr)?;
        opened.clear_buffers(true, false)?;
        *slot = Some(opened);
        Ok(())
    }

    pub fn close(&self) {
        let closed = lock(&self.backend).take();
        self.closed.notify_waiters();
        drop(closed);
    }

    /// Changing the port of an open port reopens it, as pyserial does.
    pub fn set_port(&self, port: Option<String>) -> Result<(), SerialError> {
        let was_open = self.is_open();
        self.close();
        lock(&self.state).port = port;
        if was_open { self.open() } else { Ok(()) }
    }

    pub fn set_settings(&self, settings: Settings) -> Result<(), SerialError> {
        lock(&self.state).settings = settings.clone();
        self.if_open(|port| {
            port.configure(&settings)?;
            let (rts, dtr) = {
                let state = lock(&self.state);
                (state.rts, state.dtr)
            };
            restore_lines(port, &settings, rts, dtr)
        })
    }

    pub fn set_rts(&self, level: bool) -> Result<(), SerialError> {
        lock(&self.state).rts = level;
        self.if_open(|port| port.set_rts(level))
    }

    pub fn set_dtr(&self, level: bool) -> Result<(), SerialError> {
        lock(&self.state).dtr = level;
        self.if_open(|port| port.set_dtr(level))
    }

    pub fn set_break_condition(&self, on: bool) -> Result<(), SerialError> {
        lock(&self.state).break_on = on;
        self.if_open(|port| port.set_break_state(on))
    }

    pub fn cts(&self) -> Result<bool, SerialError> {
        self.with_backend(|port| port.cts())
    }

    pub fn dsr(&self) -> Result<bool, SerialError> {
        self.with_backend(|port| port.dsr())
    }

    pub fn ri(&self) -> Result<bool, SerialError> {
        self.with_backend(|port| port.ri())
    }

    pub fn cd(&self) -> Result<bool, SerialError> {
        self.with_backend(|port| port.cd())
    }

    pub fn in_waiting(&self) -> Result<usize, SerialError> {
        self.with_backend(|port| port.in_waiting())
    }

    pub fn out_waiting(&self) -> Result<usize, SerialError> {
        self.with_backend(|port| port.out_waiting())
    }

    pub fn reset_input_buffer(&self) -> Result<(), SerialError> {
        self.with_backend(|port| port.clear_buffers(true, false))
    }

    pub fn reset_output_buffer(&self) -> Result<(), SerialError> {
        self.with_backend(|port| port.clear_buffers(false, true))
    }

    pub fn fileno(&self) -> Result<Option<i32>, SerialError> {
        self.with_backend(|port| Ok(port.fileno()))
    }

    pub async fn read(&self, size: usize) -> Result<Vec<u8>, SerialError> {
        let _turn = self.read_turn.lock().await;
        self.until_closed(self.read_locked(size)).await
    }

    pub async fn read_until(
        &self,
        expected: &[u8],
        size: Option<usize>,
    ) -> Result<Vec<u8>, SerialError> {
        let _turn = self.read_turn.lock().await;
        self.until_closed(self.read_until_locked(expected, size))
            .await
    }

    /// Reads lines until one read times out empty or `hint` bytes have been collected.
    pub async fn readlines(&self, hint: Option<usize>) -> Result<Vec<Vec<u8>>, SerialError> {
        let _turn = self.read_turn.lock().await;
        self.until_closed(async {
            let mut lines = Vec::new();
            let mut total = 0;
            loop {
                let line = self.read_until_locked(b"\n", None).await?;
                if line.is_empty() {
                    break;
                }
                total += line.len();
                lines.push(line);
                if hint.is_some_and(|limit| total >= limit) {
                    break;
                }
            }
            Ok(lines)
        })
        .await
    }

    pub async fn write(&self, data: &[u8]) -> Result<usize, SerialError> {
        let _turn = self.write_turn.lock().await;
        self.until_closed(async {
            let deadline = settings::duration(self.settings().write_timeout)
                .and_then(|t| Instant::now().checked_add(t));
            let mut written = 0;
            while written < data.len() {
                let n = match deadline {
                    None => poll_fn(|cx| self.poll_write_some(cx, &data[written..])).await?,
                    Some(at) if at <= Instant::now() => {
                        match poll_once(|cx| self.poll_write_some(cx, &data[written..])).await {
                            Some(result) => result?,
                            None => return Ok(written),
                        }
                    }
                    Some(at) => {
                        timeout_at(at, poll_fn(|cx| self.poll_write_some(cx, &data[written..])))
                            .await
                            .map_err(|_| SerialError::Timeout("Write timeout".into()))??
                    }
                };
                written += n;
            }
            Ok(data.len())
        })
        .await
    }

    // ponytail: on POSIX the flush is tcdrain, which blocks one runtime worker until output is sent; move it to spawn_blocking if ports with slow baud rates stall other ports
    pub async fn flush(&self) -> Result<(), SerialError> {
        let _turn = self.write_turn.lock().await;
        self.until_closed(poll_fn(|cx| self.poll_flush(cx))).await
    }

    pub async fn send_break(&self, duration: Duration) -> Result<(), SerialError> {
        let _turn = self.write_turn.lock().await;
        self.until_closed(async {
            self.set_break_condition(true)?;
            let _clear = BreakGuard(self);
            tokio::time::sleep(duration).await;
            Ok(())
        })
        .await
    }

    fn with_backend<R>(
        &self,
        f: impl FnOnce(&mut dyn Backend) -> Result<R, SerialError>,
    ) -> Result<R, SerialError> {
        match lock(&self.backend).as_mut() {
            Some(port) => f(port.as_mut()),
            None => Err(SerialError::NotOpen),
        }
    }

    fn if_open(
        &self,
        f: impl FnOnce(&mut dyn Backend) -> Result<(), SerialError>,
    ) -> Result<(), SerialError> {
        match lock(&self.backend).as_mut() {
            Some(port) => f(port.as_mut()),
            None => Ok(()),
        }
    }

    /// Runs `op`, ending it with `NotOpen` if the port is closed before it finishes.
    async fn until_closed<T>(
        &self,
        op: impl Future<Output = Result<T, SerialError>>,
    ) -> Result<T, SerialError> {
        let closed = self.closed.notified();
        tokio::pin!(closed);
        // Registers for notify_waiters before the open check, so a close in between is not missed.
        closed.as_mut().enable();
        if !self.is_open() {
            return Err(SerialError::NotOpen);
        }
        tokio::select! {
            result = op => result,
            () = closed => Err(SerialError::NotOpen),
        }
    }

    async fn read_locked(&self, size: usize) -> Result<Vec<u8>, SerialError> {
        let settings = self.settings();
        let overall =
            settings::duration(settings.timeout).and_then(|t| Instant::now().checked_add(t));
        let gap = settings::duration(settings.inter_byte_timeout);
        let mut out = Vec::with_capacity(size.min(4096));
        let mut chunk = vec![0u8; size.clamp(1, 4096)];
        while out.len() < size {
            let want = (size - out.len()).min(chunk.len());
            let deadline = next_deadline(overall, gap, !out.is_empty());
            match self.read_chunk(&mut chunk[..want], deadline).await? {
                Some(n) => out.extend_from_slice(&chunk[..n]),
                None => break,
            }
        }
        Ok(out)
    }

    // ponytail: one byte per poll so nothing past the terminator is consumed; a read-ahead buffer belongs to the throughput work
    async fn read_until_locked(
        &self,
        expected: &[u8],
        size: Option<usize>,
    ) -> Result<Vec<u8>, SerialError> {
        let settings = self.settings();
        let overall =
            settings::duration(settings.timeout).and_then(|t| Instant::now().checked_add(t));
        let gap = settings::duration(settings.inter_byte_timeout);
        let mut out = Vec::new();
        let mut byte = [0u8; 1];
        while size.is_none_or(|limit| out.len() < limit) {
            let deadline = next_deadline(overall, gap, !out.is_empty());
            match self.read_chunk(&mut byte, deadline).await? {
                Some(_) => out.push(byte[0]),
                None => break,
            }
            if !expected.is_empty() && out.ends_with(expected) {
                break;
            }
        }
        Ok(out)
    }

    /// Reads at least one byte into `buf`; `Ok(None)` means `deadline` passed first.
    async fn read_chunk(
        &self,
        buf: &mut [u8],
        deadline: Option<Instant>,
    ) -> Result<Option<usize>, SerialError> {
        let n = match deadline {
            None => poll_fn(|cx| self.poll_read_some(cx, buf)).await?,
            Some(at) if at <= Instant::now() => {
                match poll_once(|cx| self.poll_read_some(cx, buf)).await {
                    Some(result) => result?,
                    None => return Ok(None),
                }
            }
            Some(at) => match timeout_at(at, poll_fn(|cx| self.poll_read_some(cx, buf))).await {
                Ok(result) => result?,
                Err(_) => return Ok(None),
            },
        };
        if n == 0 {
            return Err(SerialError::disconnected());
        }
        Ok(Some(n))
    }

    fn poll_read_some(
        &self,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize, SerialError>> {
        let mut slot = lock(&self.backend);
        let Some(port) = slot.as_mut() else {
            return Poll::Ready(Err(SerialError::NotOpen));
        };
        let mut read_buf = ReadBuf::new(buf);
        match Pin::new(port.as_mut()).poll_read(cx, &mut read_buf) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(read_buf.filled().len())),
            Poll::Ready(Err(err)) => Poll::Ready(Err(err.into())),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_write_some(
        &self,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<Result<usize, SerialError>> {
        let mut slot = lock(&self.backend);
        let Some(port) = slot.as_mut() else {
            return Poll::Ready(Err(SerialError::NotOpen));
        };
        match Pin::new(port.as_mut()).poll_write(cx, data) {
            Poll::Ready(Ok(0)) => Poll::Ready(Err(SerialError::Os {
                errno: None,
                message: "write failed: the port accepted no data".into(),
            })),
            Poll::Ready(result) => Poll::Ready(result.map_err(SerialError::from)),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(&self, cx: &mut Context<'_>) -> Poll<Result<(), SerialError>> {
        let mut slot = lock(&self.backend);
        let Some(port) = slot.as_mut() else {
            return Poll::Ready(Err(SerialError::NotOpen));
        };
        Pin::new(port.as_mut())
            .poll_flush(cx)
            .map_err(SerialError::from)
    }
}

#[cfg(all(test, feature = "test-backend"))]
mod tests {
    use std::time::Duration;

    use tokio::time::Instant;

    use super::*;
    use crate::backend::mock;

    fn open_pair(configure: impl Fn(&mut Settings)) -> (PortCore, PortCore, String) {
        let (a_name, b_name) = mock::pair();
        let mut settings = Settings::default();
        configure(&mut settings);
        let a = PortCore::new(Some(a_name.clone()), settings.clone());
        let b = PortCore::new(Some(b_name), settings);
        a.open().unwrap();
        b.open().unwrap();
        (a, b, a_name)
    }

    #[test]
    fn reconfiguring_an_open_port_keeps_the_line_levels() {
        let (a, _b, a_name) = open_pair(|_| {});
        a.set_rts(false).unwrap();
        a.set_dtr(false).unwrap();
        a.set_settings(Settings {
            baudrate: 19_200,
            ..a.settings()
        })
        .unwrap();
        assert_eq!(
            mock::update(&a_name, |end| (end.rts, end.dtr)),
            Some((false, false))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn read_without_timeout_waits_for_every_byte() {
        let (a, b, _) = open_pair(|_| {});
        let (read, ()) = tokio::join!(b.read(4), async {
            a.write(b"ab").await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
            a.write(b"cd").await.unwrap();
        });
        assert_eq!(read.unwrap(), b"abcd");
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_returns_partial_data_after_the_deadline() {
        let (a, b, _) = open_pair(|s| s.timeout = Some(1.0));
        a.write(b"ab").await.unwrap();
        let start = Instant::now();
        assert_eq!(b.read(4).await.unwrap(), b"ab");
        let waited = start.elapsed();
        assert!(waited >= Duration::from_secs(1) && waited < Duration::from_millis(1100));
    }

    #[tokio::test(start_paused = true)]
    async fn zero_timeout_returns_buffered_bytes_without_waiting() {
        let (a, b, _) = open_pair(|s| s.timeout = Some(0.0));
        assert_eq!(b.read(4).await.unwrap(), b"");
        a.write(b"ab").await.unwrap();
        let start = Instant::now();
        assert_eq!(b.read(4).await.unwrap(), b"ab");
        assert_eq!(start.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn inter_byte_timeout_ends_the_read_after_a_gap() {
        let (a, b, _) = open_pair(|s| s.inter_byte_timeout = Some(0.1));
        let (read, ()) = tokio::join!(b.read(10), async {
            a.write(b"a").await.unwrap();
            tokio::time::sleep(Duration::from_millis(80)).await;
            a.write(b"b").await.unwrap();
            tokio::time::sleep(Duration::from_millis(80)).await;
            a.write(b"c").await.unwrap();
            tokio::time::sleep(Duration::from_millis(240)).await;
            a.write(b"d").await.unwrap();
        });
        assert_eq!(read.unwrap(), b"abc");
    }

    #[tokio::test(start_paused = true)]
    async fn read_until_leaves_later_bytes_unread() {
        let (a, b, _) = open_pair(|s| s.timeout = Some(0.0));
        a.write(b"x\ny\nabcdef").await.unwrap();
        assert_eq!(b.read_until(b"\n", None).await.unwrap(), b"x\n");
        assert_eq!(b.read_until(b"\n", None).await.unwrap(), b"y\n");
        assert_eq!(b.read_until(b"\n", Some(3)).await.unwrap(), b"abc");
        assert_eq!(b.read(10).await.unwrap(), b"def");
    }

    #[tokio::test(start_paused = true)]
    async fn write_timeout_raises_when_the_port_does_not_drain() {
        let (a, _b, a_name) = open_pair(|s| s.write_timeout = Some(0.5));
        mock::update(&a_name, |end| end.write_blocked = true);
        assert!(matches!(a.write(b"x").await, Err(SerialError::Timeout(_))));
    }

    #[tokio::test(start_paused = true)]
    async fn zero_write_timeout_returns_bytes_written() {
        let (a, _b, a_name) = open_pair(|s| s.write_timeout = Some(0.0));
        mock::update(&a_name, |end| end.write_blocked = true);
        assert_eq!(a.write(b"xy").await, Ok(0));
    }

    #[tokio::test(start_paused = true)]
    async fn a_write_proceeds_while_a_read_waits() {
        let (a, b, _) = open_pair(|_| {});
        let (read, ()) = tokio::join!(b.read(2), async {
            b.write(b"zz").await.unwrap();
            assert_eq!(a.read(2).await.unwrap(), b"zz");
            a.write(b"ok").await.unwrap();
        });
        assert_eq!(read.unwrap(), b"ok");
    }

    #[tokio::test(start_paused = true)]
    async fn reads_complete_in_call_order() {
        let (a, b, _) = open_pair(|_| {});
        let (first, second, ()) = tokio::join!(b.read(2), b.read(2), async {
            a.write(b"1122").await.unwrap();
        });
        assert_eq!(first.unwrap(), b"11");
        assert_eq!(second.unwrap(), b"22");
    }

    #[tokio::test(start_paused = true)]
    async fn close_wakes_a_pending_read() {
        let (_a, b, _) = open_pair(|_| {});
        let (read, ()) = tokio::join!(b.read(1), async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            b.close();
        });
        assert_eq!(read, Err(SerialError::NotOpen));
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_send_break_clears_the_break() {
        let (a, _b, a_name) = open_pair(|_| {});
        let result = tokio::time::timeout(
            Duration::from_millis(10),
            a.send_break(Duration::from_secs(10)),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(mock::update(&a_name, |end| end.break_on), Some(false));
    }

    #[tokio::test]
    async fn closed_and_unconfigured_ports_fail() {
        let core = PortCore::new(None, Settings::default());
        assert_eq!(core.read(1).await, Err(SerialError::NotOpen));
        assert_eq!(core.open(), Err(SerialError::NoPort));
        let (a, _b, _) = open_pair(|_| {});
        assert_eq!(a.open(), Err(SerialError::AlreadyOpen));
        a.close();
        a.open().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn huge_timeout_waits_like_none() {
        let (a, b, _) = open_pair(|s| s.timeout = Some(1e300));
        let (read, ()) = tokio::join!(b.read(2), async {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            a.write(b"ab").await.unwrap();
        });
        assert_eq!(read.unwrap(), b"ab");
    }
}
