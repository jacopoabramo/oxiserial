mod backend;
mod errors;
mod runtime;
mod settings;

use std::sync::{Mutex, MutexGuard, PoisonError};

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

/// Locks `mutex`, recovering the data if a panicking thread poisoned it.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Adds `child` as attribute `attr` of `parent` and registers it in `sys.modules`.
fn add_submodule(
    parent: &Bound<'_, PyModule>,
    attr: &str,
    child: &Bound<'_, PyModule>,
) -> PyResult<()> {
    parent.add(attr, child)?;
    parent
        .py()
        .import("sys")?
        .getattr("modules")?
        .set_item(child.name()?, child)
}

#[pymodule(gil_used = false)]
fn _oxiserial(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;

    m.add("PARITY_NONE", "N")?;
    m.add("PARITY_EVEN", "E")?;
    m.add("PARITY_ODD", "O")?;
    m.add("PARITY_MARK", "M")?;
    m.add("PARITY_SPACE", "S")?;
    let parity_names = PyDict::new(py);
    for (key, name) in [
        ("N", "None"),
        ("E", "Even"),
        ("O", "Odd"),
        ("M", "Mark"),
        ("S", "Space"),
    ] {
        parity_names.set_item(key, name)?;
    }
    m.add("PARITY_NAMES", parity_names)?;
    m.add("STOPBITS_ONE", 1)?;
    m.add("STOPBITS_ONE_POINT_FIVE", 1.5)?;
    m.add("STOPBITS_TWO", 2)?;
    m.add("FIVEBITS", 5)?;
    m.add("SIXBITS", 6)?;
    m.add("SEVENBITS", 7)?;
    m.add("EIGHTBITS", 8)?;
    m.add("XON", PyBytes::new(py, b"\x11"))?;
    m.add("XOFF", PyBytes::new(py, b"\x13"))?;
    m.add("CR", PyBytes::new(py, b"\r"))?;
    m.add("LF", PyBytes::new(py, b"\n"))?;
    m.add("SerialException", py.get_type::<errors::SerialException>())?;
    m.add(
        "SerialTimeoutException",
        py.get_type::<errors::SerialTimeoutException>(),
    )?;
    m.add(
        "PortNotOpenError",
        py.get_type::<errors::PortNotOpenError>(),
    )?;

    let aio_module = PyModule::new(py, "oxiserial.aio")?;
    add_submodule(m, "aio", &aio_module)?;

    let tools_module = PyModule::new(py, "oxiserial.tools")?;
    let list_ports_module = PyModule::new(py, "oxiserial.tools.list_ports")?;
    add_submodule(&tools_module, "list_ports", &list_ports_module)?;
    add_submodule(m, "tools", &tools_module)?;
    Ok(())
}
