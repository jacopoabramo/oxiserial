use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::ReadBuf;
use tokio::sync::Notify;
use tokio::task::coop::unconstrained;
use tokio::time::{Instant, timeout_at};

use crate::backend::{self, Backend, Drain};
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

/// Clears the break condition when `send_break` is cancelled before it clears it itself.
struct BreakGuard<'a>(Option<&'a PortCore>);

impl BreakGuard<'_> {
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for BreakGuard<'_> {
    fn drop(&mut self) {
        if let Some(core) = self.0 {
            // Runs only on cancellation, where no caller is left to receive an error.
            let _ = core.set_break_condition(false);
        }
    }
}

/// Cancels a write the backend still has in progress when `PortCore::write` ends early.
struct CancelWrite<'a>(&'a PortCore);

impl Drop for CancelWrite<'_> {
    fn drop(&mut self) {
        // The write already ended with its own result or was cancelled; no caller is left for this one.
        let _ = self.0.cancel_write();
    }
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
        // Taken first, as in `update_settings`, so a concurrent settings change is not missed.
        let mut slot = lock(&self.backend);
        let (port, settings, rts, dtr) = {
            let state = lock(&self.state);
            let port = state.port.clone().ok_or(SerialError::NoPort)?;
            (port, state.settings.clone(), state.rts, state.dtr)
        };
        if slot.is_some() {
            return Err(SerialError::AlreadyOpen);
        }
        // Matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt): open order.
        let opened = backend::open(&port, &settings, rts, dtr)?;
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
        self.update_settings(|current| *current = settings)
    }

    /// Changes the settings and applies them to an open port, all under the backend lock,
    /// so concurrent changes are not lost and the port always has the stored settings.
    ///
    /// If the port rejects the new settings, the previous ones are stored again.
    pub fn update_settings(&self, change: impl FnOnce(&mut Settings)) -> Result<(), SerialError> {
        let mut slot = lock(&self.backend);
        let (before, settings, rts, dtr) = {
            let mut state = lock(&self.state);
            let before = state.settings.clone();
            change(&mut state.settings);
            (before, state.settings.clone(), state.rts, state.dtr)
        };
        match slot.as_mut() {
            Some(port) if before.port_config_differs(&settings) => {
                let result = port.configure(&settings, rts, dtr);
                if result.is_err() {
                    lock(&self.state).settings = before;
                }
                result
            }
            _ => Ok(()),
        }
    }

    pub fn set_rts(&self, level: bool) -> Result<(), SerialError> {
        self.set_line(
            |state| {
                state.rts = level;
                state.settings.rtscts
            },
            |port| port.set_rts(level),
        )
    }

    pub fn set_dtr(&self, level: bool) -> Result<(), SerialError> {
        self.set_line(
            |state| {
                state.dtr = level;
                // POSIX has no DSR/DTR flow control, so there DTR stays the caller's to set.
                state.settings.dsrdtr && cfg!(windows)
            },
            |port| port.set_dtr(level),
        )
    }

    /// Stores a line level with `store`, which returns whether flow control drives the line,
    /// and writes it with `write` only when it does not.
    ///
    /// A line under flow control belongs to the driver, which may reject writes to it; the
    /// stored level is applied by the configure that turns flow control off.
    fn set_line(
        &self,
        store: impl FnOnce(&mut State) -> bool,
        write: impl FnOnce(&mut dyn Backend) -> Result<(), SerialError>,
    ) -> Result<(), SerialError> {
        // Taken first, as in `update_settings`, so flow control cannot change before the write.
        let mut slot = lock(&self.backend);
        let driven = store(&mut lock(&self.state));
        match slot.as_mut() {
            Some(port) if !driven => write(port.as_mut()),
            _ => Ok(()),
        }
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

    fn cancel_write(&self) -> Result<usize, SerialError> {
        match lock(&self.backend).as_mut() {
            Some(port) => port.cancel_write(),
            None => Ok(0),
        }
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
        self.until_closed(self.read_until_locked(expected, size, false))
            .await
    }

    /// Reads up to a newline, giving each byte the full timeout, as `io.IOBase.readline` does.
    pub async fn readline(&self, size: Option<usize>) -> Result<Vec<u8>, SerialError> {
        let _turn = self.read_turn.lock().await;
        self.until_closed(self.read_until_locked(b"\n", size, true))
            .await
    }

    /// Reads lines until one read times out empty or `hint` bytes have been collected.
    pub async fn readlines(&self, hint: Option<usize>) -> Result<Vec<Vec<u8>>, SerialError> {
        let _turn = self.read_turn.lock().await;
        self.until_closed(async {
            let mut lines = Vec::new();
            let mut total = 0;
            loop {
                let line = self.read_until_locked(b"\n", None, true).await?;
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
        // Declared after `_turn`, so it cancels a leftover write before the next writer starts.
        let _cancel = CancelWrite(self);
        self.until_closed(async {
            let write_timeout = self.settings().write_timeout;
            let deadline =
                settings::duration(write_timeout).and_then(|t| Instant::now().checked_add(t));
            let mut written = 0;
            while written < data.len() {
                let n = match deadline {
                    _ if write_timeout == Some(0.0) => {
                        match poll_once(|cx| self.poll_write_some(cx, &data[written..])).await {
                            Some(result) => result?,
                            // Matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt): a
                            // write still pending counts as written and completes in the driver.
                            None => {
                                return Ok(written + self.with_backend(|p| Ok(p.detach_write()))?);
                            }
                        }
                    }
                    None => poll_fn(|cx| self.poll_write_some(cx, &data[written..])).await?,
                    Some(at) => {
                        timeout_at(at, poll_fn(|cx| self.poll_write_some(cx, &data[written..])))
                            .await
                            // Matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt): message.
                            .map_err(|_| SerialError::Timeout("Write timeout".into()))??
                    }
                };
                // An aborted write, for example by reset_output_buffer, ends the call with what was sent, as in pyserial.
                if n == 0 {
                    return Ok(written);
                }
                written += n;
            }
            Ok(data.len())
        })
        .await
    }

    pub async fn flush(&self) -> Result<(), SerialError> {
        let _turn = self.write_turn.lock().await;
        self.until_closed(async {
            match self.with_backend(|port| port.drain_handle())? {
                #[cfg(feature = "test-backend")]
                Drain::Done => Ok(()),
                #[cfg(unix)]
                Drain::Fd(fd) => tokio::task::spawn_blocking(move || {
                    nix::sys::termios::tcdrain(&fd)
                        .map_err(|errno| SerialError::from(std::io::Error::from(errno)))
                })
                .await
                .map_err(|err| SerialError::Os {
                    code: None,
                    message: format!("flush failed: {err}"),
                })?,
                #[cfg(not(unix))]
                Drain::PollOutWaiting => {
                    poll_fn(|cx| self.poll_flush_some(cx)).await?;
                    while self.out_waiting()? > 0 {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    Ok(())
                }
            }
        })
        .await
    }

    pub async fn send_break(&self, duration: Duration) -> Result<(), SerialError> {
        let _turn = self.write_turn.lock().await;
        self.until_closed(async {
            self.set_break_condition(true)?;
            let mut clear = BreakGuard(Some(self));
            tokio::time::sleep(duration).await;
            clear.disarm();
            self.set_break_condition(false)
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
    ///
    /// When `op` fails because the device is gone, the port is closed, so later calls raise
    /// `NotOpen` rather than failing on a handle that cannot recover.
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
            result = op => {
                if result.as_ref().is_err_and(SerialError::device_gone) {
                    self.close();
                }
                result
            }
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
    /// With `per_byte`, the timeout restarts for every byte, as for a series of `read(1)` calls.
    async fn read_until_locked(
        &self,
        expected: &[u8],
        size: Option<usize>,
        per_byte: bool,
    ) -> Result<Vec<u8>, SerialError> {
        let settings = self.settings();
        let timeout = settings::duration(settings.timeout);
        let from_now = || timeout.and_then(|t| Instant::now().checked_add(t));
        let overall = from_now();
        let gap = settings::duration(settings.inter_byte_timeout);
        let mut out = Vec::new();
        let mut byte = [0u8; 1];
        while size.is_none_or(|limit| out.len() < limit) {
            let deadline = if per_byte {
                from_now()
            } else {
                next_deadline(overall, gap, !out.is_empty())
            };
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
        Pin::new(port.as_mut())
            .poll_write(cx, data)
            .map_err(SerialError::from)
    }

    #[cfg(not(unix))]
    fn poll_flush_some(&self, cx: &mut Context<'_>) -> Poll<Result<(), SerialError>> {
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

    fn open_pair(
        configure: impl Fn(&mut Settings),
    ) -> Result<(PortCore, PortCore, String), SerialError> {
        let (a_name, b_name) = mock::pair();
        let mut settings = Settings::default();
        configure(&mut settings);
        let a = PortCore::new(Some(a_name.clone()), settings.clone());
        let b = PortCore::new(Some(b_name), settings);
        a.open()?;
        b.open()?;
        Ok((a, b, a_name))
    }

    #[test]
    fn reconfiguring_an_open_port_keeps_the_line_levels() -> Result<(), SerialError> {
        let (a, _b, a_name) = open_pair(|_| {})?;
        a.set_rts(false)?;
        a.set_dtr(false)?;
        mock::update(&a_name, |end| end.high_writes = 0);
        a.set_settings(Settings {
            baudrate: 19_200,
            ..a.settings()
        })?;
        assert_eq!(
            mock::update(&a_name, |end| (end.rts, end.dtr, end.high_writes)),
            Some((false, false, 0))
        );
        Ok(())
    }

    #[test]
    fn lines_under_flow_control_are_stored_and_applied_when_it_is_turned_off()
    -> Result<(), SerialError> {
        let (a, _b, a_name) = open_pair(|s| {
            s.rtscts = true;
            s.dsrdtr = true;
        })?;
        let lines = || mock::update(&a_name, |end| (end.rts, end.dtr));
        a.set_rts(false)?;
        a.set_dtr(false)?;
        assert!(!a.rts() && !a.dtr());
        // DTR is held back only on Windows; POSIX has no DSR/DTR flow control.
        assert_eq!(lines(), Some((true, cfg!(windows))));
        a.update_settings(|s| {
            s.rtscts = false;
            s.dsrdtr = false;
        })?;
        assert_eq!(lines(), Some((false, false)));
        Ok(())
    }

    #[test]
    fn opening_writes_each_line_once_at_the_stored_level() -> Result<(), SerialError> {
        let (a_name, _) = mock::pair();
        let a = PortCore::new(Some(a_name.clone()), Settings::default());
        a.set_rts(false)?;
        a.set_dtr(false)?;
        a.open()?;
        assert_eq!(
            mock::update(&a_name, |end| (end.line_writes, end.high_writes)),
            Some((2, 0))
        );
        Ok(())
    }

    #[test]
    fn only_port_settings_reconfigure_an_open_port() -> Result<(), SerialError> {
        let (a, _b, a_name) = open_pair(|_| {})?;
        let calls = || mock::update(&a_name, |end| end.configure_calls);
        assert_eq!(calls(), Some(1));
        a.update_settings(|s| {
            s.timeout = Some(1.0);
            s.write_timeout = Some(1.0);
            s.inter_byte_timeout = Some(0.1);
        })?;
        assert_eq!(calls(), Some(1));
        a.update_settings(|s| s.baudrate = 19_200)?;
        assert_eq!(calls(), Some(2));
        Ok(())
    }

    #[test]
    fn rejected_settings_leave_the_previous_ones_stored() -> Result<(), SerialError> {
        let (a, _b, _) = open_pair(|_| {})?;
        for _ in 0..2 {
            assert!(
                a.update_settings(|s| s.baudrate = mock::REJECTED_BAUDRATE)
                    .is_err()
            );
            assert_eq!(a.settings().baudrate, 9600);
        }
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn read_without_timeout_waits_for_every_byte() -> Result<(), SerialError> {
        let (a, b, _) = open_pair(|_| {})?;
        let (read, written) = tokio::join!(b.read(4), async {
            a.write(b"ab").await?;
            tokio::time::sleep(Duration::from_secs(5)).await;
            a.write(b"cd").await?;
            Ok::<(), SerialError>(())
        });
        written?;
        assert_eq!(read?, b"abcd");
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_returns_partial_data_after_the_deadline() -> Result<(), SerialError> {
        let (a, b, _) = open_pair(|s| s.timeout = Some(1.0))?;
        a.write(b"ab").await?;
        let start = Instant::now();
        assert_eq!(b.read(4).await?, b"ab");
        let waited = start.elapsed();
        assert!(waited >= Duration::from_secs(1) && waited < Duration::from_millis(1100));
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn zero_timeout_returns_buffered_bytes_without_waiting() -> Result<(), SerialError> {
        let (a, b, _) = open_pair(|s| s.timeout = Some(0.0))?;
        assert_eq!(b.read(4).await?, b"");
        a.write(b"ab").await?;
        let start = Instant::now();
        assert_eq!(b.read(4).await?, b"ab");
        assert_eq!(start.elapsed(), Duration::ZERO);
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn inter_byte_timeout_ends_the_read_after_a_gap() -> Result<(), SerialError> {
        let (a, b, _) = open_pair(|s| s.inter_byte_timeout = Some(0.1))?;
        let (read, written) = tokio::join!(b.read(10), async {
            a.write(b"a").await?;
            tokio::time::sleep(Duration::from_millis(80)).await;
            a.write(b"b").await?;
            tokio::time::sleep(Duration::from_millis(80)).await;
            a.write(b"c").await?;
            tokio::time::sleep(Duration::from_millis(240)).await;
            a.write(b"d").await?;
            Ok::<(), SerialError>(())
        });
        written?;
        assert_eq!(read?, b"abc");
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn read_until_leaves_later_bytes_unread() -> Result<(), SerialError> {
        let (a, b, _) = open_pair(|s| s.timeout = Some(0.0))?;
        a.write(b"x\ny\nabcdef").await?;
        assert_eq!(b.read_until(b"\n", None).await?, b"x\n");
        assert_eq!(b.read_until(b"\n", None).await?, b"y\n");
        assert_eq!(b.read_until(b"\n", Some(3)).await?, b"abc");
        assert_eq!(b.read(10).await?, b"def");
        Ok(())
    }

    /// Sends `bytes` to `port` one at a time, 150 ms apart.
    async fn trickle(port: &PortCore, bytes: &[u8]) -> Result<(), SerialError> {
        for byte in bytes {
            tokio::time::sleep(Duration::from_millis(150)).await;
            port.write(&[*byte]).await?;
        }
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn readline_gives_each_byte_the_full_timeout() -> Result<(), SerialError> {
        let (a, b, _) = open_pair(|s| s.timeout = Some(0.3))?;
        let (line, written) = tokio::join!(b.readline(None), trickle(&a, b"abcd\n"));
        written?;
        assert_eq!(line?, b"abcd\n");
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn read_until_keeps_one_overall_deadline() -> Result<(), SerialError> {
        let (a, b, _) = open_pair(|s| s.timeout = Some(0.4))?;
        let (line, written) = tokio::join!(b.read_until(b"\n", None), trickle(&a, b"abcd\n"));
        written?;
        assert_eq!(line?, b"ab");
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn write_timeout_raises_when_the_port_does_not_drain() -> Result<(), SerialError> {
        let (a, _b, a_name) = open_pair(|s| s.write_timeout = Some(0.5))?;
        mock::update(&a_name, |end| end.write_blocked = true);
        assert!(matches!(a.write(b"x").await, Err(SerialError::Timeout(_))));
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn zero_write_timeout_returns_bytes_written() -> Result<(), SerialError> {
        let (a, _b, a_name) = open_pair(|s| s.write_timeout = Some(0.0))?;
        mock::update(&a_name, |end| end.write_blocked = true);
        assert_eq!(a.write(b"xy").await, Ok(0));
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn a_write_proceeds_while_a_read_waits() -> Result<(), SerialError> {
        let (a, b, _) = open_pair(|_| {})?;
        let (read, echoed) = tokio::join!(b.read(2), async {
            b.write(b"zz").await?;
            assert_eq!(a.read(2).await?, b"zz");
            a.write(b"ok").await?;
            Ok::<(), SerialError>(())
        });
        echoed?;
        assert_eq!(read?, b"ok");
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn reads_complete_in_call_order() -> Result<(), SerialError> {
        let (a, b, _) = open_pair(|_| {})?;
        let (first, second, written) = tokio::join!(b.read(2), b.read(2), a.write(b"1122"));
        written?;
        assert_eq!(first?, b"11");
        assert_eq!(second?, b"22");
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn close_wakes_a_pending_read() -> Result<(), SerialError> {
        let (_a, b, _) = open_pair(|_| {})?;
        let (read, ()) = tokio::join!(b.read(1), async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            b.close();
        });
        assert_eq!(read, Err(SerialError::NotOpen));
        Ok(())
    }

    #[tokio::test]
    async fn a_removed_device_closes_the_port() -> Result<(), SerialError> {
        let (a_name, b_name) = mock::pair();
        let a = PortCore::new(Some(a_name.clone()), Settings::default());
        let b = PortCore::new(Some(b_name.clone()), Settings::default());
        a.open()?;
        b.open()?;
        mock::update(&a_name, |end| end.gone = true);
        mock::update(&b_name, |end| end.gone = true);

        assert!(matches!(a.read(1).await, Err(SerialError::Os { .. })));
        assert!(!a.is_open());
        assert_eq!(a.read(1).await, Err(SerialError::NotOpen));

        assert!(matches!(b.write(b"x").await, Err(SerialError::Os { .. })));
        assert!(!b.is_open());
        assert_eq!(b.write(b"x").await, Err(SerialError::NotOpen));
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn send_break_clears_the_break_when_done() -> Result<(), SerialError> {
        let (a, _b, a_name) = open_pair(|_| {})?;
        a.send_break(Duration::from_millis(10)).await?;
        assert_eq!(mock::update(&a_name, |end| end.break_on), Some(false));
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_send_break_clears_the_break() -> Result<(), SerialError> {
        let (a, _b, a_name) = open_pair(|_| {})?;
        let result = tokio::time::timeout(
            Duration::from_millis(10),
            a.send_break(Duration::from_secs(10)),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(mock::update(&a_name, |end| end.break_on), Some(false));
        Ok(())
    }

    #[tokio::test]
    async fn closed_and_unconfigured_ports_fail() -> Result<(), SerialError> {
        let core = PortCore::new(None, Settings::default());
        assert_eq!(core.read(1).await, Err(SerialError::NotOpen));
        assert_eq!(core.open(), Err(SerialError::NoPort));
        let (a, _b, _) = open_pair(|_| {})?;
        assert_eq!(a.open(), Err(SerialError::AlreadyOpen));
        a.close();
        a.open()?;
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn huge_timeout_waits_like_none() -> Result<(), SerialError> {
        let (a, b, _) = open_pair(|s| s.timeout = Some(1e300))?;
        let (read, written) = tokio::join!(b.read(2), async {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            a.write(b"ab").await
        });
        written?;
        assert_eq!(read?, b"ab");
        Ok(())
    }

    #[tokio::test]
    async fn flush_completes_after_a_write_and_fails_on_a_closed_port() -> Result<(), SerialError> {
        let (a, _b, _) = open_pair(|_| {})?;
        a.write(b"abc").await?;
        a.flush().await?;
        a.close();
        assert_eq!(a.flush().await, Err(SerialError::NotOpen));
        Ok(())
    }
}
