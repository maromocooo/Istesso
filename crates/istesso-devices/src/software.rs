//! Whether other software that uses a device is running.
//!
//! Which software that is comes from the adapter's declaration; the core
//! knows no product by name. While such software runs, Istesso does not touch
//! the device: both would draw on the same buttons and both would react to
//! the same press.

use std::process::Command;

use istesso_device_api::Software;

/// `Some(true)` when the software runs on this machine, `Some(false)` when it
/// does not, `None` when it cannot be told (the software does not exist for
/// this system, or the system's tool gave no answer).
pub fn is_running(software: &Software) -> Option<bool> {
    if cfg!(target_os = "macos") {
        let bundle = software.macos_bundle_id?;
        // Prints one line per running copy and nothing when there is none.
        let output = Command::new("/usr/bin/lsappinfo")
            .args(["find", &format!("bundleID={bundle}")])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(parse_lsappinfo_find(&String::from_utf8_lossy(
            &output.stdout,
        )))
    } else if cfg!(target_os = "linux") {
        let name = software.linux_process?;
        let entries = std::fs::read_dir("/proc").ok()?;
        Some(entries.flatten().any(|entry| {
            std::fs::read_to_string(entry.path().join("comm"))
                .is_ok_and(|comm| comm.trim_end() == name)
        }))
    } else {
        None
    }
}

/// The names of the declared software that is running now.
pub fn running(all: &[Software]) -> Vec<&'static str> {
    all.iter()
        .filter(|software| is_running(software) == Some(true))
        .map(|software| software.name)
        .collect()
}

/// `lsappinfo find` answers with lines such as `ASN:0x0-0x1234-"Name":`.
fn parse_lsappinfo_find(text: &str) -> bool {
    text.lines()
        .any(|line| line.trim_start().starts_with("ASN:"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_answer_of_lsappinfo() {
        assert!(parse_lsappinfo_find("ASN:0x0-0x13076063-\"Some_App\":\n"));
        assert!(!parse_lsappinfo_find("\n"));
        assert!(!parse_lsappinfo_find(""));
    }

    #[test]
    fn software_that_does_not_exist_here_cannot_be_told() {
        let nowhere = Software {
            name: "Nowhere",
            macos_bundle_id: None,
            linux_process: None,
        };
        assert_eq!(is_running(&nowhere), None);
        assert!(running(&[nowhere]).is_empty());
    }

    #[test]
    fn software_that_is_not_running_is_reported_as_such() {
        let absent = Software {
            name: "Absent",
            macos_bundle_id: Some("invalid.istesso.test.not-installed"),
            linux_process: Some("istesso-test-not-running"),
        };
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            assert_eq!(is_running(&absent), Some(false));
        }
    }
}
