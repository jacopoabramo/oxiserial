use std::sync::{Arc, Mutex};

use pyo3::exceptions::{PyNotImplementedError, PyRuntimeError, PyStopIteration, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyCFunction, PyDict, PyTuple, PyType};

use crate::aio::start_write;
use crate::future::{OpFuture, Outcome};
use crate::lock;
use crate::port::PortCore;
use crate::serial::{SerialBase, to_bytes};

/// Largest chunk handed to `data_received` at once.
const READ_SIZE: usize = 65_536;
// Matches pyserial-asyncio (BSD-3-Clause, see LICENSES/pyserial-asyncio.txt): default limits.
const HIGH_WATER: usize = 64 * 1024;

type Step = Box<dyn FnOnce(Python<'_>) -> PyResult<Py<PyAny>> + Send>;

/// A coroutine that does its work on the first step and finishes with the result, so it can
/// be awaited or given to `asyncio.create_task` like the result of an `async def` call.
#[pyclass(module = "oxiserial.aio", frozen)]
pub struct Deferred {
    step: Mutex<Option<Step>>,
}

impl Deferred {
    fn new(step: impl FnOnce(Python<'_>) -> PyResult<Py<PyAny>> + Send + 'static) -> Self {
        Self {
            step: Mutex::new(Some(Box::new(step))),
        }
    }
}

#[pymethods]
impl Deferred {
    fn __await__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let step = lock(&self.step)
            .take()
            .ok_or_else(|| PyRuntimeError::new_err("cannot reuse already awaited coroutine"))?;
        Err(PyStopIteration::new_err((step(py)?,)))
    }

    fn send(&self, py: Python<'_>, _value: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.__next__(py)
    }

    #[pyo3(signature = (exception, _value = None, _traceback = None))]
    fn throw(
        &self,
        exception: &Bound<'_, PyAny>,
        _value: Option<&Bound<'_, PyAny>>,
        _traceback: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        lock(&self.step).take();
        let exception = if exception.is_instance_of::<PyType>() {
            exception.call0()?
        } else {
            exception.clone()
        };
        Err(PyErr::from_value(exception))
    }

    fn close(&self) {
        lock(&self.step).take();
    }
}

#[derive(Default)]
struct State {
    /// None once `connection_lost` has been called.
    protocol: Option<Py<PyAny>>,
    closing: bool,
    reading_paused: bool,
    read: Option<Py<PyAny>>,
    /// Bytes a read returned after reading was paused, delivered on resume.
    held: Vec<u8>,
    queue: Vec<u8>,
    in_flight: usize,
    write: Option<Py<PyAny>>,
    high: usize,
    low: usize,
    protocol_paused: bool,
    lost: bool,
    close_exc: Option<Py<PyAny>>,
}

/// An asyncio transport over a serial port, with pyserial-asyncio's behaviour.
#[pyclass(module = "oxiserial.aio", frozen)]
pub struct SerialTransport {
    event_loop: Py<PyAny>,
    serial: Py<SerialBase>,
    core: Arc<PortCore>,
    state: Mutex<State>,
}

type Handler = fn(&Bound<'_, SerialTransport>, &Bound<'_, PyAny>) -> PyResult<()>;

impl SerialTransport {
    fn create<'py>(
        py: Python<'py>,
        event_loop: &Bound<'py, PyAny>,
        protocol: &Bound<'py, PyAny>,
        serial: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, Self>> {
        let serial = serial.cast::<SerialBase>()?.clone();
        // Matches pyserial-asyncio (BSD-3-Clause, see LICENSES/pyserial-asyncio.txt).
        serial.setattr("timeout", 0)?;
        serial.setattr("write_timeout", 0)?;
        let core = Arc::clone(&serial.get().core);
        let state = State {
            protocol: Some(protocol.clone().unbind()),
            high: HIGH_WATER,
            low: HIGH_WATER / 4,
            ..State::default()
        };
        let transport = Bound::new(
            py,
            Self {
                event_loop: event_loop.clone().unbind(),
                serial: serial.unbind(),
                core,
                state: Mutex::new(state),
            },
        )?;
        event_loop.call_method1(
            "call_soon",
            (protocol.getattr("connection_made")?, &transport),
        )?;
        event_loop.call_method1("call_soon", (Self::callback(&transport, Self::on_start)?,))?;
        Ok(transport)
    }

    /// A Python callable that runs `handler` with this transport and the callable's first
    /// argument (`None` when there is none).
    fn callback<'py>(
        slf: &Bound<'py, Self>,
        handler: Handler,
    ) -> PyResult<Bound<'py, PyCFunction>> {
        let transport = slf.clone().unbind();
        PyCFunction::new_closure(slf.py(), None, None, move |args, _kwargs| {
            let py = args.py();
            let arg = args
                .get_item(0)
                .unwrap_or_else(|_| py.None().into_bound(py));
            handler(transport.bind(py), &arg)
        })
    }

    fn on_start(slf: &Bound<'_, Self>, _arg: &Bound<'_, PyAny>) -> PyResult<()> {
        Self::start_reading(slf)
    }

    fn start_reading(slf: &Bound<'_, Self>) -> PyResult<()> {
        let py = slf.py();
        let this = slf.get();
        {
            let state = lock(&this.state);
            if state.closing || state.reading_paused || state.read.is_some() {
                return Ok(());
            }
        }
        let core = Arc::clone(&this.core);
        let future = OpFuture::start(py, async move {
            core.read_available(READ_SIZE).await.map(Outcome::Bytes)
        })?;
        let future = Bound::new(py, future)?;
        lock(&this.state).read = Some(future.clone().into_any().unbind());
        future.call_method1("add_done_callback", (Self::callback(slf, Self::on_read)?,))?;
        Ok(())
    }

    fn on_read(slf: &Bound<'_, Self>, future: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = slf.py();
        let this = slf.get();
        lock(&this.state).read = None;
        if future.call_method0("cancelled")?.is_truthy()? {
            return Ok(());
        }
        match future.call_method0("result") {
            Ok(data) => {
                let protocol = {
                    let mut state = lock(&this.state);
                    if state.closing {
                        return Ok(());
                    }
                    if state.reading_paused {
                        state
                            .held
                            .extend_from_slice(data.cast::<PyBytes>()?.as_bytes());
                        return Ok(());
                    }
                    state.protocol.as_ref().map(|p| p.clone_ref(py))
                };
                // The next read starts first, so a failing data_received does not stop reading.
                Self::start_reading(slf)?;
                if let Some(protocol) = protocol {
                    protocol.call_method1(py, "data_received", (data,))?;
                }
                Ok(())
            }
            Err(err) => Self::close_with(slf, Some(err.into_value(py).into_any())),
        }
    }

    fn start_writing(slf: &Bound<'_, Self>) -> PyResult<()> {
        let py = slf.py();
        let this = slf.get();
        let data = {
            let mut state = lock(&this.state);
            if state.write.is_some() || state.queue.is_empty() {
                return Ok(());
            }
            let data = std::mem::take(&mut state.queue);
            state.in_flight = data.len();
            data
        };
        let core = Arc::clone(&this.core);
        let future = start_write(
            py,
            async move { core.write_all(&data).await.map(Outcome::Int) },
        )?;
        let future = Bound::new(py, future)?;
        lock(&this.state).write = Some(future.clone().into_any().unbind());
        future.call_method1("add_done_callback", (Self::callback(slf, Self::on_write)?,))?;
        Ok(())
    }

    fn on_write(slf: &Bound<'_, Self>, future: &Bound<'_, PyAny>) -> PyResult<()> {
        let this = slf.get();
        {
            let mut state = lock(&this.state);
            state.write = None;
            state.in_flight = 0;
        }
        if future.call_method0("cancelled")?.is_truthy()? {
            return Ok(());
        }
        if let Err(err) = future.call_method0("result") {
            return Self::fatal_error(slf, err);
        }
        Self::maybe_resume(slf)?;
        let (more, finish) = {
            let state = lock(&this.state);
            (
                !state.queue.is_empty(),
                state.closing && state.queue.is_empty() && !state.lost,
            )
        };
        if more {
            Self::start_writing(slf)
        } else if finish {
            Self::finish(slf, true)
        } else {
            Ok(())
        }
    }

    fn maybe_pause(slf: &Bound<'_, Self>) -> PyResult<()> {
        let protocol = {
            let mut state = lock(&slf.get().state);
            if state.protocol_paused || state.queue.len() + state.in_flight <= state.high {
                return Ok(());
            }
            state.protocol_paused = true;
            state.protocol.as_ref().map(|p| p.clone_ref(slf.py()))
        };
        Self::notify(slf, protocol, "pause_writing")
    }

    fn maybe_resume(slf: &Bound<'_, Self>) -> PyResult<()> {
        let protocol = {
            let mut state = lock(&slf.get().state);
            if !state.protocol_paused || state.queue.len() + state.in_flight > state.low {
                return Ok(());
            }
            state.protocol_paused = false;
            state.protocol.as_ref().map(|p| p.clone_ref(slf.py()))
        };
        Self::notify(slf, protocol, "resume_writing")
    }

    /// Calls a flow-control method, reporting its failure to the loop rather than raising.
    fn notify(slf: &Bound<'_, Self>, protocol: Option<Py<PyAny>>, method: &str) -> PyResult<()> {
        let py = slf.py();
        let Some(protocol) = protocol else {
            return Ok(());
        };
        if let Err(err) = protocol.call_method0(py, method) {
            // Matches pyserial-asyncio (BSD-3-Clause, see LICENSES/pyserial-asyncio.txt): message.
            Self::report(
                slf,
                &format!("protocol.{method}() failed"),
                err.into_value(py).into_any(),
            )?;
        }
        Ok(())
    }

    fn report(slf: &Bound<'_, Self>, message: &str, exception: Py<PyAny>) -> PyResult<()> {
        let py = slf.py();
        let context = PyDict::new(py);
        context.set_item("message", message)?;
        context.set_item("exception", exception)?;
        context.set_item("transport", slf)?;
        let protocol = lock(&slf.get().state)
            .protocol
            .as_ref()
            .map(|p| p.clone_ref(py));
        context.set_item("protocol", protocol)?;
        slf.get()
            .event_loop
            .call_method1(py, "call_exception_handler", (context,))?;
        Ok(())
    }

    fn fatal_error(slf: &Bound<'_, Self>, err: PyErr) -> PyResult<()> {
        let exception = err.into_value(slf.py()).into_any();
        // Matches pyserial-asyncio (BSD-3-Clause, see LICENSES/pyserial-asyncio.txt): message.
        Self::report(
            slf,
            "Fatal error on serial transport",
            exception.clone_ref(slf.py()),
        )?;
        Self::abort_with(slf, Some(exception))
    }

    /// Stops reading and closes once the queue is sent, with `exc` for `connection_lost`.
    fn close_with(slf: &Bound<'_, Self>, exc: Option<Py<PyAny>>) -> PyResult<()> {
        let py = slf.py();
        let (read, done) = {
            let mut state = lock(&slf.get().state);
            if state.closing {
                return Ok(());
            }
            state.closing = true;
            state.close_exc = exc;
            (
                state.read.take(),
                state.queue.is_empty() && state.write.is_none(),
            )
        };
        if let Some(read) = read {
            read.call_method0(py, "cancel")?;
        }
        if done {
            Self::finish(slf, true)?;
        }
        Ok(())
    }

    fn abort_with(slf: &Bound<'_, Self>, exc: Option<Py<PyAny>>) -> PyResult<()> {
        let py = slf.py();
        let (read, write) = {
            let mut state = lock(&slf.get().state);
            state.closing = true;
            if exc.is_some() {
                state.close_exc = exc;
            }
            state.queue.clear();
            state.in_flight = 0;
            (state.read.take(), state.write.take())
        };
        for future in [read, write].into_iter().flatten() {
            future.call_method0(py, "cancel")?;
        }
        Self::finish(slf, false)
    }

    /// Calls `connection_lost` once, after a flush of the driver's buffer when `drain`.
    fn finish(slf: &Bound<'_, Self>, drain: bool) -> PyResult<()> {
        let py = slf.py();
        let this = slf.get();
        {
            let mut state = lock(&this.state);
            if state.lost {
                return Ok(());
            }
            state.lost = true;
        }
        let lost = Self::callback(slf, Self::connection_lost)?;
        if drain {
            let core = Arc::clone(&this.core);
            let future = start_write(
                py,
                async move { core.flush().await.map(|()| Outcome::Unit) },
            )?;
            Bound::new(py, future)?.call_method1("add_done_callback", (lost,))?;
        } else {
            this.event_loop
                .call_method1(py, "call_soon", (lost, py.None()))?;
        }
        Ok(())
    }

    fn connection_lost(slf: &Bound<'_, Self>, _arg: &Bound<'_, PyAny>) -> PyResult<()> {
        let py = slf.py();
        let this = slf.get();
        let (protocol, exc) = {
            let mut state = lock(&this.state);
            (state.protocol.take(), state.close_exc.take())
        };
        let result = match protocol {
            Some(protocol) => protocol
                .call_method1(py, "connection_lost", (exc,))
                .map(drop),
            None => Ok(()),
        };
        // The port closes even when the protocol's connection_lost raised.
        this.serial.bind(py).call_method0("close")?;
        result
    }
}

