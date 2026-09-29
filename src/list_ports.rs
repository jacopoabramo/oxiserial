use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;

use pyo3::exceptions::{PyIndexError, PyTypeError};
use pyo3::prelude::*;
use pyo3::types::PyList;
use serialport::Location;
use tokio_serial::{SerialPortInfo, SerialPortType};

use crate::errors::SerialError;

/// Description of one serial port, with pyserial's attributes.
#[pyclass(
    module = "oxiserial.tools.list_ports",
    subclass,
    dict,
    get_all,
    set_all
)]
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

/// Bus number pyserial shows on Windows: the last `USBROOT(n)` index plus one.
fn windows_bus_number(bus_id: &str) -> Option<u32> {
    // Matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt): USBROOT(n) is shown as bus n + 1.
    const MARKER: &str = "USBROOT(";
    let digits: String = bus_id[bus_id.rfind(MARKER)? + MARKER.len()..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse::<u32>().ok().map(|n| n.saturating_add(1))
}

fn format_location(location: &Location) -> String {
    let chain: Vec<String> = location.port_chain().iter().map(u8::to_string).collect();
    let bus = windows_bus_number(location.bus_id())
        .map_or_else(|| location.bus_id().to_owned(), |n| n.to_string());
    format!("{bus}-{}", chain.join("."))
}

/// Natural-sort key: each digit run is one integer, each other run its UTF-8 bytes.
fn natural_key(text: &str) -> Vec<Vec<u128>> {
    // Matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt): port ordering follows numsplit.
    let mut key = Vec::new();
    let mut rest = text;
    while let Some(first) = rest.chars().next() {
        let digits = first.is_ascii_digit();
        let end = rest
            .find(|c: char| c.is_ascii_digit() != digits)
            .unwrap_or(rest.len());
        let (run, tail) = rest.split_at(end);
        rest = tail;
        match run.parse::<u128>() {
            Ok(n) if digits => key.push(vec![n]),
            _ => key.push(run.bytes().map(u128::from).collect()),
        }
    }
    key
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
        // Matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt): description format.
        match (&self.product, &self.interface) {
            (product, Some(interface)) => {
                format!("{} - {interface}", product.as_deref().unwrap_or("None"))
            }
            (Some(product), None) => product.clone(),
            (None, None) => self.name.clone(),
        }
    }

    fn usb_info(&self) -> String {
        // Matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt): hwid format.
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

    fn __lt__(&self, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        match other.extract::<PyRef<'_, Self>>() {
            Ok(other) => Ok(natural_key(&self.device) < natural_key(&other.device)),
            Err(_) => Err(PyTypeError::new_err(format!(
                "unorderable types: ListPortInfo() and {}()",
                other.get_type().name()?
            ))),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_bus_id_becomes_usbroot_index_plus_one() {
        let location = Location::new("PCIROOT(0)#PCI(1400)#USBROOT(0)".into(), vec![2, 1]);
        assert_eq!(format_location(&location), "1-2.1");
    }

    #[test]
    fn numeric_bus_id_is_kept() {
        let location = Location::new("3".into(), vec![1, 4]);
        assert_eq!(format_location(&location), "3-1.4");
    }

    #[test]
    fn natural_key_orders_digit_runs_numerically() {
        assert!(natural_key("COM2") < natural_key("COM10"));
        assert!(natural_key("COM1") < natural_key("COM2"));
    }
}
