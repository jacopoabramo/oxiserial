use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{
    PyByteArray, PyBytes, PyDict, PyInt, PyMemoryView, PySlice, PyString, PyTuple, PyType,
};

use crate::errors::SerialError;
use crate::future::{OpFuture, Outcome, WAIT_SLICE};
use crate::port::PortCore;
use crate::runtime::runtime;
use crate::settings::{self, Parity, Seconds, Settings, StopBits};

pub(crate) const LF: &[u8] = b"\n";

/// Operation builders shared by the sync and async classes.
pub(crate) mod ops {
    use super::*;

    type Op = Result<Outcome, SerialError>;

    pub async fn read(core: Arc<PortCore>, size: usize) -> Op {
        core.read(size).await.map(Outcome::Bytes)
    }

    pub async fn read_until(core: Arc<PortCore>, expected: Vec<u8>, size: Option<usize>) -> Op {
        core.read_until(&expected, size).await.map(Outcome::Bytes)
    }

    /// A negative size means no limit, as in `io.IOBase.readline`.
    pub async fn readline(core: Arc<PortCore>, size: isize) -> Op {
        core.readline(usize::try_from(size).ok())
            .await
            .map(Outcome::Bytes)
    }

    pub async fn readlines(core: Arc<PortCore>, hint: isize) -> Op {
        let hint = usize::try_from(hint).ok().filter(|&h| h > 0);
        core.readlines(hint).await.map(Outcome::Lines)
    }

    pub async fn write(core: Arc<PortCore>, data: Vec<u8>) -> Op {
        core.write(&data).await.map(Outcome::Int)
    }

    pub async fn flush(core: Arc<PortCore>) -> Op {
        core.flush().await.map(|()| Outcome::Unit)
    }

    pub async fn send_break(core: Arc<PortCore>, seconds: f64) -> Op {
        if seconds.is_nan() || seconds < 0.0 {
            return Err(SerialError::Value(
                "duration must be a non-negative number".into(),
            ));
        }
        let duration = Duration::try_from_secs_f64(seconds).unwrap_or(Duration::MAX);
        core.send_break(duration).await.map(|()| Outcome::Unit)
    }
}

/// The `expected` argument of `read_until`, which defaults to a newline.
pub(crate) fn expected_bytes(expected: Option<&Bound<'_, PyAny>>) -> PyResult<Vec<u8>> {
    expected.map_or_else(|| Ok(LF.to_vec()), to_bytes)
}

/// Copies `bytes`, `str` (as UTF-8) or the raw bytes of an object with the buffer protocol.
///
/// An `int` or an iterable of ints raises `TypeError`, although `bytearray()` accepts them.
pub(crate) fn to_bytes(data: &Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
    if let Ok(bytes) = data.cast::<PyBytes>() {
        return Ok(bytes.as_bytes().to_vec());
    }
    if let Ok(text) = data.cast::<PyString>() {
        return Ok(text.to_str()?.as_bytes().to_vec());
    }
    if let Ok(array) = data.cast::<PyByteArray>() {
        return Ok(array.to_vec());
    }
    // memoryview() accepts exactly the objects with the buffer protocol.
    let view = PyMemoryView::from(data).map_err(|err| {
        if err.is_instance_of::<PyTypeError>(data.py()) {
            let name = data
                .get_type()
                .name()
                .map_or_else(|_| "?".into(), |name| name.to_string());
            PyTypeError::new_err(format!(
                "a bytes-like object or str is required, not '{name}'"
            ))
        } else {
            err
        }
    })?;
    Ok(PyByteArray::from(&view)?.to_vec())
}

fn describe(value: &Bound<'_, PyAny>) -> String {
    value
        .repr()
        .map_or_else(|_| "<unprintable>".into(), |text| text.to_string())
}

/// A baudrate given as anything `int()` accepts.
pub(crate) struct Baudrate(pub u32);

