//! The udev rules that let the agent open the devices of the built-in
//! adapters on Linux.
//!
//! The rules are made from the declarations, never written by hand: an
//! adapter names its device by two numbers, and the framework derives the
//! rule (ADR 0012). The right is given to a group. `uaccess` alone would only
//! reach whoever is logged in at the screen, and a host usually has nobody
//! there (ADR 0004).
//!
//! The file is installed by root, and udev rules can start programs. So no
//! text from a declaration goes into it: only the vendor and product numbers,
//! written out as four hex digits each, and the name of the group, which is
//! checked. The names of devices stay out, even as comments.

use istesso_device_api::AdapterEntry;

/// First line of the rules. The only line that is not a rule.
const HEADER: &str =
    "# Made by Istesso from the declarations of its device adapters. Do not edit by hand.";

/// One rule per device of every adapter in `entries`, for the group `group`.
///
/// The result has exactly one line of header and one line per device.
pub fn rules(entries: &[(&'static str, AdapterEntry)], group: &str) -> Result<String, String> {
    if !valid_group(group) {
        return Err("the group: use 1 to 32 letters, digits, `_` and `-` only".into());
    }
    let mut text = String::from(HEADER);
    text.push('\n');
    for (_, entry) in entries {
        for id in entry.declaration.devices {
            let (vendor, product): (u16, u16) = (id.vendor, id.product);
            text.push_str(&format!(
                "KERNEL==\"hidraw*\", SUBSYSTEM==\"hidraw\", ATTRS{{idVendor}}==\"{vendor:04x}\", ATTRS{{idProduct}}==\"{product:04x}\", MODE=\"0660\", GROUP=\"{group}\", TAG+=\"uaccess\"\n"
            ));
        }
    }
    Ok(text)
}

/// Whether `group` can be written into a rule as it is.
pub fn valid_group(group: &str) -> bool {
    !group.is_empty()
        && group.len() <= 32
        && group
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use istesso_device_api::{
        Adapter, AdapterError, Command, Confirmation, ConfirmedBy, Context, Declaration,
        ReportKind, ReportRule, Software, UsbId,
    };

    #[test]
    fn group_names_are_checked() {
        assert!(valid_group("plugdev"));
        assert!(valid_group("istesso-devices"));
        assert!(!valid_group(""));
        assert!(!valid_group("a\", RUN+=\"/bin/sh"));
        assert!(!valid_group("plugdev\nKERNEL==\"*\""));
        assert!(rules(&[], "plug dev").is_err());
        assert!(rules(&[], "plugdev\n").is_err());
    }

    #[test]
    fn every_device_of_every_adapter_gets_one_rule_and_nothing_more() {
        let entries = crate::registry::entries();
        let text = rules(&entries, "plugdev").unwrap();
        let devices: usize = entries
            .iter()
            .map(|(_, e)| e.declaration.devices.len())
            .sum();
        assert_eq!(text.lines().count(), 1 + devices);
        assert!(text
            .lines()
            .skip(1)
            .all(|l| l.starts_with("KERNEL==") && l.contains("GROUP=\"plugdev\"")));
    }

    struct Silent;

    impl Adapter for Silent {
        fn connect(&mut self, _: &mut Context<'_>) -> Result<(), AdapterError> {
            Ok(())
        }
        fn input(&mut self, _: &[u8], _: &mut Context<'_>) -> Result<(), AdapterError> {
            Ok(())
        }
        fn command(&mut self, _: Command<'_>, _: &mut Context<'_>) -> Result<(), AdapterError> {
            Ok(())
        }
    }

    /// Every string of this declaration tries to add a line of its own.
    static HOSTILE: Declaration = Declaration {
        name: "Pad\nKERNEL==\"*\", RUN+=\"/bin/sh -c 'id > /tmp/x'\"",
        devices: &[
            UsbId {
                vendor: 0x1,
                product: 0xabcd,
            },
            UsbId {
                vendor: 0xffff,
                product: 0x0,
            },
        ],
        reports: &[ReportRule {
            kind: ReportKind::Input,
            id: 1,
            command: None,
            purpose: "x\nRUN+=\"/bin/sh\"",
        }],
        other_software: &[Software {
            name: "Tool\nRUN+=\"/bin/sh\"",
            macos_bundle_id: Some("a\nRUN+=\"/bin/sh\""),
            linux_process: Some("b\nRUN+=\"/bin/sh\""),
        }],
        used_by_apps: &["App\nRUN+=\"/bin/sh\""],
        confirmation: Confirmation {
            by: ConfirmedBy::NotYet,
            systems: &["x\nRUN+=\"/bin/sh\""],
            date: "",
        },
    };

    #[test]
    fn a_hostile_declaration_cannot_add_a_line_to_the_rules() {
        let entry = AdapterEntry {
            declaration: &HOSTILE,
            create: || Box::new(Silent),
        };
        // The registry would have refused it…
        assert!(crate::declared::check(&HOSTILE).is_err());
        // …and even handed over directly, it gets its two numbers and no more.
        let text = rules(&[("hostile", entry)], "plugdev").unwrap();
        assert_eq!(text.lines().count(), 1 + 2, "{text}");
        assert!(!text.contains("RUN"), "{text}");
        assert!(!text.contains("Pad"), "{text}");
        let rule_lines: Vec<&str> = text.lines().skip(1).collect();
        assert!(
            rule_lines[0].contains("ATTRS{idVendor}==\"0001\", ATTRS{idProduct}==\"abcd\""),
            "{text}"
        );
        assert!(
            rule_lines[1].contains("ATTRS{idVendor}==\"ffff\", ATTRS{idProduct}==\"0000\""),
            "{text}"
        );
    }
}
