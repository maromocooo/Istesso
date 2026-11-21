//! Checks on what an adapter declares, before any of it is used.
//!
//! A declaration is text written by whoever wrote the adapter. Some of it
//! ends up in places where a stray character matters: lists shown to the
//! user, and files made for the system. So every declared string is held to a
//! plain set of characters and a length, here, once.

use istesso_device_api::Declaration;

/// Longest name of a device, a piece of software or an application.
const NAME_LENGTH: usize = 64;
/// Linux keeps at most 15 bytes of a process name in `/proc/<pid>/comm`.
/// A longer declared name can never match.
const LINUX_PROCESS_LENGTH: usize = 15;
/// Longest note on what a report is for.
const PURPOSE_LENGTH: usize = 80;

/// Letters, digits, spaces and a few marks. No control characters, no line
/// breaks, no quotes, no backslash, no `#`.
fn plain(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            ' ' | '+' | '-' | '_' | '.' | ',' | '(' | ')' | '&' | '/' | ':'
        )
}

/// Whether `text` may be shown or written out as it is.
pub fn text_ok(text: &str, longest: usize) -> bool {
    !text.is_empty() && text.len() <= longest && text.chars().all(plain) && text.trim() == text
}

fn check_text(what: &str, text: &str, longest: usize) -> Result<(), String> {
    if text_ok(text, longest) {
        Ok(())
    } else {
        // The text itself is not repeated: it is the thing that is not
        // trusted.
        Err(format!(
            "{what}: use 1 to {longest} letters, digits, spaces and + - _ . , ( ) & / : only"
        ))
    }
}

/// What is wrong with the strings of a declaration, if anything.
pub fn check(declaration: &Declaration) -> Result<(), String> {
    check_text("the name of the device", declaration.name, NAME_LENGTH)?;
    for rule in declaration.reports {
        check_text("the purpose of a report", rule.purpose, PURPOSE_LENGTH)?;
    }
    for software in declaration.other_software {
        check_text("the name of other software", software.name, NAME_LENGTH)?;
        if let Some(bundle) = software.macos_bundle_id {
            check_text("a bundle identifier", bundle, NAME_LENGTH)?;
        }
        if let Some(process) = software.linux_process {
            if process.len() > LINUX_PROCESS_LENGTH {
                return Err(format!(
                    "a process name: at most {LINUX_PROCESS_LENGTH} bytes, as Linux shows it in \
                     /proc/<pid>/comm (the first {LINUX_PROCESS_LENGTH} bytes of a longer name)"
                ));
            }
            check_text("a process name", process, LINUX_PROCESS_LENGTH)?;
        }
    }
    for app in declaration.used_by_apps {
        check_text("the name of an application", app, NAME_LENGTH)?;
    }
    for system in declaration.confirmation.systems {
        check_text("a system the adapter was run on", system, NAME_LENGTH)?;
    }
    let date = declaration.confirmation.date;
    if !date.is_empty() {
        let shaped = date.len() == 10
            && date.chars().enumerate().all(|(i, c)| {
                if i == 4 || i == 7 {
                    c == '-'
                } else {
                    c.is_ascii_digit()
                }
            });
        if !shaped {
            return Err("the date of the confirmation: write it as YYYY-MM-DD".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use istesso_device_api::{Confirmation, ConfirmedBy, ReportKind, ReportRule, Software, UsbId};

    const fn declaration(name: &'static str) -> Declaration {
        Declaration {
            name,
            devices: &[UsbId {
                vendor: 0xffff,
                product: 0x0001,
            }],
            reports: &[],
            other_software: &[],
            used_by_apps: &[],
            confirmation: Confirmation {
                by: ConfirmedBy::NotYet,
                systems: &[],
                date: "",
            },
        }
    }

    #[test]
    fn plain_names_pass() {
        assert!(check(&declaration("Stream Deck +")).is_ok());
        assert!(check(&declaration("Pad (2nd model), v1.2")).is_ok());
    }

    #[test]
    fn a_name_with_a_line_break_is_refused() {
        let error = check(&declaration("Pad\nKERNEL==\"*\", RUN+=\"/bin/sh\"")).unwrap_err();
        assert!(
            !error.contains("RUN"),
            "the untrusted text is not repeated: {error}"
        );
        assert!(check(&declaration("Pad\r")).is_err());
        assert!(check(&declaration("Pad\u{0}")).is_err());
        assert!(check(&declaration("Pad\u{1b}[31m")).is_err());
    }

    #[test]
    fn quotes_marks_and_overlong_names_are_refused() {
        assert!(check(&declaration("")).is_err());
        assert!(check(&declaration(" Pad")).is_err());
        assert!(check(&declaration("Pad \"x\"")).is_err());
        assert!(check(&declaration("Pad # comment")).is_err());
        assert!(check(&declaration("Pad\\n")).is_err());
        assert!(check(&declaration("パッド")).is_err());
        assert!(check(&declaration(
            "0123456789012345678901234567890123456789012345678901234567890123456789"
        ))
        .is_err());
    }

    #[test]
    fn a_linux_process_name_must_fit_in_15_bytes() {
        let mut d = declaration("Pad");
        d.other_software = &[Software {
            name: "Tool",
            macos_bundle_id: None,
            linux_process: Some("123456789012345"),
        }];
        assert!(check(&d).is_ok());

        d.other_software = &[Software {
            name: "Tool",
            macos_bundle_id: None,
            linux_process: Some("1234567890123456"),
        }];
        assert_eq!(
            check(&d).unwrap_err(),
            "a process name: at most 15 bytes, as Linux shows it in /proc/<pid>/comm \
             (the first 15 bytes of a longer name)"
        );
    }

    #[test]
    fn every_other_declared_string_is_held_to_the_same_set() {
        let mut d = declaration("Pad");
        d.reports = &[ReportRule {
            kind: ReportKind::Input,
            id: 1,
            command: None,
            purpose: "buttons\nRUN",
        }];
        assert!(check(&d).is_err());

        let mut d = declaration("Pad");
        d.other_software = &[Software {
            name: "Vendor\tTool",
            macos_bundle_id: None,
            linux_process: None,
        }];
        assert!(check(&d).is_err());

        let mut d = declaration("Pad");
        d.other_software = &[Software {
            name: "Tool",
            macos_bundle_id: Some("com.example\n"),
            linux_process: None,
        }];
        assert!(check(&d).is_err());

        let mut d = declaration("Pad");
        d.other_software = &[Software {
            name: "Tool",
            macos_bundle_id: None,
            linux_process: Some("a b\nc"),
        }];
        assert!(check(&d).is_err());

        let mut d = declaration("Pad");
        d.used_by_apps = &["Editor\n"];
        assert!(check(&d).is_err());

        let mut d = declaration("Pad");
        d.confirmation = Confirmation {
            by: ConfirmedBy::Author,
            systems: &["macOS 26"],
            date: "2026-11-17\n",
        };
        assert!(check(&d).is_err());
        d.confirmation = Confirmation {
            by: ConfirmedBy::Author,
            systems: &["macOS\n26"],
            date: "2026-11-17",
        };
        assert!(check(&d).is_err());
        d.confirmation = Confirmation {
            by: ConfirmedBy::Author,
            systems: &["macOS 26"],
            date: "2026-11-17",
        };
        assert!(check(&d).is_ok());
    }
}