impl<'a, 'py> FromPyObject<'a, 'py> for Baudrate {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        let value: &Bound<'py, PyAny> = &obj;
        let number = value
            .py()
            .get_type::<PyInt>()
            .call1((value,))
            .and_then(|int| int.extract::<i64>())
            .map_err(|_| {
                // Matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt): message.
                PyValueError::new_err(format!("Not a valid baudrate: {}", describe(value)))
            })?;
        Ok(Self(settings::baudrate(number)?))
    }
}

// Matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt): SerialBase.BAUDRATES.
const STANDARD_BAUDRATES: [u32; 30] = [
    50, 75, 110, 134, 150, 200, 300, 600, 1200, 1800, 2400, 4800, 9600, 19200, 38400, 57600,
    115200, 230400, 460800, 500000, 576000, 921600, 1000000, 1152000, 1500000, 2000000, 2500000,
    3000000, 3500000, 4000000,
];

/// The `oxiserial.Baudrate` IntEnum of pyserial's standard rates, built on first use.
pub(crate) fn baudrate_enum(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static ENUM: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
    ENUM.get_or_try_init(py, || {
        let members: Vec<(String, u32)> = STANDARD_BAUDRATES
            .iter()
            .map(|&rate| (format!("B{rate}"), rate))
            .collect();
        let kwargs = PyDict::new(py);
        kwargs.set_item("module", "oxiserial")?;
        Ok::<_, PyErr>(
            py.import("enum")?
                .getattr("IntEnum")?
                .call(("Baudrate", members), Some(&kwargs))?
                .unbind(),
        )
    })
    .map(|members| members.bind(py))
}

/// A flag given as any object and read with Python truthiness.
pub(crate) struct Truthy(pub bool);

impl<'a, 'py> FromPyObject<'a, 'py> for Truthy {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        Ok(Self(obj.is_truthy()?))
    }
}

impl<'a, 'py> FromPyObject<'a, 'py> for Seconds {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        let value: &Bound<'py, PyAny> = &obj;
        if value.is_instance_of::<PyInt>()
            && let Ok(int) = value.extract::<i64>()
        {
            return Ok(Seconds::Int(int));
        }
        value.extract::<f64>().map(Seconds::Float).map_err(|_| {
            // Matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt): message.
            PyValueError::new_err(format!("Not a valid timeout: {}", describe(value)))
        })
    }
}

impl<'py> IntoPyObject<'py> for Seconds {
    type Target = PyAny;
    type Output = Bound<'py, PyAny>;
    type Error = std::convert::Infallible;

    fn into_pyobject(self, py: Python<'py>) -> Result<Self::Output, Self::Error> {
        Ok(match self {
            Seconds::Int(value) => value.into_pyobject(py)?.into_any(),
            Seconds::Float(value) => value.into_pyobject(py)?.into_any(),
        })
    }
}

/// A byte size given as a number equal to 5, 6, 7 or 8.
pub(crate) struct Bytesize(pub u8);

impl<'a, 'py> FromPyObject<'a, 'py> for Bytesize {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        let value: &Bound<'py, PyAny> = &obj;
        let number = value
            .extract::<f64>()
            .ok()
            .filter(|number| number.fract() == 0.0)
            .ok_or_else(|| {
                // Matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt): message.
                PyValueError::new_err(format!("Not a valid byte size: {}", describe(value)))
            })?;
        Ok(Self(settings::bytesize(number as i64)?))
    }
}

fn stopbits_object<'py>(py: Python<'py>, stopbits: StopBits) -> PyResult<Bound<'py, PyAny>> {
    Ok(match stopbits {
        StopBits::One => 1_i32.into_pyobject(py)?.into_any(),
        StopBits::OnePointFive => 1.5_f64.into_pyobject(py)?.into_any(),
        StopBits::Two => 2_i32.into_pyobject(py)?.into_any(),
    })
}