#[pymethods]
impl SerialTransport {
    #[getter(r#loop)]
    fn event_loop(&self, py: Python<'_>) -> Py<PyAny> {
        self.event_loop.clone_ref(py)
    }

    #[getter]
    fn serial(&self, py: Python<'_>) -> Py<SerialBase> {
        self.serial.clone_ref(py)
    }

    #[pyo3(signature = (name, default = None))]
    fn get_extra_info(&self, py: Python<'_>, name: &str, default: Option<Py<PyAny>>) -> Py<PyAny> {
        if name == "serial" {
            self.serial.clone_ref(py).into_any()
        } else {
            default.unwrap_or_else(|| py.None())
        }
    }

    fn is_closing(&self) -> bool {
        lock(&self.state).closing
    }

    fn is_reading(&self) -> bool {
        let state = lock(&self.state);
        !state.closing && !state.reading_paused
    }

    fn close(slf: &Bound<'_, Self>) -> PyResult<()> {
        Self::close_with(slf, None)
    }

    fn abort(slf: &Bound<'_, Self>) -> PyResult<()> {
        Self::abort_with(slf, None)
    }

    fn write(slf: &Bound<'_, Self>, data: &Bound<'_, PyAny>) -> PyResult<()> {
        let data = to_bytes(data)?;
        let start = {
            let mut state = lock(&slf.get().state);
            if state.closing || data.is_empty() {
                return Ok(());
            }
            state.queue.extend_from_slice(&data);
            state.write.is_none()
        };
        if start {
            Self::start_writing(slf)?;
        }
        Self::maybe_pause(slf)
    }

