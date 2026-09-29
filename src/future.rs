use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use pyo3::exceptions::PyTimeoutError;
use pyo3::exceptions::asyncio::{CancelledError, InvalidStateError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyCFunction, PyList};
use tokio::task::AbortHandle;

use crate::errors::SerialError;
use crate::lock;
use crate::runtime::runtime;

/// Value produced by a finished port operation.
#[derive(Clone)]
pub enum Outcome {
    Unit,
    Int(usize),
    Bytes(Vec<u8>),
    Lines(Vec<Vec<u8>>),
    Object(Arc<Py<PyAny>>),
}

impl Outcome {
    fn to_py(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        Ok(match self {
            Self::Unit => py.None(),
            Self::Int(n) => n.into_pyobject(py)?.into_any().unbind(),
            Self::Bytes(bytes) => PyBytes::new(py, bytes).into_any().unbind(),
            Self::Lines(lines) => PyList::new(py, lines.iter().map(|line| PyBytes::new(py, line)))?
                .into_any()
                .unbind(),
            Self::Object(object) => object.clone_ref(py),
        })
    }
}

type Resolution = Result<Outcome, SerialError>;

struct Waiter {
    event_loop: Py<PyAny>,
    future: Py<PyAny>,
}

#[derive(Default)]
struct State {
    result: Option<Resolution>,
    waiters: Vec<Waiter>,
    abort: Option<AbortHandle>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    finished: Condvar,
}

fn to_python(py: Python<'_>, resolution: &Resolution) -> PyResult<Py<PyAny>> {
    match resolution {
        Ok(outcome) => outcome.to_py(py),
        Err(err) => Err(err.clone().into()),
    }
}

/// Completes an asyncio future unless it is already done, for example cancelled.
fn settle(future: &Bound<'_, PyAny>, value: &PyResult<Py<PyAny>>) -> PyResult<()> {
    let py = future.py();
    if future.call_method0("done")?.is_truthy()? {
        return Ok(());
    }
    match value {
        Ok(v) => future.call_method1("set_result", (v.clone_ref(py),))?,
        Err(err) if err.is_instance_of::<CancelledError>(py) => future.call_method0("cancel")?,
        Err(err) => future.call_method1("set_exception", (err.clone_ref(py).into_value(py),))?,
    };
    Ok(())
}

/// Hands the result to `waiter`'s loop; asyncio futures may only be touched from their loop's thread.
fn schedule(py: Python<'_>, waiter: &Waiter, resolution: &Resolution) -> PyResult<()> {
    let future = waiter.future.clone_ref(py);
    let value = to_python(py, resolution);
    let callback = PyCFunction::new_closure(py, None, None, move |args, _kwargs| {
        settle(future.bind(args.py()), &value)
    })?;
    waiter
        .event_loop
        .bind(py)
        .call_method1("call_soon_threadsafe", (callback,))?;
    Ok(())
}

/// Stores the first resolution and wakes every waiter; later calls return false.
fn complete(shared: &Shared, resolution: Resolution) -> bool {
    let waiters = {
        let mut state = lock(&shared.state);
        if state.result.is_some() {
            return false;
        }
        state.abort = None;
        state.result = Some(resolution.clone());
        std::mem::take(&mut state.waiters)
    };
    shared.finished.notify_all();
    if !waiters.is_empty() {
        // None when the interpreter is finalizing; its loops are gone by then.
        Python::try_attach(|py| {
            for waiter in &waiters {
                // A closed loop raises RuntimeError, and nobody can be waiting on it.
                let _ = schedule(py, waiter, &resolution);
            }
        });
    }
    true
}

fn cancel_shared(shared: &Shared) -> bool {
    if let Some(handle) = lock(&shared.state).abort.take() {
        handle.abort();
    }
    complete(shared, Err(SerialError::Cancelled))
}

/// A port operation running on the runtime.
#[pyclass(name = "Future", module = "oxiserial.aio", frozen, generic)]
pub struct OpFuture {
    shared: Arc<Shared>,
}

impl OpFuture {
    pub fn spawn<F>(op: F) -> Result<Self, SerialError>
    where
        F: std::future::Future<Output = Resolution> + Send + 'static,
    {
        let shared = Arc::new(Shared::default());
        let task_shared = Arc::clone(&shared);
        let task = runtime()?.spawn(async move {
            complete(&task_shared, op.await);
        });
        let mut state = lock(&shared.state);
        if state.result.is_none() {
            state.abort = Some(task.abort_handle());
        }
        drop(state);
        Ok(Self { shared })
    }

    pub fn ready(outcome: Outcome) -> Self {
        let shared = Shared::default();
        lock(&shared.state).result = Some(Ok(outcome));
        Self {
            shared: Arc::new(shared),
        }
    }

    /// Waits for the result; an interrupted wait cancels the operation so it cannot consume later data.
    pub fn block(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.wait(py, None).inspect_err(|_| {
            cancel_shared(&self.shared);
        })
    }
}

#[pymethods]
impl OpFuture {
    #[pyo3(signature = (timeout = None))]
    fn wait(&self, py: Python<'_>, timeout: Option<f64>) -> PyResult<Py<PyAny>> {
        // Short slices let Ctrl-C through while the wait is otherwise unbounded.
        const SLICE: Duration = Duration::from_millis(50);
        let deadline = timeout
            .filter(|t| t.is_finite())
            .map(|t| Instant::now() + Duration::from_secs_f64(t.max(0.0)));
        let shared = &*self.shared;
        loop {
            let finished = py.detach(|| {
                let pause = deadline.map_or(SLICE, |d| {
                    d.saturating_duration_since(Instant::now()).min(SLICE)
                });
                let state = lock(&shared.state);
                let (state, _) = shared
                    .finished
                    .wait_timeout_while(state, pause, |s| s.result.is_none())
                    .unwrap_or_else(PoisonError::into_inner);
                state.result.is_some()
            });
            if finished {
                return self.result(py);
            }
            py.check_signals()?;
            if deadline.is_some_and(|d| Instant::now() >= d) {
                return Err(PyTimeoutError::new_err(
                    "the operation did not finish within the timeout",
                ));
            }
        }
    }

    fn __await__<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyAny>> {
        let py = slf.py();
        let event_loop = py.import("asyncio")?.call_method0("get_running_loop")?;
        let future = event_loop.call_method0("create_future")?;
        let shared = Arc::clone(&slf.get().shared);
        let finished = {
            let mut state = lock(&shared.state);
            if state.result.is_none() {
                state.waiters.push(Waiter {
                    event_loop: event_loop.clone().unbind(),
                    future: future.clone().unbind(),
                });
            }
            state.result.clone()
        };
        if let Some(resolution) = finished {
            settle(&future, &to_python(py, &resolution))?;
        }
        let on_done = PyCFunction::new_closure(py, None, None, move |args, _kwargs| {
            if args.get_item(0)?.call_method0("cancelled")?.is_truthy()? {
                cancel_shared(&shared);
            }
            PyResult::Ok(())
        })?;
        future.call_method1("add_done_callback", (on_done,))?;
        future.call_method0("__await__")
    }

    fn done(&self) -> bool {
        lock(&self.shared.state).result.is_some()
    }

    fn cancel(&self) -> bool {
        cancel_shared(&self.shared)
    }

    fn result(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let resolution = lock(&self.shared.state).result.clone();
        match resolution {
            None => Err(InvalidStateError::new_err("Result is not ready.")),
            Some(resolution) => to_python(py, &resolution),
        }
    }
}