/// Creates `cls(None, *args, **kwargs)`, sets its port to `url` and opens it unless `do_not_open`.
// Matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt): serial_for_url.
pub(crate) fn for_url<'py>(
    cls: &Bound<'py, PyType>,
    url: &Bound<'py, PyAny>,
    args: &Bound<'py, PyTuple>,
    do_not_open: Truthy,
    kwargs: Option<&Bound<'py, PyDict>>,
) -> PyResult<Bound<'py, PyAny>> {
    let py = cls.py();
    if let Ok(text) = url.cast::<PyString>()
        && let Some((protocol, _)) = text.to_str()?.to_lowercase().split_once("://")
        && protocol != "loop"
    {
        return Err(PyValueError::new_err(format!(
            "invalid URL, protocol {} not known",
            PyString::new(py, protocol).repr()?
        )));
    }
    let mut call_args = vec![py.None().into_bound(py)];
    call_args.extend(args.iter());
    let instance = cls.call(PyTuple::new(py, call_args)?, kwargs)?;
    instance.setattr("port", url)?;
    if !do_not_open.0 {
        instance.call_method0("open")?;
    }
    Ok(instance)
}

/// Returns a `Serial` for `url`, which is a device name or `loop://`.
#[pyfunction]
#[pyo3(
    signature = (url, *args, do_not_open = Truthy(false), **kwargs),
    text_signature = "(url, *args, do_not_open=False, **kwargs)"
)]
pub(crate) fn serial_for_url<'py>(
    url: &Bound<'py, PyAny>,
    args: &Bound<'py, PyTuple>,
    do_not_open: Truthy,
    kwargs: Option<&Bound<'py, PyDict>>,
) -> PyResult<Bound<'py, PyAny>> {
    for_url(
        &url.py().get_type::<Serial>(),
        url,
        args,
        do_not_open,
        kwargs,
    )
}

/// Properties and non-I/O methods shared by `oxiserial.Serial` and `oxiserial.aio.Serial`.
#[pyclass(module = "oxiserial", subclass, frozen)]
pub struct SerialBase {
    pub(crate) core: Arc<PortCore>,
}

impl SerialBase {
    pub(crate) fn unopened() -> Self {
        Self {
            core: Arc::new(PortCore::new(None, Settings::default())),
        }
    }

    /// Validates every argument, applies the settings and port, and calls `open()` when a port is given.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn init(
        slf: &Bound<'_, Self>,
        port: Option<String>,
        baudrate: Baudrate,
        bytesize: Bytesize,
        parity: &str,
        stopbits: f64,
        timeout: Option<Seconds>,
        xonxoff: Truthy,
        rtscts: Truthy,
        write_timeout: Option<Seconds>,
        dsrdtr: Option<Truthy>,
        inter_byte_timeout: Option<Seconds>,
        exclusive: Option<Truthy>,
    ) -> PyResult<()> {
        let settings = Settings {
            baudrate: baudrate.0,
            bytesize: bytesize.0,
            parity: Parity::from_name(parity)?,
            stopbits: StopBits::from_value(stopbits)?,
            timeout: settings::seconds(timeout)?,
            write_timeout: settings::seconds(write_timeout)?,
            inter_byte_timeout: settings::seconds(inter_byte_timeout)?,
            xonxoff: xonxoff.0,
            rtscts: rtscts.0,
            dsrdtr: dsrdtr.map_or(rtscts.0, |d| d.0),
            exclusive: exclusive.map(|e| e.0),
        };
        let open_now = port.is_some();
        slf.get().detached(slf.py(), move |core| {
            core.set_settings(settings)?;
            core.set_port(port)
        })?;
        if open_now && !slf.get().core.is_open() {
            slf.call_method0("open")?;
        }
        Ok(())
    }

    /// Calls `open()` when a port is set and it is closed.
    pub(crate) fn enter(slf: &Bound<'_, Self>) -> PyResult<()> {
        let core = &slf.get().core;
        if core.port().is_some() && !core.is_open() {
            slf.call_method0("open")?;
        }
        Ok(())
    }

    fn detached<R: Send>(
        &self,
        py: Python<'_>,
        f: impl FnOnce(&PortCore) -> Result<R, SerialError> + Send,
    ) -> PyResult<R> {
        Ok(py.detach(|| f(&self.core))?)
    }

    fn update(&self, py: Python<'_>, change: impl FnOnce(&mut Settings) + Send) -> PyResult<()> {
        self.detached(py, |core| core.update_settings(change))
    }
}

