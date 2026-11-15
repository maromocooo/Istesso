//! How busy this machine is right now.

use std::process::Command;

use ud_proto::{HostStatus, OsFamily};

/// Measures the local load and free memory. Values that cannot be read are
/// reported as unknown.
pub fn local_status() -> HostStatus {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get() as f32)
        .unwrap_or(1.0);
    match OsFamily::current() {
        OsFamily::Linux => HostStatus {
            load: std::fs::read_to_string("/proc/loadavg")
                .ok()
                .and_then(|t| first_number(&t))
                .map(|load| load / cores),
            memory_available_bytes: std::fs::read_to_string("/proc/meminfo")
                .ok()
                .and_then(|t| meminfo_available_bytes(&t)),
        },
        OsFamily::Macos => HostStatus {
            load: run("sysctl", &["-n", "vm.loadavg"])
                .and_then(|t| first_number(&t))
                .map(|load| load / cores),
            memory_available_bytes: run("vm_stat", &[]).and_then(|t| vm_stat_available_bytes(&t)),
        },
        _ => HostStatus::default(),
    }
}

fn run(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The first number in the text: the one-minute load average in both
/// `/proc/loadavg` ("0.52 0.58 0.59 1/1234 5678") and `sysctl -n vm.loadavg`
/// ("{ 1.52 1.60 1.66 }").
pub fn first_number(text: &str) -> Option<f32> {
    text.split(|c: char| c.is_whitespace() || c == '{' || c == '}')
        .find(|token| !token.is_empty())?
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
}

/// `MemAvailable` from `/proc/meminfo`, in bytes.
pub fn meminfo_available_bytes(text: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    kib.checked_mul(1024)
}

/// Memory that macOS could hand to applications without swapping, from
/// `vm_stat`: free, inactive, speculative and purgeable pages. This is an
/// estimate; macOS has no single figure like Linux's `MemAvailable`.
pub fn vm_stat_available_bytes(text: &str) -> Option<u64> {
    let page_size: u64 = text
        .lines()
        .next()?
        .split("page size of ")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    let pages = |label: &str| -> Option<u64> {
        let line = text.lines().find(|l| l.starts_with(label))?;
        line.rsplit(':')
            .next()?
            .trim()
            .trim_end_matches('.')
            .parse()
            .ok()
    };
    let free = pages("Pages free")?;
    let reclaimable = ["Pages inactive", "Pages speculative", "Pages purgeable"]
        .iter()
        .filter_map(|label| pages(label))
        .sum::<u64>();
    free.checked_add(reclaimable)?.checked_mul(page_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_one_minute_load() {
        assert_eq!(first_number("0.52 0.58 0.59 1/1234 5678\n"), Some(0.52));
        assert_eq!(first_number("{ 1.52 1.60 1.66 }\n"), Some(1.52));
        assert_eq!(first_number(""), None);
        assert_eq!(first_number("n/a 1.0"), None);
        assert_eq!(first_number("-1.0"), None);
    }

    #[test]
    fn reads_available_memory_on_linux() {
        let text =
            "MemTotal:       65792104 kB\nMemFree:         1234 kB\nMemAvailable:   40000000 kB\n";
        assert_eq!(meminfo_available_bytes(text), Some(40_000_000 * 1024));
        assert_eq!(meminfo_available_bytes("MemTotal: 1 kB\n"), None);
    }

    #[test]
    fn estimates_available_memory_on_macos() {
        let text = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\n\
                    Pages free:                               10000.\n\
                    Pages active:                            500000.\n\
                    Pages inactive:                          200000.\n\
                    Pages speculative:                         5000.\n\
                    Pages throttled:                              0.\n\
                    Pages wired down:                        150000.\n\
                    Pages purgeable:                           1000.\n\
                    \"Translation faults\":                 123456789.\n";
        assert_eq!(
            vm_stat_available_bytes(text),
            Some((10_000 + 200_000 + 5_000 + 1_000) * 16_384)
        );
        assert_eq!(vm_stat_available_bytes("garbage"), None);
        assert_eq!(vm_stat_available_bytes(""), None);
    }

    #[test]
    fn measuring_this_machine_gives_sane_values() {
        let status = local_status();
        if let Some(load) = status.load {
            assert!(load >= 0.0);
        }
        if cfg!(target_os = "linux") {
            assert!(status.load.is_some());
            assert!(status.memory_available_bytes.unwrap() > 0);
        }
    }
}
