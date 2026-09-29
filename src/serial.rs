use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyByteArray, PyBytes, PyDict, PyInt, PySlice, PyString, PyTuple};

use crate::errors::SerialError;
use crate::future::{OpFuture, Outcome};
use crate::port::PortCore;
use crate::settings::{self, Parity, Settings, StopBits};

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
        read_until(core, LF.to_vec(), usize::try_from(size).ok()).await
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

/// Copies `bytes`, `str` (as UTF-8) or anything `bytearray(data)` accepts.
pub(crate) fn to_bytes(data: &Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
    if let Ok(bytes) = data.cast::<PyBytes>() {
        return Ok(bytes.as_bytes().to_vec());
    }
    if let Ok(text) = data.cast::<PyString>() {
        return Ok(text.to_str()?.as_bytes().to_vec());
    }
    Ok(PyByteArray::from(data)?.to_vec())
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
                PyValueError::new_err(format!("Not a valid baudrate: {}", describe(value)))
            })?;
        Ok(Self(settings::baudrate(number)?))
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

    /// Validates every argument, then applies the settings and port and opens it when a port is given.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn init(
        &self,
        py: Python<'_>,
        port: Option<String>,
        baudrate: Baudrate,
        bytesize: Bytesize,
        parity: &str,
        stopbits: f64,
        timeout: Option<f64>,
        xonxoff: bool,
        rtscts: bool,
        write_timeout: Option<f64>,
        dsrdtr: Option<bool>,
        inter_byte_timeout: Option<f64>,
        exclusive: Option<bool>,
    ) -> PyResult<()> {
        let settings = Settings {
            baudrate: baudrate.0,
            bytesize: bytesize.0,
            parity: Parity::from_name(parity)?,
            stopbits: StopBits::from_value(stopbits)?,
            timeout: settings::seconds(timeout)?,
            write_timeout: settings::seconds(write_timeout)?,
            inter_byte_timeout: settings::seconds(inter_byte_timeout)?,
            xonxoff,
            rtscts,
            dsrdtr: dsrdtr.unwrap_or(rtscts),
            exclusive,
        };
        self.detached(py, move |core| {
            core.set_settings(settings)?;
            let open_now = port.is_some();
            core.set_port(port)?;
            if open_now && !core.is_open() {
                core.open()?;
            }
            Ok(())
        })
    }

    /// Opens the port when one is set and it is closed.
    pub(crate) fn enter(&self, py: Python<'_>) -> PyResult<()> {
        self.detached(py, |core| {
            if core.port().is_some() && !core.is_open() {
                core.open()
            } else {
                Ok(())
            }
        })
    }

    fn detached<R: Send>(
        &self,
        py: Python<'_>,
        f: impl FnOnce(&PortCore) -> Result<R, SerialError> + Send,
    ) -> PyResult<R> {
        Ok(py.detach(|| f(&self.core))?)
    }

    fn update(&self, py: Python<'_>, change: impl FnOnce(&mut Settings) + Send) -> PyResult<()> {
        self.detached(py, |core| {
            let mut settings = core.settings();
            change(&mut settings);
            core.set_settings(settings)
        })
    }
}

#[pymethods]
impl SerialBase {
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
    fn timeout(&self) -> Option<f64> {
        self.core.settings().timeout
    }

    #[setter]
    fn set_timeout(&self, py: Python<'_>, value: Option<f64>) -> PyResult<()> {
        let value = settings::seconds(value)?;
        self.update(py, move |s| s.timeout = value)
    }

    #[getter]
    fn write_timeout(&self) -> Option<f64> {
        self.core.settings().write_timeout
    }

    #[setter]
    fn set_write_timeout(&self, py: Python<'_>, value: Option<f64>) -> PyResult<()> {
        let value = settings::seconds(value)?;
        self.update(py, move |s| s.write_timeout = value)
    }

    #[getter]
    fn inter_byte_timeout(&self) -> Option<f64> {
        self.core.settings().inter_byte_timeout
    }

    #[setter]
    fn set_inter_byte_timeout(&self, py: Python<'_>, value: Option<f64>) -> PyResult<()> {
        let value = settings::seconds(value)?;
        self.update(py, move |s| s.inter_byte_timeout = value)
    }

    #[getter]
    fn xonxoff(&self) -> bool {
        self.core.settings().xonxoff
    }

    #[setter]
    fn set_xonxoff(&self, py: Python<'_>, value: bool) -> PyResult<()> {
        self.update(py, move |s| s.xonxoff = value)
    }

    #[getter]
    fn rtscts(&self) -> bool {
        self.core.settings().rtscts
    }

    #[setter]
    fn set_rtscts(&self, py: Python<'_>, value: bool) -> PyResult<()> {
        self.update(py, move |s| s.rtscts = value)
    }

    #[getter]
    fn dsrdtr(&self) -> bool {
        self.core.settings().dsrdtr
    }

