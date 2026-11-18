//! Real HID devices, through the `hidapi` crate.
//!
//! Only this module touches the operating system's HID interface. Serial
//! numbers are never read here: they are private, and no part of Istesso
//! needs them to find a device.

use std::ffi::CString;

use hidapi::{HidApi, HidDevice};
use istesso_device_api::{AdapterEntry, UsbId};

use crate::text::printable;
use crate::transport::Transport;

/// A device that is plugged in and has an adapter.
pub struct Found {
    /// Directory name of the adapter.
    pub adapter: &'static str,
    pub entry: AdapterEntry,
    pub id: UsbId,
    /// The name from the adapter's declaration. What the device calls itself
    /// is not used: it is text from outside, and it would be shown as it is.
    pub product: &'static str,
    path: CString,
}

/// The access to the system's HID devices. Keep it for as long as devices
/// are open.
pub struct Hid {
    api: HidApi,
}

impl Hid {
    pub fn new() -> Result<Self, String> {
        let api = HidApi::new().map_err(|error| {
            printable(&format!("the HID interface could not be opened: {error}"))
        })?;
        // Do not take the device away from other software. Whether Istesso
        // may talk to it at all is decided before opening (see `software`).
        #[cfg(target_os = "macos")]
        api.set_open_exclusive(false);
        Ok(Self { api })
    }

    /// The plugged-in devices that one of `entries` handles.
    pub fn find(&self, entries: &[(&'static str, AdapterEntry)]) -> Vec<Found> {
        let mut found: Vec<Found> = Vec::new();
        for info in self.api.device_list() {
            let id = UsbId {
                vendor: info.vendor_id(),
                product: info.product_id(),
            };
            let Some((adapter, entry)) = entries
                .iter()
                .find(|(_, e)| e.declaration.devices.contains(&id))
            else {
                continue;
            };
            let path = info.path().to_owned();
            if found.iter().any(|other| other.path == path) {
                continue;
            }
            found.push(Found {
                adapter,
                entry: *entry,
                id,
                product: entry.declaration.name,
                path,
            });
        }
        found
    }

    /// Open a device that [`Hid::find`] returned.
    pub fn open(&self, found: &Found) -> Result<HidTransport, String> {
        let device = self.api.open_path(&found.path).map_err(|error| {
            printable(&format!("{} could not be opened: {error}", found.product))
        })?;
        Ok(HidTransport { device })
    }
}

/// An open HID device.
pub struct HidTransport {
    device: HidDevice,
}

impl Transport for HidTransport {
    fn write(&mut self, report: &[u8]) -> Result<(), String> {
        self.device
            .write(report)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn send_feature(&mut self, report: &[u8]) -> Result<(), String> {
        self.device
            .send_feature_report(report)
            .map_err(|error| error.to_string())
    }

    fn get_feature(&mut self, id: u8, buffer: &mut [u8]) -> Result<usize, String> {
        let Some(first) = buffer.first_mut() else {
            return Ok(0);
        };
        *first = id;
        self.device
            .get_feature_report(buffer)
            .map_err(|error| error.to_string())
    }

    fn read(&mut self, buffer: &mut [u8], timeout_ms: u32) -> Result<usize, String> {
        let timeout = i32::try_from(timeout_ms).unwrap_or(i32::MAX);
        self.device
            .read_timeout(buffer, timeout)
            .map_err(|error| error.to_string())
    }
}
