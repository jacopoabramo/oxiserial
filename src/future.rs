use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use pyo3::exceptions::PyTimeoutError;
use pyo3::exceptions::asyncio::{CancelledError, InvalidStateError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyCFunction, PyDict, PyGenericAlias, PyList, PyType};
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

/// A done callback and where to run it.
struct Callback {
    func: Py<PyAny>,
    future: Py<PyAny>,
    context: Py<PyAny>,
    /// The loop that was running when the callback was added.
    event_loop: Option<Py<PyAny>>,
}

impl Callback {
    /// Hands the callback to its loop through `schedule`, or calls it in this thread without one.
    fn run(&self, py: Python<'_>, schedule: &str) -> PyResult<()> {
        let args = (self.func.clone_ref(py), self.future.clone_ref(py));
        let Some(event_loop) = &self.event_loop else {
            return self.context.bind(py).call_method1("run", args).map(drop);
        };
        let event_loop = event_loop.bind(py);
        // A closed loop can no longer run anything, so there is nobody left to call back.
        if event_loop.call_method0("is_closed")?.is_truthy()? {
            return Ok(());
        }
        let kwargs = PyDict::new(py);
        kwargs.set_item("context", &self.context)?;
        event_loop
            .call_method(schedule, args, Some(&kwargs))
            .map(drop)
    }

    fn run_or_report(&self, py: Python<'_>, schedule: &str) {
        if let Err(err) = self.run(py, schedule) {
            err.write_unraisable(py, Some(self.func.bind(py)));
        }
    }
}

#[derive(Default)]
struct State {
    result: Option<Resolution>,
    waiters: Vec<Waiter>,
    callbacks: Vec<Arc<Callback>>,
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

/// Stores the first resolution, wakes every waiter and runs the done callbacks; later calls return false.
fn complete(shared: &Shared, resolution: Resolution) -> bool {
    let (waiters, callbacks) = {
        let mut state = lock(&shared.state);
        if state.result.is_some() {
            return false;
        }
        state.abort = None;
        state.result = Some(resolution.clone());
        (
            std::mem::take(&mut state.waiters),
            std::mem::take(&mut state.callbacks),
        )
    };
    shared.finished.notify_all();
    if !waiters.is_empty() || !callbacks.is_empty() {
        // None when the interpreter is finalizing; its loops are gone by then.
        Python::try_attach(|py| {
            for waiter in waiters {
                let closed = waiter
                    .event_loop
                    .bind(py)
                    .call_method0("is_closed")
                    .and_then(|closed| closed.is_truthy());
                match closed {
                    // Nobody can receive a result on a closed loop; scheduling on it would raise.
                    Ok(true) => {}
                    Ok(false) => {
                        if let Err(err) = schedule(py, &waiter, &resolution) {
                            err.write_unraisable(py, None);
                        }
                    }
                    Err(err) => err.write_unraisable(py, None),
                }
            }
            for callback in callbacks {
                callback.run_or_report(py, "call_soon_threadsafe");
            }
        });
    }
    true
}

fn cancel_shared(shared: &Shared) -> bool {
    let handle = lock(&shared.state).abort.clone();
    let cancelled = complete(shared, Err(SerialError::Cancelled));
    // Aborting first would let the task's drop guard store a panic result before this one.
    if cancelled && let Some(handle) = handle {
        handle.abort();
    }
    cancelled
}

struct CompleteOnDrop(Arc<Shared>);

impl Drop for CompleteOnDrop {
    fn drop(&mut self) {
        complete(
            &self.0,
            Err(SerialError::Os {
                errno: None,
                message: "the operation panicked".into(),
            }),
        );
    }
}

/// A port operation running on the runtime.
#[pyclass(name = "Future", module = "oxiserial.aio", frozen)]
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
            // tokio catches task panics, so without this guard waiters would never wake.
            let _guard = CompleteOnDrop(Arc::clone(&task_shared));
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
        let deadline = timeout.and_then(|t| {
            let span = Duration::try_from_secs_f64(t.max(0.0)).ok()?;
            Instant::now().checked_add(span)
        });
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

    #[classmethod]
    #[pyo3(signature = (item, /))]
    fn __class_getitem__<'py>(
        cls: &Bound<'py, PyType>,
        item: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyGenericAlias>> {
        PyGenericAlias::new(cls.py(), cls.as_any(), item)
    }

    #[pyo3(signature = (r#fn, /, *, context = None))]
    fn add_done_callback(
        slf: &Bound<'_, Self>,
        r#fn: Py<PyAny>,
        context: Option<Py<PyAny>>,
    ) -> PyResult<()> {
        let py = slf.py();
        let context = match context {
            Some(context) => context,
            None => py
                .import("contextvars")?
                .call_method0("copy_context")?
                .unbind(),
        };
        let event_loop = py.import("asyncio")?.call_method0("_get_running_loop")?;
        let callback = Arc::new(Callback {
            func: r#fn,
            future: slf.clone().into_any().unbind(),
            context,
            event_loop: (!event_loop.is_none()).then(|| event_loop.unbind()),
        });
        {
            let mut state = lock(&slf.get().shared.state);
            if state.result.is_none() {
                state.callbacks.push(callback);
                return Ok(());
            }
        }
        callback.run_or_report(py, "call_soon");
        Ok(())
    }

    #[pyo3(signature = (r#fn, /))]
    fn remove_done_callback(&self, r#fn: &Bound<'_, PyAny>) -> PyResult<usize> {
        let registered = lock(&self.shared.state).callbacks.clone();
        let mut matched = Vec::new();
        for callback in registered {
            if callback.func.bind(r#fn.py()).eq(r#fn)? {
                matched.push(callback);
            }
        }
        // Dropping a callback can run Python code, so the removed ones are dropped after the lock.
        let removed: Vec<_> = {
            let mut state = lock(&self.shared.state);
            let (removed, kept) = std::mem::take(&mut state.callbacks)
                .into_iter()
                .partition(|callback| matched.iter().any(|m| Arc::ptr_eq(callback, m)));
            state.callbacks = kept;
            removed
        };
        Ok(removed.len())
    }

    fn cancelled(&self) -> bool {
        matches!(
            lock(&self.shared.state).result,
            Some(Err(SerialError::Cancelled))
        )
    }

    fn exception(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        let failure = match &lock(&self.shared.state).result {
            None => None,
            Some(Ok(_)) => return Ok(None),
            Some(Err(err)) => Some(err.clone()),
        };
        match failure {
            None => Err(InvalidStateError::new_err("Exception is not set.")),
            Some(SerialError::Cancelled) => Err(SerialError::Cancelled.into()),
            Some(err) => Ok(Some(PyErr::from(err).into_value(py).into_any())),
        }
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