    #[setter]
    fn set_dsrdtr(&self, py: Python<'_>, value: Option<bool>) -> PyResult<()> {
        let value = value.unwrap_or_else(|| self.core.settings().rtscts);
        self.update(py, move |s| s.dsrdtr = value)
    }

    #[getter]
    fn exclusive(&self) -> Option<bool> {
        self.core.settings().exclusive
    }

    // ponytail: takes effect at the next open; pyserial also re-locks an open POSIX port, add that if someone toggles it while open
    #[setter]
    fn set_exclusive(&self, py: Python<'_>, value: Option<bool>) -> PyResult<()> {
        self.update(py, move |s| s.exclusive = value)
    }

    #[getter]
    fn rts(&self) -> bool {
        self.core.rts()
    }

    #[setter]
    fn set_rts(&self, py: Python<'_>, value: bool) -> PyResult<()> {
        self.detached(py, |core| core.set_rts(value))
    }

    #[getter]
    fn dtr(&self) -> bool {
        self.core.dtr()
    }

    #[setter]
    fn set_dtr(&self, py: Python<'_>, value: bool) -> PyResult<()> {
        self.detached(py, |core| core.set_dtr(value))
    }

    #[getter]
    fn break_condition(&self) -> bool {
        self.core.break_condition()
    }

    #[setter]
    fn set_break_condition(&self, py: Python<'_>, value: bool) -> PyResult<()> {
        self.detached(py, |core| core.set_break_condition(value))
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
        let mut s = self.core.settings();
        if let Some(v) = fetch("baudrate")? {
            s.baudrate = v.extract::<Baudrate>()?.0;
        }
        if let Some(v) = fetch("bytesize")? {
            s.bytesize = v.extract::<Bytesize>()?.0;
        }
        if let Some(v) = fetch("parity")? {
            s.parity = Parity::from_name(&v.extract::<String>()?)?;
        }
        if let Some(v) = fetch("stopbits")? {
            s.stopbits = StopBits::from_value(v.extract()?)?;
        }
        if let Some(v) = fetch("xonxoff")? {
            s.xonxoff = v.extract()?;
        }
        if let Some(v) = fetch("dsrdtr")? {
            s.dsrdtr = v.extract::<Option<bool>>()?.unwrap_or(s.rtscts);
        }
        if let Some(v) = fetch("rtscts")? {
            s.rtscts = v.extract()?;
        }
        if let Some(v) = fetch("timeout")? {
            s.timeout = settings::seconds(v.extract()?)?;
        }
        if let Some(v) = fetch("write_timeout")? {
            s.write_timeout = settings::seconds(v.extract()?)?;
        }
        if let Some(v) = fetch("inter_byte_timeout")? {
            s.inter_byte_timeout = settings::seconds(v.extract()?)?;
        }
        self.detached(py, move |core| core.set_settings(s))
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        true
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
}

/// Serial port with pyserial's blocking API.
#[pyclass(module = "oxiserial", extends = SerialBase, subclass, frozen)]
pub struct Serial;

impl Serial {
    fn core(slf: &Bound<'_, Self>) -> Arc<PortCore> {
        Arc::clone(&slf.as_super().get().core)
    }

    fn run<F>(py: Python<'_>, op: F) -> PyResult<Py<PyAny>>
    where
        F: Future<Output = Result<Outcome, SerialError>> + Send + 'static,
    {
        OpFuture::spawn(op)?.block(py)
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
            stopbits = 1.0, timeout = None, xonxoff = false, rtscts = false,
            write_timeout = None, dsrdtr = Some(false), inter_byte_timeout = None, exclusive = None
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
        timeout: Option<f64>,
        xonxoff: bool,
        rtscts: bool,
        write_timeout: Option<f64>,
        dsrdtr: Option<bool>,
        inter_byte_timeout: Option<f64>,
        exclusive: Option<bool>,
    ) -> PyResult<()> {
        slf.as_super().get().init(
            slf.py(),
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

    #[pyo3(signature = (expected = LF, size = None), text_signature = "(self, /, expected=b'\\n', size=None)")]
    fn read_until(
        slf: &Bound<'_, Self>,
        expected: &[u8],
        size: Option<usize>,
    ) -> PyResult<Py<PyAny>> {
        Self::run(
            slf.py(),
            ops::read_until(Self::core(slf), expected.to_vec(), size),
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

    fn __enter__<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, Self>> {
        slf.as_super().get().enter(slf.py())?;
        Ok(slf.clone())
    }

    #[pyo3(signature = (*_args))]
    fn __exit__(slf: &Bound<'_, Self>, _args: &Bound<'_, PyTuple>) {
        let core = Self::core(slf);
        slf.py().detach(|| core.close());
    }

    fn __iter__<'py>(slf: &Bound<'py, Self>) -> Bound<'py, Self> {
        slf.clone()
    }

    fn __next__(slf: &Bound<'_, Self>) -> PyResult<Option<Py<PyAny>>> {
        let line = Self::run(slf.py(), ops::readline(Self::core(slf), -1))?;
        Ok((line.bind(slf.py()).len()? > 0).then_some(line))
    }
}
