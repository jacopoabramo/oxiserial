use std::collections::VecDeque;
use std::io;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll, Waker};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::backend::{Backend, Drain};
use crate::errors::SerialError;
use crate::lock;
use crate::settings::Settings;

// Matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt): LoopbackSerial.buffer_size.
const CAPACITY: usize = 4096;

/// Whether `port` names a loopback port: `loop://`, in any case, with anything after it.
pub fn is_loopback(port: &str) -> bool {
    port.get(..7)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("loop://"))
}

#[derive(Default)]
struct State {
    queue: VecDeque<u8>,
    rts: bool,
    dtr: bool,
    read_waker: Option<Waker>,
    write_waker: Option<Waker>,
}

impl State {
    fn wake_writer(&mut self) {
        if let Some(waker) = self.write_waker.take() {
            waker.wake();
        }
    }
}

/// A port whose reads return what was written to it, as pyserial's `loop://`.
#[derive(Default)]
pub struct LoopbackPort {
    state: Mutex<State>,
}

impl AsyncRead for LoopbackPort {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut state = lock(&self.state);
        if state.queue.is_empty() {
            state.read_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let n = buf.remaining().min(state.queue.len());
        let bytes: Vec<u8> = state.queue.drain(..n).collect();
        buf.put_slice(&bytes);
        state.wake_writer();
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for LoopbackPort {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut state = lock(&self.state);
        let n = data.len().min(CAPACITY - state.queue.len());
        if n == 0 && !data.is_empty() {
            state.write_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        state.queue.extend(&data[..n]);
        if let Some(waker) = state.read_waker.take() {
            waker.wake();
        }
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

// Matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt): LoopbackSerial line and buffer behaviour.
impl Backend for LoopbackPort {
    fn configure(&mut self, _settings: &Settings, rts: bool, dtr: bool) -> Result<(), SerialError> {
        let mut state = lock(&self.state);
        state.rts = rts;
        state.dtr = dtr;
        Ok(())
    }

    fn set_rts(&mut self, level: bool) -> Result<(), SerialError> {
        lock(&self.state).rts = level;
        Ok(())
    }

    fn set_dtr(&mut self, level: bool) -> Result<(), SerialError> {
        lock(&self.state).dtr = level;
        Ok(())
    }

    fn cts(&mut self) -> Result<bool, SerialError> {
        Ok(lock(&self.state).rts)
    }

    fn dsr(&mut self) -> Result<bool, SerialError> {
        Ok(lock(&self.state).dtr)
    }

    fn ri(&mut self) -> Result<bool, SerialError> {
        Ok(false)
    }

    fn cd(&mut self) -> Result<bool, SerialError> {
        Ok(true)
    }

    fn in_waiting(&self) -> Result<usize, SerialError> {
        Ok(lock(&self.state).queue.len())
    }

    fn out_waiting(&self) -> Result<usize, SerialError> {
        Ok(lock(&self.state).queue.len())
    }

    /// Input and output are the same queue, so clearing either empties it.
    fn clear_buffers(&self, input: bool, output: bool) -> Result<(), SerialError> {
        if input || output {
            let mut state = lock(&self.state);
            state.queue.clear();
            state.wake_writer();
        }
        Ok(())
    }

    /// Does nothing: there is no line to hold, and `break_condition` is kept by the caller.
    fn set_break_state(&self, _on: bool) -> Result<(), SerialError> {
        Ok(())
    }

    fn fileno(&self) -> Option<i32> {
        None
    }

    fn drain_handle(&self) -> Result<Drain, SerialError> {
        Ok(Drain::Done)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::time::Instant;

    use crate::errors::SerialError;
    use crate::port::PortCore;
    use crate::settings::Settings;

    fn open(configure: impl Fn(&mut Settings)) -> Result<PortCore, SerialError> {
        let mut settings = Settings::default();
        configure(&mut settings);
        let port = PortCore::new(Some("LOOP://?logging=debug".into()), settings);
        port.open()?;
        Ok(port)
    }

    #[tokio::test]
    async fn written_bytes_come_back_in_order() -> Result<(), SerialError> {
        let port = open(|s| s.timeout = Some(0.0.into()))?;
        port.write(b"abc").await?;
        port.write(b"de").await?;
        assert_eq!(port.in_waiting()?, 5);
        assert_eq!(port.read(10).await?, b"abcde");
        assert_eq!(port.read(1).await?, b"");
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn a_full_queue_blocks_a_write_until_a_read_frees_space() -> Result<(), SerialError> {
        let port = open(|s| s.write_timeout = Some(0.5.into()))?;
        assert!(matches!(
            port.write(&[0; super::CAPACITY + 1]).await,
            Err(SerialError::Timeout(_))
        ));
        assert_eq!(port.in_waiting()?, super::CAPACITY);
        let start = Instant::now();
        let (written, read) = tokio::join!(port.write(b"x"), port.read(1));
        assert_eq!(written?, 1);
        assert_eq!(read?, [0]);
        // Paused time only moves when every task waits, so a write left waiting would take the full timeout.
        assert_eq!(start.elapsed(), Duration::ZERO);
        Ok(())
    }

    #[tokio::test]
    async fn clearing_either_buffer_empties_the_queue() -> Result<(), SerialError> {
        let port = open(|_| {})?;
        port.write(b"abc").await?;
        port.reset_output_buffer()?;
        assert_eq!(port.in_waiting()?, 0);
        port.write(b"abc").await?;
        port.reset_input_buffer()?;
        assert_eq!(port.out_waiting()?, 0);
        Ok(())
    }

    #[test]
    fn lines_read_back_as_pyserial_loopback() -> Result<(), SerialError> {
        let port = open(|_| {})?;
        assert!(port.cts()? && port.dsr()?);
        port.set_rts(false)?;
        assert!(!port.cts()? && port.dsr()?);
        port.set_dtr(false)?;
        assert!(!port.dsr()?);
        assert!(!port.ri()? && port.cd()?);
        Ok(())
    }
}