    fn writelines(slf: &Bound<'_, Self>, list_of_data: &Bound<'_, PyAny>) -> PyResult<()> {
        for data in list_of_data.try_iter()? {
            Self::write(slf, &data?)?;
        }
        Ok(())
    }

    fn can_write_eof(&self) -> bool {
        false
    }

    fn write_eof(&self) -> PyResult<()> {
        // Matches pyserial-asyncio (BSD-3-Clause, see LICENSES/pyserial-asyncio.txt): message.
        Err(PyNotImplementedError::new_err(
            "Serial connections do not support end-of-file",
        ))
    }

    fn pause_reading(slf: &Bound<'_, Self>) -> PyResult<()> {
        let read = {
            let mut state = lock(&slf.get().state);
            state.reading_paused = true;
            state.read.take()
        };
        if let Some(read) = read {
            read.call_method0(slf.py(), "cancel")?;
        }
        Ok(())
    }

    fn resume_reading(slf: &Bound<'_, Self>) -> PyResult<()> {
        let py = slf.py();
        let this = slf.get();
        let (held, protocol) = {
            let mut state = lock(&this.state);
            if !state.reading_paused {
                return Ok(());
            }
            state.reading_paused = false;
            (
                std::mem::take(&mut state.held),
                state.protocol.as_ref().map(|p| p.clone_ref(py)),
            )
        };
        if !held.is_empty()
            && let Some(protocol) = protocol
        {
            // Scheduled, not called, so a StreamReader resuming from inside read() is not re-entered.
            this.event_loop.call_method1(
                py,
                "call_soon",
                (
                    protocol.getattr(py, "data_received")?,
                    PyBytes::new(py, &held),
                ),
            )?;
        }
        Self::start_reading(slf)
    }

