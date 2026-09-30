use std::ffi::c_void;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::pin::Pin;
use std::ptr;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use serialport::ClearBuffer;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use windows_sys::Win32::Devices::Communication::{
    CLRDTR, CLRRTS, COMMTIMEOUTS, COMSTAT, ClearCommBreak, ClearCommError, ESCAPE_COMM_FUNCTION,
    EV_RXCHAR, EscapeCommFunction, GetCommModemStatus, MODEM_STATUS_FLAGS, MS_CTS_ON, MS_DSR_ON,
    MS_RING_ON, MS_RLSD_ON, PURGE_RXABORT, PURGE_RXCLEAR, PURGE_TXABORT, PURGE_TXCLEAR, PurgeComm,
    SETDTR, SETRTS, SetCommBreak, SetCommMask, SetCommTimeouts, SetupComm, WaitCommEvent,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_IO_INCOMPLETE, ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, GENERIC_READ,
    GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, ReadFile, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, RegisterWaitForSingleObject, ResetEvent, UnregisterWaitEx,
    WT_EXECUTEONLYONCE,
};

use crate::lock;

type WakerCell = Mutex<Option<Waker>>;

fn last_error() -> io::Error {
    io::Error::last_os_error()
}

/// The error of a Win32 call that returned `ok`, with its Windows error code.
fn check(ok: i32) -> io::Result<()> {
    if ok == 0 { Err(last_error()) } else { Ok(()) }
}

fn is(err: &io::Error, code: u32) -> bool {
    err.raw_os_error() == Some(code as i32)
}

/// Thread-pool callback: wakes the task waiting on an overlapped operation.
unsafe extern "system" fn wake(context: *mut c_void, _timed_out: bool) {
    // SAFETY: `context` is the waker cell passed by `Op::register`; `Op` keeps it alive
    // until `Op::unregister` has waited for every callback to return.
    let cell = unsafe { &*context.cast::<WakerCell>() };
    if let Some(waker) = lock(cell).take() {
        waker.wake();
    }
}

/// One overlapped operation slot: its OVERLAPPED, event, data buffer and wake-up registration.
///
/// The kernel may write `overlapped` and `mask` and read or write `buffer` while `pending`
/// is true, so none of them is touched, moved or freed until the operation has completed.
struct Op {
    handle: HANDLE,
    overlapped: *mut OVERLAPPED,
    mask: *mut u32,
    buffer: Vec<u8>,
    pending: bool,
    wait: HANDLE,
    waker: Arc<WakerCell>,
}

impl Op {
    fn new(handle: HANDLE) -> io::Result<Self> {
        // SAFETY: null security attributes and name are allowed; the event is closed in `drop`.
        let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if event.is_null() {
            return Err(last_error());
        }
        Ok(Self {
            handle,
            overlapped: Box::into_raw(Box::new(OVERLAPPED {
                hEvent: event,
                ..OVERLAPPED::default()
            })),
            mask: Box::into_raw(Box::new(0)),
            buffer: Vec::new(),
            pending: false,
            wait: ptr::null_mut(),
            waker: Arc::default(),
        })
    }

    /// Resets the OVERLAPPED for a new operation and returns it.
    fn start(&mut self) -> *mut OVERLAPPED {
        // SAFETY: no operation is pending, so the kernel no longer uses `overlapped`.
        unsafe {
            let event = (*self.overlapped).hEvent;
            ResetEvent(event);
            *self.overlapped = OVERLAPPED {
                hEvent: event,
                ..OVERLAPPED::default()
            };
        }
        self.overlapped
    }

    /// Marks the operation started by `start` as pending unless issuing it failed.
    fn issued(&mut self, ok: i32) -> io::Result<()> {
        if ok == 0 {
            let err = last_error();
            if !is(&err, ERROR_IO_PENDING) {
                return Err(err);
            }
        }
        self.pending = true;
        Ok(())
    }

