use std::sync::Arc;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};

use crate::future::{OpFuture, Outcome};
use crate::port::PortCore;
use crate::serial::{Baudrate, Bytesize, SerialBase, Truthy, expected_bytes, ops, to_bytes};

/// Serial port whose I/O methods return futures.
#[pyclass(name = "Serial", module = "oxiserial.aio", extends = SerialBase, subclass, frozen)]
pub struct AioSerial;

impl AioSerial {
    fn core(slf: &Bound<'_, Self>) -> Arc<PortCore> {
        Arc::clone(&slf.as_super().get().core)
    }
}

#[pymethods]
impl AioSerial {
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
        timeout: Option<f64>,
        xonxoff: Truthy,
        rtscts: Truthy,
        write_timeout: Option<f64>,
        dsrdtr: Option<Truthy>,
        inter_byte_timeout: Option<f64>,
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
    fn read(slf: &Bound<'_, Self>, size: usize) -> PyResult<OpFuture> {
        Ok(OpFuture::spawn(ops::read(Self::core(slf), size))?)
    }

    #[pyo3(signature = (expected = None, size = None), text_signature = "(self, /, expected=b'\\n', size=None)")]
    fn read_until(
        slf: &Bound<'_, Self>,
        expected: Option<&Bound<'_, PyAny>>,
        size: Option<usize>,
    ) -> PyResult<OpFuture> {
        Ok(OpFuture::spawn(ops::read_until(
            Self::core(slf),
            expected_bytes(expected)?,
            size,
        ))?)
    }

    #[pyo3(signature = (size = -1), text_signature = "(self, /, size=-1)")]
    fn readline(slf: &Bound<'_, Self>, size: isize) -> PyResult<OpFuture> {
        Ok(OpFuture::spawn(ops::readline(Self::core(slf), size))?)
    }

    #[pyo3(signature = (hint = -1), text_signature = "(self, /, hint=-1)")]
    fn readlines(slf: &Bound<'_, Self>, hint: isize) -> PyResult<OpFuture> {
        Ok(OpFuture::spawn(ops::readlines(Self::core(slf), hint))?)
    }

    fn write(slf: &Bound<'_, Self>, data: &Bound<'_, PyAny>) -> PyResult<OpFuture> {
        Ok(OpFuture::spawn(ops::write(
            Self::core(slf),
            to_bytes(data)?,
        ))?)
    }

    fn flush(slf: &Bound<'_, Self>) -> PyResult<OpFuture> {
        Ok(OpFuture::spawn(ops::flush(Self::core(slf)))?)
    }

    #[pyo3(signature = (duration = 0.25))]
    fn send_break(slf: &Bound<'_, Self>, duration: f64) -> PyResult<OpFuture> {
        Ok(OpFuture::spawn(ops::send_break(Self::core(slf), duration))?)
    }

    fn read_all<'py>(slf: &Bound<'py, Self>) -> PyResult<Bound<'py, PyAny>> {
        slf.call_method1("read", (slf.getattr("in_waiting")?,))
    }

    #[pyo3(name = "sendBreak", signature = (duration = 0.25))]
    fn send_break_alias<'py>(slf: &Bound<'py, Self>, duration: f64) -> PyResult<Bound<'py, PyAny>> {
        slf.call_method1("send_break", (duration,))
    }

    fn __aenter__(slf: &Bound<'_, Self>) -> PyResult<OpFuture> {
        SerialBase::enter(slf.as_super())?;
        Ok(OpFuture::ready(Outcome::Object(Arc::new(
            slf.clone().into_any().unbind(),
        ))))
    }

    #[pyo3(signature = (*_args))]
    fn __aexit__(slf: &Bound<'_, Self>, _args: &Bound<'_, PyTuple>) -> PyResult<OpFuture> {
        slf.call_method0("close")?;
        Ok(OpFuture::ready(Outcome::Unit))
    }
}
