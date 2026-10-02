use std::time::Duration;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::backend::mock;
use crate::errors::SerialError;
use crate::future::{OpFuture, Outcome};
use crate::serial::Serial;

fn unknown(port: &str) -> PyErr {
    PyValueError::new_err(format!("unknown mock port '{port}'"))
}

#[pyfunction]
fn mock_pair() -> (String, String) {
    mock::pair()
}

#[pyfunction]
fn mock_unplug(port: &str) -> PyResult<()> {
    mock::unplug(port).ok_or_else(|| unknown(port))
}

#[pyfunction]
fn mock_block_writes(port: &str, blocked: bool) -> PyResult<()> {
    mock::update(port, |end| end.write_blocked = blocked).ok_or_else(|| unknown(port))
}

#[pyfunction]
fn mock_state<'py>(py: Python<'py>, port: &str) -> PyResult<Bound<'py, PyDict>> {
    let (baudrate, rts, dtr, break_on) =
        mock::update(port, |end| (end.baudrate, end.rts, end.dtr, end.break_on))
            .ok_or_else(|| unknown(port))?;
    let state = PyDict::new(py);
    state.set_item("baudrate", baudrate)?;
    state.set_item("rts", rts)?;
    state.set_item("dtr", dtr)?;
    state.set_item("break", break_on)?;
    Ok(state)
}

#[pyfunction]
fn delayed(value: &[u8], delay: f64) -> PyResult<OpFuture> {
    let delay = Duration::try_from_secs_f64(delay)
        .map_err(|err| PyValueError::new_err(format!("invalid delay: {err}")))?;
    let value = value.to_vec();
    Ok(OpFuture::spawn(async move {
        tokio::time::sleep(delay).await;
        Ok::<_, SerialError>(Outcome::Bytes(value))
    })?)
}

#[pyfunction]
#[allow(
    clippy::panic,
    reason = "drives the test of the guard that resolves panicked operations"
)]
fn panic_in_task() -> PyResult<OpFuture> {
    Ok(OpFuture::spawn(async { panic!("panic_in_task") })?)
}

#[pyfunction]
#[allow(
    clippy::panic,
    reason = "drives the test of a blocking call that panics before it hands off"
)]
fn panic_in_call(py: Python<'_>) -> PyResult<Py<PyAny>> {
    Serial::run(py, async { panic!("panic_in_call") })
}

/// Whether the extension was compiled without optimisations.
#[pyfunction]
fn debug_build() -> bool {
    cfg!(debug_assertions)
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(mock_pair, m)?)?;
    m.add_function(wrap_pyfunction!(mock_block_writes, m)?)?;
    m.add_function(wrap_pyfunction!(mock_unplug, m)?)?;
    m.add_function(wrap_pyfunction!(mock_state, m)?)?;
    m.add_function(wrap_pyfunction!(delayed, m)?)?;
    m.add_function(wrap_pyfunction!(panic_in_task, m)?)?;
    m.add_function(wrap_pyfunction!(panic_in_call, m)?)?;
    m.add_function(wrap_pyfunction!(debug_build, m)?)?;
    Ok(())
}