    /// The byte count of the pending operation, or `None` while it runs; `block` waits for it.
    ///
    /// A cancelled operation reports the bytes it transferred before it stopped.
    fn result(&mut self, block: bool) -> Option<io::Result<usize>> {
        let mut count = 0;
        // SAFETY: `overlapped` belongs to an operation started on `handle`, which is open.
        let ok =
            unsafe { GetOverlappedResult(self.handle, self.overlapped, &mut count, block.into()) };
        let result = if ok == 0 {
            let err = last_error();
            if is(&err, ERROR_IO_INCOMPLETE) {
                return None;
            }
            if is(&err, ERROR_OPERATION_ABORTED) {
                Ok(count as usize)
            } else {
                Err(err)
            }
        } else {
            Ok(count as usize)
        };
        self.pending = false;
        self.unregister();
        Some(result)
    }

    /// Stores the task's waker and makes sure a callback wakes it when the event is signalled.
    fn register(&mut self, cx: &Context<'_>) -> io::Result<()> {
        *lock(&self.waker) = Some(cx.waker().clone());
        if !self.wait.is_null() {
            return Ok(());
        }
        let mut wait = ptr::null_mut();
        // SAFETY: the event outlives the registration, which `unregister` removes before the
        // event is closed; the context is the waker cell, kept alive by `self` until then.
        let ok = unsafe {
            RegisterWaitForSingleObject(
                &mut wait,
                (*self.overlapped).hEvent,
                Some(wake),
                Arc::as_ptr(&self.waker).cast(),
                INFINITE,
                WT_EXECUTEONLYONCE,
            )
        };
        if ok == 0 {
            return Err(last_error());
        }
        self.wait = wait;
        Ok(())
    }

    fn unregister(&mut self) {
        if self.wait.is_null() {
            return;
        }
        // SAFETY: INVALID_HANDLE_VALUE makes the call wait for a running callback, so no
        // callback uses the waker cell afterwards. It fails only for an invalid wait handle,
        // which has no callback left to wait for.
        unsafe { UnregisterWaitEx(self.wait, INVALID_HANDLE_VALUE) };
        self.wait = ptr::null_mut();
    }

    /// Cancels the pending operation and returns the bytes it transferred.
    fn cancel(&mut self) -> io::Result<usize> {
        if !self.pending {
            return Ok(0);
        }
        // SAFETY: `overlapped` identifies an operation on `handle`. A failure means it already
        // completed, which the blocking `result` below then reports.
        unsafe { CancelIoEx(self.handle, self.overlapped) };
        self.result(true).unwrap_or(Ok(0))
    }
}

impl Drop for Op {
    fn drop(&mut self) {
        // Dropping the port can report nothing, and waiting is what makes freeing safe.
        let _ = self.cancel();
        self.unregister();
        // SAFETY: nothing is pending and no callback is registered, so neither the kernel
        // nor the thread pool uses these allocations or the event any more.
        unsafe {
            CloseHandle((*self.overlapped).hEvent);
            drop(Box::from_raw(self.overlapped));
            drop(Box::from_raw(self.mask));
        }
    }
}

/// A COM port opened for overlapped I/O, read only as far as the driver has buffered data.
///
/// Status and control calls go to Win32 directly rather than through serialport, whose
/// errors do not keep the Windows error code.
pub struct Port {
    // The operations are dropped before `file`, which closes the handle they run on.
    readiness: Op,
    read: Op,
    write: Op,
    // The pending write was handed to the driver by a non-blocking `write` that already returned.
    write_detached: bool,
    file: OwnedHandle,
}

// SAFETY: the raw pointers and handles in `Op` are owned by the port and used only through
// `&mut self` or by the kernel and thread pool, which synchronise on their own.
unsafe impl Send for Port {}