impl Drop for SerialBase {
    fn drop(&mut self) {
        // A pending aio operation holds the core; closing here keeps it from holding the port open.
        self.core.close();
    }
}

#[pymethods]
impl SerialBase {
    fn cancel_read(&self) {
        self.core.interrupt_read();
    }

    // Matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt): Windows-only, and tx_size
    // defaults to rx_size.
    #[cfg(windows)]
    #[pyo3(signature = (rx_size = 4096, tx_size = None))]
    fn set_buffer_size(&self, py: Python<'_>, rx_size: u32, tx_size: Option<u32>) -> PyResult<()> {
        self.detached(py, |core| {
            core.set_buffer_size(rx_size, tx_size.unwrap_or(rx_size))
        })
    }

    fn cancel_write(&self) {
        self.core.interrupt_write();
    }

    #[getter]
    fn port(&self) -> Option<String> {
        self.core.port()
    }

    #[setter]
    fn set_port(&self, py: Python<'_>, value: Option<String>) -> PyResult<()> {
        self.detached(py, |core| core.set_port(value))
    }

    #[getter]
    fn name(&self) -> Option<String> {
        self.core.port()
    }

    #[getter]
    fn is_open(&self, py: Python<'_>) -> bool {
        py.detach(|| self.core.is_open())
    }

    #[getter]
    fn baudrate(&self) -> u32 {
        self.core.settings().baudrate
    }

    #[setter]
    fn set_baudrate(&self, py: Python<'_>, value: Baudrate) -> PyResult<()> {
        let value = value.0;
        self.update(py, move |s| s.baudrate = value)
    }

    #[getter]
    fn bytesize(&self) -> u8 {
        self.core.settings().bytesize
    }

    #[setter]
    fn set_bytesize(&self, py: Python<'_>, value: Bytesize) -> PyResult<()> {
        let value = value.0;
        self.update(py, move |s| s.bytesize = value)
    }

