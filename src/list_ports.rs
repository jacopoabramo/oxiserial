use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;

use pyo3::exceptions::PyIndexError;
use pyo3::prelude::*;
use pyo3::types::PyList;
use serialport::Location;
use tokio_serial::{SerialPortInfo, SerialPortType};

use crate::errors::SerialError;

/// Description of one serial port, with pyserial's attributes.
#[pyclass(module = "oxiserial.tools.list_ports", get_all, set_all)]
pub struct ListPortInfo {
    device: String,
    name: String,
    description: String,
    hwid: String,
    vid: Option<u16>,
    pid: Option<u16>,
    serial_number: Option<String>,
    location: Option<String>,
    manufacturer: Option<String>,
    product: Option<String>,
    interface: Option<String>,
}

fn format_location(location: &Location) -> String {
    let chain: Vec<String> = location.port_chain().iter().map(u8::to_string).collect();
    format!("{}-{}", location.bus_id(), chain.join("."))
}

impl ListPortInfo {
    fn from_info(info: SerialPortInfo) -> Self {
        let mut port = Self::new(info.port_name, false);
        if let SerialPortType::UsbPort(usb) = info.port_type {
            port.vid = Some(usb.vid);
            port.pid = Some(usb.pid);
            port.serial_number = usb.serial_number;
            port.manufacturer = usb.manufacturer;
            port.product = usb.product;
            port.location = usb.location.as_ref().map(format_location);
            port.apply_usb_info();
        }
        port
    }
}

#[pymethods]
impl ListPortInfo {
    #[new]
    #[pyo3(signature = (device, skip_link_detection = false))]
    fn new(device: String, skip_link_detection: bool) -> Self {
        let _ = skip_link_detection;
        let name = Path::new(&device)
            .file_name()
            .map_or_else(|| device.clone(), |n| n.to_string_lossy().into_owned());
        Self {
            device,
            name,
            description: "n/a".into(),
            hwid: "n/a".into(),
            vid: None,
            pid: None,
            serial_number: None,
            location: None,
            manufacturer: None,
            product: None,
            interface: None,
        }
    }

    fn usb_description(&self) -> String {
        match (&self.product, &self.interface) {
            (product, Some(interface)) => {
                format!("{} - {interface}", product.as_deref().unwrap_or("None"))
            }
            (Some(product), None) => product.clone(),
            (None, None) => self.name.clone(),
        }
    }

    fn usb_info(&self) -> String {
        let serial = self
            .serial_number
            .as_ref()
            .map(|s| format!(" SER={s}"))
            .unwrap_or_default();
        let location = self
            .location
            .as_ref()
            .map(|l| format!(" LOCATION={l}"))
            .unwrap_or_default();
        format!(
            "USB VID:PID={:04X}:{:04X}{serial}{location}",
            self.vid.unwrap_or(0),
            self.pid.unwrap_or(0)
        )
    }

    fn apply_usb_info(&mut self) {
        self.description = self.usb_description();
        self.hwid = self.usb_info();
    }

    fn __getitem__(&self, index: isize) -> PyResult<String> {
        match index {
            0 => Ok(self.device.clone()),
            1 => Ok(self.description.clone()),
            2 => Ok(self.hwid.clone()),
            _ => Err(PyIndexError::new_err("list index out of range")),
        }
    }

    fn __iter__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        PyList::new(py, [&self.device, &self.description, &self.hwid])?
            .try_iter()
            .map(Bound::into_any)
    }

    fn __str__(&self) -> String {
        format!("{} - {}", self.device, self.description)
    }

    fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
        other
            .extract::<PyRef<'_, Self>>()
            .is_ok_and(|o| o.device == self.device)
    }

    fn __hash__(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.device.hash(&mut hasher);
        hasher.finish()
    }
}

// ponytail: include_links is ignored, so /dev/serial/by-id symlinks are not listed; add when a user needs them
#[pyfunction]
#[pyo3(signature = (include_links = false))]
pub fn comports(py: Python<'_>, include_links: bool) -> PyResult<Vec<ListPortInfo>> {
    let _ = include_links;
    let ports = py
        .detach(tokio_serial::available_ports)
        .map_err(SerialError::from)?;
    Ok(ports.into_iter().map(ListPortInfo::from_info).collect())
}