    #[pyo3(signature = (high = None, low = None))]
    fn set_write_buffer_limits(
        slf: &Bound<'_, Self>,
        high: Option<isize>,
        low: Option<isize>,
    ) -> PyResult<()> {
        // Matches pyserial-asyncio (BSD-3-Clause, see LICENSES/pyserial-asyncio.txt): defaults and message.
        let high = high.unwrap_or_else(|| low.map_or(HIGH_WATER as isize, |low| 4 * low));
        let low = low.unwrap_or(high / 4);
        if !(high >= low && low >= 0) {
            return Err(PyValueError::new_err(format!(
                "high ({high}) must be >= low ({low}) must be >= 0"
            )));
        }
        {
            let mut state = lock(&slf.get().state);
            state.high = high as usize;
            state.low = low as usize;
        }
        Self::maybe_pause(slf)
    }

    fn get_write_buffer_limits(&self) -> (usize, usize) {
        let state = lock(&self.state);
        (state.low, state.high)
    }

    fn get_write_buffer_size(&self) -> usize {
        let state = lock(&self.state);
        state.queue.len() + state.in_flight
    }

    fn flush(slf: &Bound<'_, Self>) -> PyResult<()> {
        lock(&slf.get().state).queue.clear();
        Self::maybe_resume(slf)
    }