impl Port {
    pub fn open(path: &str) -> io::Result<Self> {
        // The device namespace prefix lets names past COM9 open.
        let mut name: Vec<u16> = if path.starts_with('\\') {
            Vec::new()
        } else {
            r"\\.\".encode_utf16().collect()
        };
        name.extend(path.encode_utf16());
        name.push(0);
        // SAFETY: `name` is NUL-terminated and outlives the call; the other pointers may be null.
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(last_error());
        }
        // SAFETY: `handle` is a freshly opened port that nothing else owns.
        let file = unsafe { OwnedHandle::from_raw_handle(handle) };
        // Matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt): 4096-byte driver queues,
        // result ignored, as a driver that rejects the size still works with its default queues.
        // SAFETY: `handle` is open and owned by `file`.
        unsafe { SetupComm(handle, 4096, 4096) };
        // ReadFile returns at once with whatever is buffered; timeouts are kept by the caller.
        let timeouts = COMMTIMEOUTS {
            ReadIntervalTimeout: u32::MAX,
            ..COMMTIMEOUTS::default()
        };
        // SAFETY: as above.
        check(unsafe { SetCommTimeouts(handle, &timeouts) })?;
        // SAFETY: as above.
        check(unsafe { SetCommMask(handle, EV_RXCHAR) })?;
        Ok(Self {
            readiness: Op::new(handle)?,
            read: Op::new(handle)?,
            write: Op::new(handle)?,
            write_detached: false,
            file,
        })
    }

    fn handle(&self) -> HANDLE {
        self.file.as_raw_handle()
    }

    /// Sets the driver's input and output queue sizes.
    pub fn setup_queues(&self, rx: u32, tx: u32) -> io::Result<()> {
        // SAFETY: the handle is open for as long as `self` lives.
        check(unsafe { SetupComm(self.handle(), rx, tx) })
    }

    fn escape(&self, function: ESCAPE_COMM_FUNCTION) -> io::Result<()> {
        // SAFETY: the handle is open for as long as `self` lives.
        check(unsafe { EscapeCommFunction(self.handle(), function) })
    }

    fn modem_line(&self, line: MODEM_STATUS_FLAGS) -> io::Result<bool> {
        let mut status = 0;
        // SAFETY: as above; `status` outlives the call.
        check(unsafe { GetCommModemStatus(self.handle(), &mut status) })?;
        Ok(status & line != 0)
    }

    fn comm_status(&self) -> io::Result<COMSTAT> {
        let mut errors = 0;
        let mut status = COMSTAT::default();
        // SAFETY: as above; `errors` and `status` outlive the call.
        check(unsafe { ClearCommError(self.handle(), &mut errors, &mut status) })?;
        Ok(status)
    }

    // The methods below are named after serialport's `SerialPort` methods, so the backend code
    // is shared with the POSIX port.
    pub fn write_request_to_send(&mut self, level: bool) -> io::Result<()> {
        self.escape(if level { SETRTS } else { CLRRTS })
    }

    pub fn write_data_terminal_ready(&mut self, level: bool) -> io::Result<()> {
        self.escape(if level { SETDTR } else { CLRDTR })
    }

    pub fn read_clear_to_send(&mut self) -> io::Result<bool> {
        self.modem_line(MS_CTS_ON)
    }

    pub fn read_data_set_ready(&mut self) -> io::Result<bool> {
        self.modem_line(MS_DSR_ON)
    }

    pub fn read_ring_indicator(&mut self) -> io::Result<bool> {
        self.modem_line(MS_RING_ON)
    }

    pub fn read_carrier_detect(&mut self) -> io::Result<bool> {
        self.modem_line(MS_RLSD_ON)
    }

    pub fn bytes_to_read(&self) -> io::Result<u32> {
        Ok(self.comm_status()?.cbInQue)
    }

    pub fn bytes_to_write(&self) -> io::Result<u32> {
        Ok(self.comm_status()?.cbOutQue)
    }

    pub fn clear(&self, which: ClearBuffer) -> io::Result<()> {
        let flags = match which {
            ClearBuffer::Input => PURGE_RXABORT | PURGE_RXCLEAR,
            ClearBuffer::Output => PURGE_TXABORT | PURGE_TXCLEAR,
            ClearBuffer::All => PURGE_RXABORT | PURGE_RXCLEAR | PURGE_TXABORT | PURGE_TXCLEAR,
        };
        // SAFETY: as above.
        check(unsafe { PurgeComm(self.handle(), flags) })
    }

    pub fn set_break(&self) -> io::Result<()> {
        // SAFETY: as above.
        check(unsafe { SetCommBreak(self.handle()) })
    }

    pub fn clear_break(&self) -> io::Result<()> {
        // SAFETY: as above.
        check(unsafe { ClearCommBreak(self.handle()) })
    }

    /// Reads up to `len` bytes that the driver already holds.
    fn read_buffered(&mut self, len: usize) -> io::Result<&[u8]> {
        let handle = self.handle();
        let op = &mut self.read;
        op.buffer.resize(len, 0);
        let overlapped = op.start();
        // SAFETY: the buffer and OVERLAPPED belong to `op`, which is not touched again until the
        // blocking `result` below has seen the read complete, or `Op::drop` has cancelled it.
        let ok = unsafe {
            ReadFile(
                handle,
                op.buffer.as_mut_ptr(),
                op.buffer.len() as u32,
                ptr::null_mut(),
                overlapped,
            )
        };
        op.issued(ok)?;
        let n = op.result(true).unwrap_or(Ok(0))?;
        Ok(&op.buffer[..n])
    }

    /// Starts a WaitCommEvent unless one is pending; false if an event was already recorded.
    fn arm_readiness(&mut self) -> io::Result<bool> {
        if self.readiness.pending {
            return Ok(true);
        }
        let handle = self.handle();
        let op = &mut self.readiness;
        let overlapped = op.start();
        // SAFETY: the mask and OVERLAPPED belong to `op` and stay put while it is pending.
        let ok = unsafe { WaitCommEvent(handle, op.mask, overlapped) };
        if ok != 0 {
            return Ok(false);
        }
        op.issued(ok)?;
        Ok(true)
    }

    /// Cancels a write still in progress for its caller and returns the bytes it sent.
    pub fn abort_write(&mut self) -> io::Result<usize> {
        if self.write_detached {
            return Ok(0);
        }
        self.write.cancel()
    }

    /// Leaves the pending write to finish in the driver and returns the bytes it was given.
    pub fn detach_write(&mut self) -> usize {
        if !self.write.pending || self.write_detached {
            return 0;
        }
        self.write_detached = true;
        self.write.buffer.len()
    }

    /// Waits for the pending write, if any, and returns its byte count.
    fn poll_write_done(&mut self, cx: &Context<'_>) -> Poll<io::Result<usize>> {
        if !self.write.pending {
            return Poll::Ready(Ok(0));
        }
        // The waker is stored before the completion check, so a completion in between still wakes the task.
        self.write.register(cx)?;
        match self.write.result(false) {
            Some(result) => {
                self.write_detached = false;
                Poll::Ready(result)
            }
            None => Poll::Pending,
        }
    }
}