    #[getter]
    fn parity(&self) -> &'static str {
        self.core.settings().parity.name()
    }

    #[setter]
    fn set_parity(&self, py: Python<'_>, value: &str) -> PyResult<()> {
        let value = Parity::from_name(value)?;
        self.update(py, move |s| s.parity = value)
    }

    #[getter]
    fn stopbits<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        stopbits_object(py, self.core.settings().stopbits)
    }

    #[setter]
    fn set_stopbits(&self, py: Python<'_>, value: f64) -> PyResult<()> {
        let value = StopBits::from_value(value)?;
        self.update(py, move |s| s.stopbits = value)
    }

    #[getter]
    fn timeout(&self) -> Option<Seconds> {
        self.core.settings().timeout
    }

    #[setter]
    fn set_timeout(&self, py: Python<'_>, value: Option<Seconds>) -> PyResult<()> {
        let value = settings::seconds(value)?;
        self.update(py, move |s| s.timeout = value)
    }

    #[getter]
    fn write_timeout(&self) -> Option<Seconds> {
        self.core.settings().write_timeout
    }

    #[setter]
    fn set_write_timeout(&self, py: Python<'_>, value: Option<Seconds>) -> PyResult<()> {
        let value = settings::seconds(value)?;
        self.update(py, move |s| s.write_timeout = value)
    }

    #[getter]
    fn inter_byte_timeout(&self) -> Option<Seconds> {
        self.core.settings().inter_byte_timeout
    }

    #[setter]
    fn set_inter_byte_timeout(&self, py: Python<'_>, value: Option<Seconds>) -> PyResult<()> {
        let value = settings::seconds(value)?;
        self.update(py, move |s| s.inter_byte_timeout = value)
    }

    #[getter]
    fn xonxoff(&self) -> bool {
        self.core.settings().xonxoff
    }

    #[setter]
    fn set_xonxoff(&self, py: Python<'_>, value: Truthy) -> PyResult<()> {
        self.update(py, move |s| s.xonxoff = value.0)
    }

    #[getter]
    fn rtscts(&self) -> bool {
        self.core.settings().rtscts
    }

    #[setter]
    fn set_rtscts(&self, py: Python<'_>, value: Truthy) -> PyResult<()> {
        self.update(py, move |s| s.rtscts = value.0)
    }

    #[getter]
    fn dsrdtr(&self) -> bool {
        self.core.settings().dsrdtr
    }

    #[setter]
    fn set_dsrdtr(&self, py: Python<'_>, value: Option<Truthy>) -> PyResult<()> {
        self.update(py, move |s| s.dsrdtr = value.map_or(s.rtscts, |v| v.0))
    }

    #[getter]
    fn exclusive(&self) -> Option<bool> {
        self.core.settings().exclusive
    }

    // ponytail: takes effect at the next open; pyserial also re-locks an open POSIX port, add that if someone toggles it while open
    #[setter]
    fn set_exclusive(&self, py: Python<'_>, value: Option<Truthy>) -> PyResult<()> {
        self.update(py, move |s| s.exclusive = value.map(|v| v.0))
    }

    #[getter]
    fn rts(&self) -> bool {
        self.core.rts()
    }

    #[setter]
    fn set_rts(&self, py: Python<'_>, value: Truthy) -> PyResult<()> {
        self.detached(py, |core| core.set_rts(value.0))
    }

    #[getter]
    fn dtr(&self) -> bool {
        self.core.dtr()
    }

    #[setter]
    fn set_dtr(&self, py: Python<'_>, value: Truthy) -> PyResult<()> {
        self.detached(py, |core| core.set_dtr(value.0))
    }

    #[getter]
    fn break_condition(&self) -> bool {
        self.core.break_condition()
    }

    #[setter]
    fn set_break_condition(&self, py: Python<'_>, value: Truthy) -> PyResult<()> {
        self.detached(py, |core| core.set_break_condition(value.0))
    }

    #[getter]
    fn cts(&self, py: Python<'_>) -> PyResult<bool> {
        self.detached(py, PortCore::cts)
    }

    #[getter]
    fn dsr(&self, py: Python<'_>) -> PyResult<bool> {
        self.detached(py, PortCore::dsr)
    }

    #[getter]
    fn ri(&self, py: Python<'_>) -> PyResult<bool> {
        self.detached(py, PortCore::ri)
    }

    #[getter]
    fn cd(&self, py: Python<'_>) -> PyResult<bool> {
        self.detached(py, PortCore::cd)
    }

    #[getter]
    fn in_waiting(&self, py: Python<'_>) -> PyResult<usize> {
        self.detached(py, PortCore::in_waiting)
    }

    #[getter]
    fn out_waiting(&self, py: Python<'_>) -> PyResult<usize> {
        self.detached(py, PortCore::out_waiting)
    }

    fn open(&self, py: Python<'_>) -> PyResult<()> {
        self.detached(py, PortCore::open)
    }

    fn close(&self, py: Python<'_>) {
        py.detach(|| self.core.close());
    }

    fn reset_input_buffer(&self, py: Python<'_>) -> PyResult<()> {
        self.detached(py, PortCore::reset_input_buffer)
    }

    fn reset_output_buffer(&self, py: Python<'_>) -> PyResult<()> {
        self.detached(py, PortCore::reset_output_buffer)
    }

    fn get_settings<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let s = self.core.settings();
        let d = PyDict::new(py);
        d.set_item("baudrate", s.baudrate)?;
        d.set_item("bytesize", s.bytesize)?;
        d.set_item("parity", s.parity.name())?;
        d.set_item("stopbits", stopbits_object(py, s.stopbits)?)?;
        d.set_item("xonxoff", s.xonxoff)?;
        d.set_item("dsrdtr", s.dsrdtr)?;
        d.set_item("rtscts", s.rtscts)?;
        d.set_item("timeout", s.timeout)?;
        d.set_item("write_timeout", s.write_timeout)?;
        d.set_item("inter_byte_timeout", s.inter_byte_timeout)?;
        Ok(d)
    }

    fn apply_settings(&self, py: Python<'_>, d: &Bound<'_, PyAny>) -> PyResult<()> {
        let missing = py.import("builtins")?.getattr("object")?.call0()?;
        let fetch = |key: &str| -> PyResult<Option<Bound<'_, PyAny>>> {
            let value = d.call_method1("get", (key, &missing))?;
            Ok((!value.is(&missing)).then_some(value))
        };
        let baudrate = fetch("baudrate")?
            .map(|v| v.extract::<Baudrate>())
            .transpose()?;
        let bytesize = fetch("bytesize")?
            .map(|v| v.extract::<Bytesize>())
            .transpose()?;
        let parity = fetch("parity")?
            .map(|v| Ok::<_, PyErr>(Parity::from_name(&v.extract::<String>()?)?))
            .transpose()?;
        let stopbits = fetch("stopbits")?
            .map(|v| Ok::<_, PyErr>(StopBits::from_value(v.extract()?)?))
            .transpose()?;
        let xonxoff = fetch("xonxoff")?
            .map(|v| v.extract::<Truthy>())
            .transpose()?;
        let dsrdtr = fetch("dsrdtr")?
            .map(|v| v.extract::<Option<Truthy>>())
            .transpose()?;
        let rtscts = fetch("rtscts")?
            .map(|v| v.extract::<Truthy>())
            .transpose()?;
        let timeout = |key| -> PyResult<Option<Option<Seconds>>> {
            fetch(key)?
                .map(|v| Ok(settings::seconds(v.extract()?)?))
                .transpose()
        };
        let (timeout, write_timeout, inter_byte_timeout) = (
            timeout("timeout")?,
            timeout("write_timeout")?,
            timeout("inter_byte_timeout")?,
        );
        self.update(py, move |s| {
            // Matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt): key order, so a None
            // dsrdtr follows the rtscts value from before this call.
            if let Some(v) = baudrate {
                s.baudrate = v.0;
            }
            if let Some(v) = bytesize {
                s.bytesize = v.0;
            }
            if let Some(v) = parity {
                s.parity = v;
            }
            if let Some(v) = stopbits {
                s.stopbits = v;
            }
            if let Some(v) = xonxoff {
                s.xonxoff = v.0;
            }
            if let Some(v) = dsrdtr {
                s.dsrdtr = v.map_or(s.rtscts, |v| v.0);
            }
            if let Some(v) = rtscts {
                s.rtscts = v.0;
            }
            if let Some(v) = timeout {
                s.timeout = v;
            }
            if let Some(v) = write_timeout {
                s.write_timeout = v;
            }
            if let Some(v) = inter_byte_timeout {
                s.inter_byte_timeout = v;
            }
        })
    }

    fn fileno(&self, py: Python<'_>) -> PyResult<i32> {
        match self.detached(py, PortCore::fileno)? {
            Some(fd) => Ok(fd),
            None => Err(PyErr::from_value(
                py.import("io")?
                    .getattr("UnsupportedOperation")?
                    .call1(("fileno",))?,
            )),
        }
    }

    #[getter]
    fn portstr(&self) -> Option<String> {
        self.core.port()
    }

    #[classattr]
    #[pyo3(name = "BAUDRATES")]
    fn baudrates(py: Python<'_>) -> PyResult<Py<PyAny>> {
        Ok(py
            .get_type::<PyTuple>()
            .call1((baudrate_enum(py)?,))?
            .unbind())
    }

    #[classattr]
    #[pyo3(name = "BYTESIZES")]
    fn bytesizes() -> (u8, u8, u8, u8) {
        (5, 6, 7, 8)
    }

    #[classattr]
    #[pyo3(name = "PARITIES")]
    fn parities() -> (
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
    ) {
        ("N", "E", "O", "M", "S")
    }

    #[classattr]
    #[pyo3(name = "STOPBITS")]
    fn stopbits_values() -> (i32, f64, i32) {
        (1, 1.5, 2)
    }

    fn __repr__(slf: &Bound<'_, Self>) -> PyResult<String> {
        // Matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt): __repr__ format.
        const FORMAT: &str = "{name}<id=0x{id:x}, open={p.is_open}>(port={p.portstr!r}, \
            baudrate={p.baudrate!r}, bytesize={p.bytesize!r}, parity={p.parity!r}, \
            stopbits={p.stopbits!r}, timeout={p.timeout!r}, xonxoff={p.xonxoff!r}, \
            rtscts={p.rtscts!r}, dsrdtr={p.dsrdtr!r})";
        let py = slf.py();
        let fields = PyDict::new(py);
        fields.set_item("name", slf.get_type().name()?)?;
        fields.set_item("id", slf.as_ptr() as usize)?;
        fields.set_item("p", slf)?;
        PyString::new(py, FORMAT)
            .call_method("format", (), Some(&fields))?
            .extract()
    }
}

