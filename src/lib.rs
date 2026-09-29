use std::sync::{Mutex, MutexGuard, PoisonError};

use pyo3::prelude::*;

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

    let aio_module = PyModule::new(py, "oxiserial.aio")?;
    add_submodule(m, "aio", &aio_module)?;

    let tools_module = PyModule::new(py, "oxiserial.tools")?;
    let list_ports_module = PyModule::new(py, "oxiserial.tools.list_ports")?;
    add_submodule(&tools_module, "list_ports", &list_ports_module)?;
    add_submodule(m, "tools", &tools_module)?;
    Ok(())
}