impl AsRawHandle for Port {
    fn as_raw_handle(&self) -> RawHandle {
        self.handle()
    }
}

impl AsyncRead for Port {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            let queued = this.bytes_to_read()? as usize;
            if queued > 0 {
                // Never more than the driver holds, so in_waiting and PurgeComm see every unread byte.
                let data = this.read_buffered(queued.min(buf.remaining()))?;
                if !data.is_empty() {
                    buf.put_slice(data);
                    return Poll::Ready(Ok(()));
                }
                continue;
            }
            if !this.arm_readiness()? {
                continue;
            }
            // The waker is stored before the completion check, so a completion in between still wakes the task.
            this.readiness.register(cx)?;
            match this.readiness.result(false) {
                None => return Poll::Pending,
                Some(result) => {
                    result?;
                }
            }
        }
    }
}

impl AsyncWrite for Port {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.write_detached {
            // An earlier non-blocking write finishes first; a failure of it is this call's error.
            std::task::ready!(this.poll_write_done(cx))?;
        }
        let handle = this.handle();
        let op = &mut this.write;
        // A pending write that is not detached is the one the caller is retrying with the same data.
        if !op.pending {
            if data.is_empty() {
                return Poll::Ready(Ok(0));
            }
            op.buffer.clear();
            op.buffer
                .extend_from_slice(&data[..data.len().min(u32::MAX as usize)]);
            let overlapped = op.start();
            // SAFETY: the buffer and OVERLAPPED belong to `op` and are left alone until the
            // write completes or `Op::cancel` has waited for it.
            let ok = unsafe {
                WriteFile(
                    handle,
                    op.buffer.as_ptr(),
                    op.buffer.len() as u32,
                    ptr::null_mut(),
                    overlapped,
                )
            };
            op.issued(ok)?;
            if let Some(result) = op.result(false) {
                return Poll::Ready(result);
            }
        }
        this.poll_write_done(cx)
    }

    /// Waits for a write left pending by a non-blocking `write`.
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.get_mut().poll_write_done(cx).map_ok(drop)
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