/// Serial port with pyserial's blocking API.
#[pyclass(module = "oxiserial", extends = SerialBase, subclass, frozen)]
pub struct Serial;

impl Serial {
    fn core(slf: &Bound<'_, Self>) -> Arc<PortCore> {
        Arc::clone(&slf.as_super().get().core)
    }

    pub(crate) fn run<F>(py: Python<'_>, op: F) -> PyResult<Py<PyAny>>
    where
        F: Future<Output = Result<Outcome, SerialError>> + Send + 'static,
    {
        // On an oxiserial worker, such as one running a done-callback, this hands the worker's
        // queued tasks to another thread before blocking; elsewhere it calls the closure directly.
        tokio::task::block_in_place(|| Self::drive(py, op))
    }

    /// Runs `op` on this thread for one wait slice, where a ready result or a short wait
    /// finishes, then hands it to a worker.
    fn drive<F>(py: Python<'_>, op: F) -> PyResult<Py<PyAny>>
    where
        F: Future<Output = Result<Outcome, SerialError>> + Send + 'static,
    {
        let runtime = runtime()?;
        let mut op = Box::pin(op);
        let first = py.detach(|| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                runtime.block_on(async { tokio::time::timeout(WAIT_SLICE, &mut op).await })
            }))
        });
        match first {
            Ok(Ok(result)) => result.map_err(PyErr::from)?.to_py(py),
            // On a worker the operation keeps going while a signal handler runs here, so the
            // handler can use the port; kept on this thread, it would hold the port's turn.
            Ok(Err(_)) => OpFuture::spawn(op)?.block(py),
            // Reported as a panic after the hand-off is; the drop guards run without the GIL.
            Err(_) => {
                py.detach(|| drop(op));
                Err(SerialError::panicked().into())
            }
        }
    }
}

