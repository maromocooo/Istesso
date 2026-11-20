//! Whether other software that uses a device is running.
//!
//! Which software that is comes from the adapter's declaration; the core
//! knows no product by name. While such software runs, Istesso does not touch
//! the device: both would draw on the same buttons and both would react to
//! the same press. If Istesso cannot tell whether the software runs, it also
//! leaves the device alone.

use std::process::Command;

use istesso_device_api::Software;

/// What Istesso knows about one declared program on this machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// A copy runs on this machine.
    Runs,
    /// The system's tool answered and no copy runs.
    DoesNotRun,
    /// The declaration names nothing to look for on this system, so the program cannot run here.
    NotForThisSystem,
    /// The tool gave no answer or there is no way to check here. Istesso leaves the device alone.
    CannotTell,
}

/// Whether the declared software runs on this machine.
pub fn is_running(software: &Software) -> State {
    if cfg!(target_os = "macos") {
        let Some(bundle) = software.macos_bundle_id else {
            return State::NotForThisSystem;
        };
        // Prints one line per running copy and nothing when there is none.
        let Ok(output) = Command::new("/usr/bin/lsappinfo")
            .args(["find", &format!("bundleID={bundle}")])
            .output()
        else {
            return State::CannotTell;
        };
        if !output.status.success() {
            return State::CannotTell;
        }
        if parse_lsappinfo_find(&String::from_utf8_lossy(&output.stdout)) {
            State::Runs
        } else {
            State::DoesNotRun
        }
    } else if cfg!(target_os = "linux") {
        let Some(name) = software.linux_process else {
            return State::NotForThisSystem;
        };
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return State::CannotTell;
        };
        let mut state = State::DoesNotRun;
        for entry in entries {
            let Ok(entry) = entry else {
                state = State::CannotTell;
                continue;
            };
            if std::fs::read_to_string(entry.path().join("comm"))
                .is_ok_and(|comm| comm.trim_end() == name)
            {
                return State::Runs;
            }
        }
        state
    } else {
        State::CannotTell
    }
}

/// Declared software that keeps Istesso from touching the device.
#[derive(Default)]
pub struct Look {
    /// Names of programs running on this machine.
    pub running: Vec<&'static str>,
    /// Names of programs whose running state is unknown.
    pub cannot_tell: Vec<&'static str>,
}

/// Lists the declared programs that run here or whose running state is unknown.
pub fn look(all: &[Software]) -> Look {
    look_with(all, is_running)
}

fn look_with(all: &[Software], lookup: impl Fn(&Software) -> State) -> Look {
    let mut result = Look::default();
    for software in all {
        match lookup(software) {
            State::Runs => result.running.push(software.name),
            State::CannotTell => result.cannot_tell.push(software.name),
            State::DoesNotRun | State::NotForThisSystem => {}
        }
    }
    result
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
    fn software_without_a_platform_identifier() {
        let nowhere = Software {
            name: "Nowhere",
            macos_bundle_id: None,
            linux_process: None,
        };
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            assert_eq!(is_running(&nowhere), State::NotForThisSystem);
        } else {
            assert_eq!(is_running(&nowhere), State::CannotTell);
        }
    }

    #[test]
    fn software_that_is_not_running_is_reported_as_such() {
        let absent = Software {
            name: "Absent",
            macos_bundle_id: Some("invalid.istesso.test.not-installed"),
            linux_process: Some("istesso-absent"),
        };
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            assert_eq!(is_running(&absent), State::DoesNotRun);
        }
    }

    #[test]
    fn look_lists_running_and_unknown_software_in_declaration_order() {
        let all = [
            "Unknown B",
            "Running B",
            "Stopped",
            "Other OS",
            "Running A",
            "Unknown A",
        ]
        .map(|name| Software {
            name,
            macos_bundle_id: None,
            linux_process: None,
        });
        let result = look_with(&all, |software| match software.name {
            "Running A" | "Running B" => State::Runs,
            "Unknown A" | "Unknown B" => State::CannotTell,
            "Stopped" => State::DoesNotRun,
            "Other OS" => State::NotForThisSystem,
            _ => unreachable!(),
        });
        assert_eq!(result.running, ["Running B", "Running A"]);
        assert_eq!(result.cannot_tell, ["Unknown B", "Unknown A"]);
    }
}
