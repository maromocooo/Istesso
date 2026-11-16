//! The adapters that were built in.
//!
//! They live in the crate `istesso-device-adapters`, which collects them from
//! its directories. Nothing here names an adapter.

use istesso_device_api::{AdapterEntry, UsbId};

/// Every adapter, with the name of its directory.
pub fn entries() -> Vec<(&'static str, AdapterEntry)> {
    istesso_device_adapters::entries()
}

/// The adapter for a device, if there is one.
pub fn find(id: UsbId) -> Option<(&'static str, AdapterEntry)> {
    entries()
        .into_iter()
        .find(|(_, entry)| entry.declaration.devices.contains(&id))
}

/// The adapter that lives in the directory `name`.
pub fn by_name(name: &str) -> Option<AdapterEntry> {
    entries()
        .into_iter()
        .find(|(n, _)| *n == name)
        .map(|(_, entry)| entry)
}

/// What is wrong with the declarations, if anything: a string with characters
/// that do not belong in a name, a device claimed by two adapters, a write
/// without a command byte, an adapter without a device.
pub fn check() -> Result<(), String> {
    let all = entries();
    for (index, (name, entry)) in all.iter().enumerate() {
        let declaration = entry.declaration;
        crate::declared::check(declaration)
            .map_err(|error| format!("adapter `{name}`: {error}"))?;
        if declaration.devices.is_empty() {
            return Err(format!("adapter `{name}` names no device"));
        }
        crate::guard::check_rules(declaration.reports)
            .map_err(|error| format!("adapter `{name}`: {error}"))?;
        for (other_name, other) in &all[index + 1..] {
            if let Some(id) = declaration
                .devices
                .iter()
                .find(|id| other.declaration.devices.contains(id))
            {
                return Err(format!(
                    "adapters `{name}` and `{other_name}` both claim device {:04x}:{:04x}",
                    id.vendor, id.product
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_declarations_of_all_adapters_are_sound() {
        check().unwrap();
    }

    #[test]
    fn an_unknown_device_has_no_adapter() {
        assert!(find(UsbId {
            vendor: 0xffff,
            product: 0xffff
        })
        .is_none());
        assert!(by_name("no_such_adapter").is_none());
    }
}
