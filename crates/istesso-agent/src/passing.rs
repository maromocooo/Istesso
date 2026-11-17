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
//! The sending end also writes what happened to its conversation with the
//! other device, with the number of times it has been cut so far:
//!
//! ```text
//! {"kind":"reconnections","unix_ns":1763300000000000000,"event":"open","input":0}
//! {"kind":"reconnections","unix_ns":1763300060000000000,"event":"cut","input":1}
//! {"kind":"reconnections","unix_ns":1763300060900000000,"event":"back","input":1}
//! ```
//!
//! The last such line has the count. A log without any was written by a
//! program that does not count: that is not the same as 0.
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

    /// Notes what happened to the conversation the input goes over (`event`:
    /// "open", "cut", "back"), and how often it has been cut so far.
    pub fn reconnection(&self, event: &str, cuts: u64) {
        let mark = PassLog::mark();
        if let Ok(mut file) = self.file.lock() {
            let _ = writeln!(
                file,
                "{{\"kind\":\"reconnections\",\"unix_ns\":{},\"event\":\"{event}\",\"input\":{cuts}}}",
                mark.unix_ns
            );
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
    fn the_number_of_cuts_is_written_with_what_happened() {
        let dir = crate::testing::TempDir::new("passing-cuts");
        let path = dir.path().join("passed.jsonl");
        let log = PassLog::create(&path).unwrap();
        log.reconnection("open", 0);
        log.reconnection("cut", 1);
        log.reconnection("back", 1);
        drop(log);
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 3);
        for (line, (event, cuts)) in lines.iter().zip([("open", 0), ("cut", 1), ("back", 1)]) {
            assert_eq!(line["kind"], "reconnections");
            assert_eq!(line["event"], event);
            assert_eq!(line["input"], cuts);
            assert!(line["unix_ns"].as_u64().unwrap() > 1_700_000_000_000_000_000);
        }
    }

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