    fn get_protocol(&self, py: Python<'_>) -> Option<Py<PyAny>> {
        lock(&self.state).protocol.as_ref().map(|p| p.clone_ref(py))
    }

    fn set_protocol(&self, protocol: Py<PyAny>) {
        lock(&self.state).protocol = Some(protocol);
    }

    fn __repr__(slf: &Bound<'_, Self>) -> PyResult<String> {
        let py = slf.py();
        let this = slf.get();
        let protocol = this.get_protocol(py);
        Ok(format!(
            "SerialTransport({}, {}, {})",
            this.event_loop.bind(py).repr()?,
            protocol.map_or(Ok("None".to_owned()), |p| p
                .bind(py)
                .repr()
                .map(|r| r.to_string()))?,
            this.serial.bind(py).repr()?,
        ))
    }
}

fn connect(
    py: Python<'_>,
    event_loop: &Bound<'_, PyAny>,
    protocol_factory: &Bound<'_, PyAny>,
    serial: &Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    let protocol = protocol_factory.call0()?;
    let transport = SerialTransport::create(py, event_loop, &protocol, serial)?;
    Ok(PyTuple::new(py, [transport.into_any(), protocol])?
        .into_any()
        .unbind())
}

/// Opens `url` like `oxiserial.serial_for_url` and connects it to a new protocol.
#[pyfunction]
#[pyo3(signature = (r#loop, protocol_factory, url, *args, **kwargs))]
pub fn create_serial_connection(
    r#loop: Py<PyAny>,
    protocol_factory: Py<PyAny>,
    url: Py<PyAny>,
    args: Py<PyTuple>,
    kwargs: Option<Py<PyDict>>,
) -> Deferred {
    Deferred::new(move |py| {
        let mut call_args = vec![url.into_bound(py)];
        call_args.extend(args.bind(py).iter());
        let serial = py.import("oxiserial")?.getattr("serial_for_url")?.call(
            PyTuple::new(py, call_args)?,
            kwargs.as_ref().map(|k| k.bind(py)),
        )?;
        connect(py, r#loop.bind(py), protocol_factory.bind(py), &serial)
    })
}

/// Connects an open port to a new protocol.
#[pyfunction]
#[pyo3(signature = (r#loop, protocol_factory, serial_instance))]
pub fn connection_for_serial(
    r#loop: Py<PyAny>,
    protocol_factory: Py<PyAny>,
    serial_instance: Py<PyAny>,
) -> Deferred {
    Deferred::new(move |py| {
        connect(
            py,
            r#loop.bind(py),
            protocol_factory.bind(py),
            serial_instance.bind(py),
        )
    })
}

/// Opens a port and returns an asyncio `StreamReader` and `StreamWriter` for it.
#[pyfunction]
#[pyo3(signature = (*, r#loop = None, limit = None, **kwargs))]
pub fn open_serial_connection(
    r#loop: Option<Py<PyAny>>,
    limit: Option<Py<PyAny>>,
    kwargs: Option<Py<PyDict>>,
) -> Deferred {
    Deferred::new(move |py| {
        let asyncio = py.import("asyncio")?;
        let event_loop = match r#loop {
            Some(event_loop) => event_loop.into_bound(py),
            None => asyncio.call_method0("get_running_loop")?,
        };
        // Matches pyserial-asyncio (BSD-3-Clause, see LICENSES/pyserial-asyncio.txt): default limit.
        let limit = match limit {
            Some(limit) => limit.into_bound(py),
            None => asyncio.getattr("streams")?.getattr("_DEFAULT_LIMIT")?,
        };
        let reader_kwargs = PyDict::new(py);
        reader_kwargs.set_item("limit", limit)?;
        reader_kwargs.set_item("loop", &event_loop)?;
        let reader = asyncio
            .getattr("StreamReader")?
            .call((), Some(&reader_kwargs))?;
        let protocol_kwargs = PyDict::new(py);
        protocol_kwargs.set_item("loop", &event_loop)?;
        let protocol = asyncio
            .getattr("StreamReaderProtocol")?
            .call((&reader,), Some(&protocol_kwargs))?;
        let serial = py
            .import("oxiserial")?
            .getattr("serial_for_url")?
            .call((), kwargs.as_ref().map(|k| k.bind(py)))?;
        let transport = SerialTransport::create(py, &event_loop, &protocol, &serial)?;
        let writer = asyncio.getattr("StreamWriter")?.call1((
            &transport,
            &protocol,
            &reader,
            &event_loop,
        ))?;
        Ok(PyTuple::new(py, [reader, writer])?.into_any().unbind())
    })
}
