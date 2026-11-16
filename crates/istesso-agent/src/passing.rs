//! When input passed through a program, written down for measuring.
//!
//! The stretch between a key on the client and the application on the host
//! runs through several programs, and a measurement wants to know how long
//! each of them holds an input. A program that is asked to writes one line
//! for every message of input it passes on:
//!
//! ```text
//! {"kind":"passed","unix_ns":1763300000000000000,"took_ns":41000,"events":2}
//! ```
//!
//! `unix_ns` is the time of day at which the message was in hand, `took_ns`
//! how long it was until it had been handed on (on the monotonic clock), and
//! `events` how many events it carried.
//!
//! What the input was is never written. The times alone still say when
//! someone typed, so nothing is written unless it is asked for, and the file
//! can be read by its owner only.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// The moment a message of input was in hand.
#[derive(Clone, Copy, Debug)]
pub struct Mark {
    at: Instant,
    unix_ns: u128,
}

/// The file the passing of input is written to.
#[derive(Debug)]
pub struct PassLog {
    file: Mutex<File>,
}

impl PassLog {
    /// Starts the log anew at `path`.
    pub fn create(path: &Path) -> std::io::Result<PassLog> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        // A file that was already there keeps the rights it had.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(PassLog {
            file: Mutex::new(file),
        })
    }

    /// Now, as the moment a message was in hand.
    pub fn mark() -> Mark {
        Mark {
            at: Instant::now(),
            unix_ns: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |since| since.as_nanos()),
        }
    }

    /// Notes that the message in hand at `mark` has been handed on just now.
    /// A log that cannot be written to loses the line; the input goes on.
    pub fn passed(&self, mark: Mark, events: usize) {
        let took = mark.at.elapsed().as_nanos();
        if let Ok(mut file) = self.file.lock() {
            let _ = writeln!(
                file,
                "{{\"kind\":\"passed\",\"unix_ns\":{},\"took_ns\":{took},\"events\":{events}}}",
                mark.unix_ns
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_says_when_how_long_and_how_many_and_nothing_else() {
        let dir = crate::testing::TempDir::new("passing");
        let path = dir.path().join("passed.jsonl");
        let log = PassLog::create(&path).unwrap();
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mark = PassLog::mark();
        std::thread::sleep(std::time::Duration::from_millis(2));
        log.passed(mark, 2);
        log.passed(PassLog::mark(), 1);

        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        let first = lines[0].as_object().unwrap();
        let mut keys: Vec<&str> = first.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["events", "kind", "took_ns", "unix_ns"]);
        assert_eq!(first["kind"], "passed");
        assert_eq!(first["events"], 2);
        assert!(first["took_ns"].as_u64().unwrap() >= 2_000_000);
        assert!(u128::from(first["unix_ns"].as_u64().unwrap()) >= before);
        assert_eq!(lines[1]["events"], 1);
    }

    #[cfg(unix)]
    #[test]
    fn only_the_owner_can_read_the_log_even_where_a_file_was_before() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::testing::TempDir::new("passing-rights");
        let path = dir.path().join("passed.jsonl");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let _log = PassLog::create(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
    }
}
