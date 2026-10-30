use std::collections::HashMap;
use std::ptr;

use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    DICS_FLAG_GLOBAL, DIGCF_PRESENT, DIREG_DEV, HDEVINFO, SETUP_DI_REGISTRY_PROPERTY,
    SP_DEVINFO_DATA, SPDRP_FRIENDLYNAME, SPDRP_HARDWAREID, SPDRP_MFG, SetupDiClassGuidsFromNameW,
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
    SetupDiGetDeviceInstanceIdW, SetupDiGetDeviceRegistryPropertyW, SetupDiOpenDevRegKey,
};
use windows_sys::Win32::Foundation::{ERROR_SUCCESS, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Registry::{KEY_READ, RegCloseKey, RegQueryValueExW};
use windows_sys::core::GUID;

use crate::list_ports::PortDetails;

/// pyserial's buffer length, above Windows' limit of 200 characters for a device instance id.
const TEXT_LENGTH: usize = 250;

/// Destroys a device information set when dropped.
struct DeviceList(HDEVINFO);

impl Drop for DeviceList {
    fn drop(&mut self) {
        // SAFETY: the set came from SetupDiGetClassDevsW and is destroyed once.
        unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

fn from_wide(text: &[u16]) -> String {
    let end = text.iter().position(|&c| c == 0).unwrap_or(text.len());
    String::from_utf16_lossy(&text[..end])
}

fn class_guids(name: &str) -> Vec<GUID> {
    let mut guids = [GUID::default(); 8];
    let mut count = 0;
    // SAFETY: the name is nul-terminated and the list holds `guids.len()` GUIDs.
    let ok = unsafe {
        SetupDiClassGuidsFromNameW(
            wide(name).as_ptr(),
            guids.as_mut_ptr(),
            guids.len() as u32,
            &mut count,
        )
    };
    if ok == 0 {
        return Vec::new();
    }
    guids[..(count as usize).min(guids.len())].to_vec()
}

fn port_name(list: &DeviceList, device: &SP_DEVINFO_DATA) -> Option<String> {
    // SAFETY: `device` was filled by SetupDiEnumDeviceInfo from `list`.
    let key =
        unsafe { SetupDiOpenDevRegKey(list.0, device, DICS_FLAG_GLOBAL, 0, DIREG_DEV, KEY_READ) };
    if key == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut name = [0u16; TEXT_LENGTH];
    // Leaves room for the nul that a registry string may lack.
    let mut size = (size_of_val(&name) - 2) as u32;
    // SAFETY: the key is open, and `size` bytes fit in `name`.
    let status = unsafe {
        RegQueryValueExW(
            key,
            wide("PortName").as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            name.as_mut_ptr().cast(),
            &mut size,
        )
    };
    // SAFETY: the key is open and closed once.
    unsafe { RegCloseKey(key) };
    (status == ERROR_SUCCESS).then(|| from_wide(&name))
}

fn property(
    list: &DeviceList,
    device: &SP_DEVINFO_DATA,
    property: SETUP_DI_REGISTRY_PROPERTY,
) -> Option<String> {
    let mut text = [0u16; TEXT_LENGTH];
    // SAFETY: `device` belongs to `list`, and the size passed leaves the last nul in `text`.
    let ok = unsafe {
        SetupDiGetDeviceRegistryPropertyW(
            list.0,
            device,
            property,
            ptr::null_mut(),
            text.as_mut_ptr().cast(),
            (size_of_val(&text) - 2) as u32,
            ptr::null_mut(),
        )
    };
    (ok != 0).then(|| from_wide(&text))
}

fn instance_id(list: &DeviceList, device: &SP_DEVINFO_DATA) -> Option<String> {
    let mut id = [0u16; TEXT_LENGTH];
    // SAFETY: `device` belongs to `list`, and `id` holds `id.len()` characters.
    let ok = unsafe {
        SetupDiGetDeviceInstanceIdW(
            list.0,
            device,
            id.as_mut_ptr(),
            id.len() as u32,
            ptr::null_mut(),
        )
    };
    (ok != 0).then(|| from_wide(&id))
}

/// Device Manager's name, instance id and manufacturer of each present port, by port name.
pub fn port_details() -> HashMap<String, PortDetails> {
    // Matches pyserial (BSD-3-Clause, see LICENSES/pyserial.txt): device classes and fields.
    let mut details = HashMap::new();
    for guid in class_guids("Ports").into_iter().chain(class_guids("Modem")) {
        // SAFETY: `guid` is a valid class GUID; the other arguments may be null.
        let handle =
            unsafe { SetupDiGetClassDevsW(&guid, ptr::null(), ptr::null_mut(), DIGCF_PRESENT) };
        if handle == INVALID_HANDLE_VALUE as HDEVINFO {
            continue;
        }
        let list = DeviceList(handle);
        let mut device = SP_DEVINFO_DATA {
            cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
            ..Default::default()
        };
        let mut index = 0;
        // SAFETY: `device.cbSize` is set, as SetupDiEnumDeviceInfo requires.
        while unsafe { SetupDiEnumDeviceInfo(list.0, index, &mut device) } != 0 {
            index += 1;
            let Some(name) = port_name(&list, &device) else {
                continue;
            };
            if name.starts_with("LPT") {
                continue;
            }
            let hwid = instance_id(&list, &device)
                .or_else(|| property(&list, &device, SPDRP_HARDWAREID))
                .unwrap_or_default();
            details.insert(
                name,
                PortDetails {
                    description: property(&list, &device, SPDRP_FRIENDLYNAME),
                    hwid,
                    manufacturer: property(&list, &device, SPDRP_MFG),
                },
            );
        }
    }
    details
}
