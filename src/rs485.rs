use std::time::Duration;

use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyFloat};

use crate::settings::Rs485;

/// The Python view of an [`Rs485`] setting.
#[pyclass(name = "RS485", module = "oxiserial", frozen, eq, hash, from_py_object)]
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PyRs485(pub Rs485);

fn delay(field: &str, seconds: f64) -> PyResult<Duration> {
    Duration::try_from_secs_f64(seconds).map_err(|_| {
        // The error does not say which bound failed, so the value tells.
        if seconds.is_nan() || seconds < 0.0 {
            PyValueError::new_err(format!("{field} must be a non-negative number of seconds"))
        } else {
            PyValueError::new_err(format!("{field} is too large"))
        }
    })
}

fn python_bool(value: bool) -> &'static str {
    if value { "True" } else { "False" }
}

#[pymethods]
impl PyRs485 {
    #[new]
    #[pyo3(signature = (
        *, rts_while_tx = true, rts_while_rx = false, delay_before_tx = 0.0,
        delay_after_tx = 0.0, loopback = false
    ))]
    fn new(
        rts_while_tx: bool,
        rts_while_rx: bool,
        delay_before_tx: f64,
        delay_after_tx: f64,
        loopback: bool,
    ) -> PyResult<Self> {
        Ok(Self(Rs485 {
            rts_while_tx,
            rts_while_rx,
            delay_before_tx: delay("delay_before_tx", delay_before_tx)?,
            delay_after_tx: delay("delay_after_tx", delay_after_tx)?,
            loopback,
        }))
    }

    #[getter]
    fn rts_while_tx(&self) -> bool {
        self.0.rts_while_tx
    }

    #[getter]
    fn rts_while_rx(&self) -> bool {
        self.0.rts_while_rx
    }

    #[getter]
    fn delay_before_tx(&self) -> f64 {
        self.0.delay_before_tx.as_secs_f64()
    }

    #[getter]
    fn delay_after_tx(&self) -> f64 {
        self.0.delay_after_tx.as_secs_f64()
    }

    #[getter]
    fn loopback(&self) -> bool {
        self.0.loopback
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let seconds = |d: Duration| PyFloat::new(py, d.as_secs_f64()).repr();
        Ok(format!(
            "RS485(rts_while_tx={}, rts_while_rx={}, delay_before_tx={}, delay_after_tx={}, loopback={})",
            python_bool(self.0.rts_while_tx),
            python_bool(self.0.rts_while_rx),
            seconds(self.0.delay_before_tx)?,
            seconds(self.0.delay_after_tx)?,
            python_bool(self.0.loopback),
        ))
    }
}

/// A Python `bool | RS485` given for `rs485`; `None` is handled by the caller's `Option`.
pub struct Rs485Arg(pub Option<Rs485>);

impl<'a, 'py> FromPyObject<'a, 'py> for Rs485Arg {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        if let Ok(on) = obj.cast::<PyBool>() {
            return Ok(Self(on.is_true().then(Rs485::default)));
        }
        if let Ok(value) = obj.cast::<PyRs485>() {
            return Ok(Self(Some(value.get().0)));
        }
        Err(PyTypeError::new_err(
            "rs485 must be a bool, an RS485 or None",
        ))
    }
}
