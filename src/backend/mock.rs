use std::collections::{HashMap, VecDeque};
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll, Waker};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::backend::{Backend, Drain};
use crate::errors::SerialError;
use crate::lock;
use crate::settings::Settings;

/// Control state of one end of a mock pair.
pub struct EndState {
    pub rts: bool,
    pub dtr: bool,
    pub break_on: bool,
    pub write_blocked: bool,
    pub baudrate: u32,
    pub configure_calls: usize,
    write_waker: Option<Waker>,
}

impl Default for EndState {
    fn default() -> Self {
        Self {
            rts: true,
            dtr: true,
            break_on: false,
            write_blocked: false,
            baudrate: 0,
            configure_calls: 0,
            write_waker: None,
        }
    }
}

#[derive(Default)]
struct PairState {
    inbox: [VecDeque<u8>; 2],
    read_waker: [Option<Waker>; 2],
    ends: [EndState; 2],
}

type Registry = Mutex<HashMap<String, (Arc<Mutex<PairState>>, usize)>>;

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(Registry::default)
}

/// One end of an in-memory null-modem pair.
pub struct MockPort {
    pair: Arc<Mutex<PairState>>,
    side: usize,
}

/// Creates a connected pair and returns the two port names.
pub fn pair() -> (String, String) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let state = Arc::new(Mutex::new(PairState::default()));
    let names = (format!("mock://{id}/a"), format!("mock://{id}/b"));
    let mut ports = lock(registry());
    ports.insert(names.0.clone(), (Arc::clone(&state), 0));
    ports.insert(names.1.clone(), (state, 1));
    names
}

pub fn lookup(port: &str) -> Option<MockPort> {
    lock(registry()).get(port).map(|(pair, side)| MockPort {
        pair: Arc::clone(pair),
        side: *side,
    })
}

/// Runs `f` on the control state of `port`, waking a writer that is no longer blocked.
pub fn update<R>(port: &str, f: impl FnOnce(&mut EndState) -> R) -> Option<R> {
    let mock = lookup(port)?;
    let mut state = lock(&mock.pair);
    let end = &mut state.ends[mock.side];
    let result = f(end);
    if !end.write_blocked
        && let Some(waker) = end.write_waker.take()
    {
        waker.wake();
    }
    Some(result)
}

impl MockPort {
    fn peer(&self) -> usize {
        1 - self.side
    }
}

impl AsyncRead for MockPort {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut guard = lock(&self.pair);
        let state = &mut *guard;
        let inbox = &mut state.inbox[self.side];
        if inbox.is_empty() {
            state.read_waker[self.side] = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let n = buf.remaining().min(inbox.len());
        let bytes: Vec<u8> = inbox.drain(..n).collect();
        buf.put_slice(&bytes);
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for MockPort {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let peer = self.peer();
        let mut guard = lock(&self.pair);
        let state = &mut *guard;
        let end = &mut state.ends[self.side];
        if end.write_blocked {
            end.write_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        state.inbox[peer].extend(data);
        if let Some(waker) = state.read_waker[peer].take() {
            waker.wake();
        }
        Poll::Ready(Ok(data.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl Backend for MockPort {
    fn configure(&mut self, settings: &Settings) -> Result<(), SerialError> {
        let mut state = lock(&self.pair);
        let end = &mut state.ends[self.side];
        end.baudrate = settings.baudrate;
        end.configure_calls += 1;
        // Reconfiguring resets the lines on some real backends; callers must restore them.
        end.rts = true;
        end.dtr = true;
        Ok(())
    }

    fn set_rts(&mut self, level: bool) -> Result<(), SerialError> {
        lock(&self.pair).ends[self.side].rts = level;
        Ok(())
    }

    fn set_dtr(&mut self, level: bool) -> Result<(), SerialError> {
        lock(&self.pair).ends[self.side].dtr = level;
        Ok(())
    }

    fn cts(&mut self) -> Result<bool, SerialError> {
        Ok(lock(&self.pair).ends[self.peer()].rts)
    }

    fn dsr(&mut self) -> Result<bool, SerialError> {
        Ok(lock(&self.pair).ends[self.peer()].dtr)
    }

    fn ri(&mut self) -> Result<bool, SerialError> {
        Ok(false)
    }

    fn cd(&mut self) -> Result<bool, SerialError> {
        Ok(lock(&self.pair).ends[self.peer()].dtr)
    }

    fn in_waiting(&self) -> Result<usize, SerialError> {
        Ok(lock(&self.pair).inbox[self.side].len())
    }

    fn out_waiting(&self) -> Result<usize, SerialError> {
        Ok(0)
    }

    fn clear_buffers(&self, input: bool, _output: bool) -> Result<(), SerialError> {
        if input {
            lock(&self.pair).inbox[self.side].clear();
        }
        Ok(())
    }

    fn set_break_state(&self, on: bool) -> Result<(), SerialError> {
        lock(&self.pair).ends[self.side].break_on = on;
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::backend::Backend;

    #[tokio::test]
    async fn bytes_and_lines_cross_over() -> Result<(), Box<dyn std::error::Error>> {
        let (a_name, b_name) = pair();
        let mut a = lookup(&a_name).ok_or("mock port a is not registered")?;
        let mut b = lookup(&b_name).ok_or("mock port b is not registered")?;

        a.write_all(b"hi").await?;
        assert_eq!(b.in_waiting()?, 2);
        let mut buf = [0u8; 2];
        b.read_exact(&mut buf).await?;
        assert_eq!(&buf, b"hi");

        a.set_rts(false)?;
        a.set_dtr(false)?;
        assert!(!b.cts()?);
        assert!(!b.dsr()? && !b.cd()?);
        assert!(a.dsr()? && a.cd()?);
        Ok(())
    }

    #[tokio::test]
    async fn pending_read_wakes_when_peer_writes() -> Result<(), Box<dyn std::error::Error>> {
        let (a_name, b_name) = pair();
        let mut a = lookup(&a_name).ok_or("mock port a is not registered")?;
        let mut b = lookup(&b_name).ok_or("mock port b is not registered")?;
        let read = tokio::spawn(async move {
            let mut buf = [0u8; 2];
            b.read_exact(&mut buf).await?;
            Ok::<_, std::io::Error>(buf)
        });
        tokio::task::yield_now().await;
        assert!(!read.is_finished());
        a.write_all(b"ok").await?;
        assert_eq!(&read.await??, b"ok");
        Ok(())
    }

    #[tokio::test]
    async fn blocked_writes_resume_when_unblocked() -> Result<(), Box<dyn std::error::Error>> {
        let (a_name, _b_name) = pair();
        let mut a = lookup(&a_name).ok_or("mock port a is not registered")?;
        update(&a_name, |end| end.write_blocked = true);
        let write = tokio::spawn(async move { a.write_all(b"x").await });
        tokio::task::yield_now().await;
        assert!(!write.is_finished());
        update(&a_name, |end| end.write_blocked = false);
        write.await??;
        Ok(())
    }
}
