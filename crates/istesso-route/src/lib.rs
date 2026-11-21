//! Decides which machine a project's task goes to.
//!
//! A project states what each of its build, run and deploy tasks needs from
//! the machine it runs on, in `.istesso/tasks.json` ([`tasks`]); only a file
//! this user alone can have written is used ([`find`]). The role
//! table that `istesso hosts --json` prints says what every paired machine
//! can do ([`table`]). [`decide`] puts the two together and answers with a
//! place and the reasons, or with why there is none. A person's choice of a
//! deploy target is remembered per device and user ([`choices`]), so it is
//! asked only once. A task whose place is this machine can be run here
//! ([`exec`]); running on other machines waits for the isolation (M5).
//!
//! Nothing in this crate talks to other machines or reads this device's keys.
//!
//! What is checked before anything runs is the file this command reads
//! itself (the task file), the file it starts itself (a program the task
//! names by a relative path), and the folders leading to them. What a
//! started program reads or runs after that is not checked. Anything outside
//! this line is a limit written in the documents, not something to fix; one
//! of them, until ADR 0014, is that the ACLs of folders above the project's
//! root are not looked at.

mod accounts;
pub mod choices;
pub mod decide;
pub mod exec;
pub mod find;
pub mod table;
pub mod tasks;

use std::borrow::Cow;

/// Escapes control characters, bidirectional formatting and zero-width
/// characters in text printed for a person.
pub fn shown(text: &str) -> Cow<'_, str> {
    let needs_escape = |c: char| {
        c.is_control()
            || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}'
                | '\u{2060}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
    };
    if !text.contains(needs_escape) {
        return Cow::Borrowed(text);
    }
    let mut line = String::with_capacity(text.len());
    for c in text.chars() {
        if needs_escape(c) {
            line.extend(c.escape_debug());
        } else {
            line.push(c);
        }
    }
    Cow::Owned(line)
}

/// Exit codes of the `istesso-route` command.
///
/// A task that was run here returns its own exit code instead. The numbers
/// sit where ordinary commands do not put theirs, so that a coding agent can
/// tell the outcomes apart without reading the text.
pub mod exit {
    /// A place was decided; for `run`, `build` and `deploy`, the task ended with 0.
    pub const OK: i32 = 0;
    /// The command was written wrongly.
    pub const USAGE: i32 = 2;
    /// No machine meets the task's needs.
    pub const NOBODY: i32 = 70;
    /// Machines meet the needs, but none of them answers.
    pub const STOPPED: i32 = 71;
    /// A deploy task has no single place: none was chosen, or the chosen one
    /// cannot be used any more.
    pub const AMBIGUOUS: i32 = 72;
    /// A file cannot be read, there is no such task, or the task is of
    /// another kind (the task file, the role table, the remembered choices).
    pub const UNREADABLE: i32 = 73;
    /// The task states needs this version cannot decide on.
    pub const NOT_UNDERSTOOD: i32 = 74;
    /// The place is another machine; nothing was run.
    pub const ELSEWHERE: i32 = 75;
    /// The task's program could not be started (as shells report a command
    /// that is not found).
    pub const CANNOT_START: i32 = 127;

    /// Every code with a short name, in order.
    pub const ALL: [(i32, &str); 9] = [
        (OK, "ok"),
        (USAGE, "usage"),
        (NOBODY, "nobody"),
        (STOPPED, "stopped"),
        (AMBIGUOUS, "ambiguous"),
        (UNREADABLE, "unreadable"),
        (NOT_UNDERSTOOD, "not_understood"),
        (ELSEWHERE, "elsewhere"),
        (CANNOT_START, "cannot_start"),
    ];
}

/// A point in time (seconds since the Unix epoch) as `YYYY-MM-DD HH:MM UTC`.
pub fn utc_time(secs: u64) -> String {
    let (y, m, d) = civil_date(secs / 86_400);
    let rest = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        rest / 3600,
        (rest % 3600) / 60
    )
}

/// The day of a point in time as `YYYY-MM-DD` (UTC).
pub fn utc_date(secs: u64) -> String {
    let (y, m, d) = civil_date(secs / 86_400);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since 1970-01-01 to a proleptic Gregorian date.
fn civil_date(days: u64) -> (i64, u32, u32) {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// Seconds since the Unix epoch, now.
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shown_escapes_controls() {
        assert_eq!(
            shown("ノート\"\\\x1b\x07\r\n\t\0\x7f\u{9b}"),
            r#"ノート"\\u{1b}\u{7}\r\n\t\0\u{7f}\u{9b}"#
        );
    }

    #[test]
    fn shown_escapes_bidirectional_and_zero_width_characters() {
        let text = "\u{200e}\u{200f}\u{202a}\u{202b}\u{202c}\u{202d}\u{202e}\
                    \u{2066}\u{2067}\u{2068}\u{2069}\u{200b}\u{200c}\u{200d}\u{2060}\u{feff}";
        for c in text.chars() {
            assert_eq!(
                shown(&format!("a{c}b")),
                format!("a{}b", c.escape_unicode())
            );
        }
    }

    #[test]
    fn shown_borrows_plain_text() {
        let samples = ["tower", "ノート", "\"", "\\"]; // check-private: ok - fictional fixture names
        for text in samples {
            assert!(matches!(shown(text), Cow::Borrowed(s) if s == text));
        }
    }

    #[test]
    fn times_are_written_in_utc() {
        assert_eq!(utc_time(0), "1970-01-01 00:00 UTC");
        // 2026-11-17 01:30:00 UTC
        assert_eq!(utc_time(1_794_879_000), "2026-11-17 01:30 UTC");
        assert_eq!(utc_date(1_794_879_000), "2026-11-17");
        // A leap day.
        assert_eq!(utc_date(1_709_164_800), "2024-02-29");
    }

    #[test]
    fn exit_codes_are_distinct() {
        let mut codes: Vec<i32> = exit::ALL.iter().map(|(c, _)| *c).collect();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), exit::ALL.len());
    }
}
