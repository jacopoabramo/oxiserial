use std::time::Duration;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::backend::mock;
use crate::errors::SerialError;
use crate::future::{OpFuture, Outcome};

fn unknown(port: &str) -> PyErr {
    PyValueError::new_err(format!("unknown mock port '{port}'"))
}

#[pyfunction]
fn mock_pair() -> (String, String) {
    mock::pair()
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
    let value = value.to_vec();
    Ok(OpFuture::spawn(async move {
        tokio::time::sleep(Duration::from_secs_f64(delay)).await;
        Ok::<_, SerialError>(Outcome::Bytes(value))
    })?)
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(mock_pair, m)?)?;
    m.add_function(wrap_pyfunction!(mock_block_writes, m)?)?;
    m.add_function(wrap_pyfunction!(mock_state, m)?)?;
    m.add_function(wrap_pyfunction!(delayed, m)?)?;
    Ok(())
}