#[pymethods]
impl Serial {
    #[new]
    #[pyo3(signature = (*_args, **_kwargs))]
    fn new(
        _args: &Bound<'_, PyTuple>,
        _kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyClassInitializer<Self> {
        PyClassInitializer::from(SerialBase::unopened()).add_subclass(Self)
    }

    #[pyo3(
        signature = (
            port = None, baudrate = Baudrate(9600), bytesize = Bytesize(8), parity = "N",
            stopbits = 1.0, timeout = None, xonxoff = Truthy(false), rtscts = Truthy(false),
            write_timeout = None, dsrdtr = Some(Truthy(false)), inter_byte_timeout = None, exclusive = None
        ),
    )]
    #[allow(clippy::too_many_arguments)]
    fn __init__(
        slf: &Bound<'_, Self>,
        port: Option<String>,
        baudrate: Baudrate,
        bytesize: Bytesize,
        parity: &str,
        stopbits: f64,
        timeout: Option<Seconds>,
        xonxoff: Truthy,
        rtscts: Truthy,
        write_timeout: Option<Seconds>,
        dsrdtr: Option<Truthy>,
        inter_byte_timeout: Option<Seconds>,
        exclusive: Option<Truthy>,
    ) -> PyResult<()> {
        SerialBase::init(
            slf.as_super(),
            port,
            baudrate,
            bytesize,
            parity,
            stopbits,
            timeout,
            xonxoff,
            rtscts,
            write_timeout,
            dsrdtr,
            inter_byte_timeout,
            exclusive,
        )
    }

    #[pyo3(signature = (size = 1))]
    fn read(slf: &Bound<'_, Self>, size: usize) -> PyResult<Py<PyAny>> {
        Self::run(slf.py(), ops::read(Self::core(slf), size))
    }

    #[pyo3(signature = (expected = None, size = None), text_signature = "(self, /, expected=b'\\n', size=None)")]
    fn read_until(
        slf: &Bound<'_, Self>,
        expected: Option<&Bound<'_, PyAny>>,
        size: Option<usize>,
    ) -> PyResult<Py<PyAny>> {
        Self::run(
            slf.py(),
            ops::read_until(Self::core(slf), expected_bytes(expected)?, size),
        )
    }

    #[pyo3(signature = (size = -1), text_signature = "(self, /, size=-1)")]
    fn readline(slf: &Bound<'_, Self>, size: isize) -> PyResult<Py<PyAny>> {
        Self::run(slf.py(), ops::readline(Self::core(slf), size))
    }

    #[pyo3(signature = (hint = -1), text_signature = "(self, /, hint=-1)")]
    fn readlines(slf: &Bound<'_, Self>, hint: isize) -> PyResult<Py<PyAny>> {
        Self::run(slf.py(), ops::readlines(Self::core(slf), hint))
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        true
    }

    fn seekable(&self) -> bool {
        false
    }

    #[getter]
    fn closed(slf: &Bound<'_, Self>) -> bool {
        let core = Self::core(slf);
        !slf.py().detach(|| core.is_open())
    }

    fn readinto(slf: &Bound<'_, Self>, b: &Bound<'_, PyAny>) -> PyResult<usize> {
        let py = slf.py();
        let data = Self::run(py, ops::read(Self::core(slf), b.len()?))?;
        let data = data.bind(py);
        let n = data.len()?;
        b.set_item(PySlice::new(py, 0, n as isize, 1), data)?;
        Ok(n)
    }

    fn write(slf: &Bound<'_, Self>, data: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        Self::run(slf.py(), ops::write(Self::core(slf), to_bytes(data)?))
    }

    fn flush(slf: &Bound<'_, Self>) -> PyResult<()> {
        Self::run(slf.py(), ops::flush(Self::core(slf))).map(drop)
    }

    #[pyo3(signature = (duration = 0.25))]
    fn send_break(slf: &Bound<'_, Self>, duration: f64) -> PyResult<()> {
        Self::run(slf.py(), ops::send_break(Self::core(slf), duration)).map(drop)
    }

    fn read_all<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyAny>> {
        slf.call_method1("read", (slf.getattr("in_waiting")?,))
    }

    fn __enter__<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, Self>> {
        SerialBase::enter(slf.as_super())?;
        Ok(slf.clone())
    }

    #[pyo3(signature = (*_args))]
    fn __exit__(slf: &Bound<'_, Self>, _args: &Bound<'_, PyTuple>) -> PyResult<()> {
        slf.call_method0("close")?;
        Ok(())
    }

    fn __iter__<'py>(slf: &Bound<'py, Self>) -> Bound<'py, Self> {
        slf.clone()
    }

    fn __next__(slf: &Bound<'_, Self>) -> PyResult<Option<Py<PyAny>>> {
        let line = Self::run(slf.py(), ops::readline(Self::core(slf), -1))?;
        Ok((line.bind(slf.py()).len()? > 0).then_some(line))
    }
}
