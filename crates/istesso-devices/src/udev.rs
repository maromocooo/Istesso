//! The udev rules that let the agent open the devices of the built-in
//! adapters on Linux.
//!
//! The rules are made from the declarations, never written by hand: a
//! contributor's adapter names its device, and the framework derives the
//! rule (ADR 0012). The right is given to a group. `uaccess` alone would only
//! reach whoever is logged in at the screen, and a host usually has nobody
//! there (ADR 0004).

use istesso_device_api::AdapterEntry;

/// One rule per device of every adapter in `entries`, for the group `group`.
pub fn rules(entries: &[(&'static str, AdapterEntry)], group: &str) -> String {
    let mut text = String::from(
        "# Made by Istesso from the declarations of its device adapters. Do not edit by hand.\n",
    );
    for (_, entry) in entries {
        for id in entry.declaration.devices {
            text.push_str(&format!(
                "# {}\nKERNEL==\"hidraw*\", SUBSYSTEM==\"hidraw\", ATTRS{{idVendor}}==\"{:04x}\", ATTRS{{idProduct}}==\"{:04x}\", MODE=\"0660\", GROUP=\"{}\", TAG+=\"uaccess\"\n",
                entry.declaration.name, id.vendor, id.product, group
            ));
        }
    }
    text
}

/// Whether `group` can be written into a rule as it is.
pub fn valid_group(group: &str) -> bool {
    !group.is_empty()
        && group
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_names_are_checked() {
        assert!(valid_group("plugdev"));
        assert!(valid_group("istesso-devices"));
        assert!(!valid_group(""));
        assert!(!valid_group("a\", RUN+=\"/bin/sh"));
    }

    #[test]
    fn every_device_of_every_adapter_gets_a_rule() {
        let entries = crate::registry::entries();
        let text = rules(&entries, "plugdev");
        let devices: usize = entries
            .iter()
            .map(|(_, e)| e.declaration.devices.len())
            .sum();
        assert_eq!(
            text.lines()
                .filter(|line| line.starts_with("KERNEL=="))
                .count(),
            devices
        );
        assert!(text
            .lines()
            .filter(|l| l.starts_with("KERNEL=="))
            .all(|l| l.contains("GROUP=\"plugdev\"")));
    }
}
