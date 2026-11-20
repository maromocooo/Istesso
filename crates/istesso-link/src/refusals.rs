//! How often a listener says that it refused a connection.
//!
//! A refused caller can call again at once, as often as it likes; each refusal
//! would be one line of the log. In a minute, the first refusal of each key (a
//! caller, or a reason) is shown, for a few keys; the rest are counted and told
//! in one line when the minute is over.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

/// Keys whose first refusal is shown in one minute.
pub const KEYS_A_MINUTE: usize = 8;

/// How many different callers are counted at most in one minute.
const CALLERS_COUNTED: usize = 4096;

const MINUTE: Duration = Duration::from_secs(60);

/// The count of one listener's refusals. Keep one per listener, behind a lock.
#[derive(Debug, Default)]
pub struct RefusalLog {
    began: Option<Instant>,
    shown: BTreeSet<String>,
    held: u64,
    held_from: BTreeSet<String>,
}

impl RefusalLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// The lines to log for one more refusal at `now`. `key` is what one line is
    /// shown for in a minute (the caller's address, or the reason); `caller` is who
    /// called, for the count; `line` is what the refusal would say. First comes the
    /// count of a minute that is over.
    pub fn refused(&mut self, now: Instant, key: &str, caller: &str, line: String) -> Vec<String> {
        let mut said = self.tick(now);
        self.began.get_or_insert(now);
        if !self.shown.contains(key) && self.shown.len() < KEYS_A_MINUTE {
            self.shown.insert(key.to_string());
            said.push(line);
        } else {
            self.held += 1;
            if self.held_from.len() < CALLERS_COUNTED {
                self.held_from.insert(caller.to_string());
            }
        }
        said
    }

    /// When the minute is over: how many refusals were not shown, from how many
    /// callers. Asked now and then as well, so that the count comes even when no
    /// refusal follows.
    pub fn tick(&mut self, now: Instant) -> Vec<String> {
        let Some(began) = self.began else {
            return Vec::new();
        };
        if now.saturating_duration_since(began) < MINUTE {
            return Vec::new();
        }
        let mut said = Vec::new();
        if self.held > 0 {
            let callers = self.held_from.len();
            let at_least = if callers >= CALLERS_COUNTED {
                "at least "
            } else {
                ""
            };
            said.push(format!(
                "{} more connection(s) from {at_least}{callers} caller(s) were refused in the last minute (not shown one by one)",
                self.held
            ));
        }
        *self = RefusalLog::default();
        said
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_caller_that_floods_does_not_hide_another() {
        let mut log = RefusalLog::new();
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        assert_eq!(log.refused(at(0), "a", "a", "from a".into()), ["from a"]);
        for i in 0..100 {
            assert!(log.refused(at(1 + i), "a", "a", "from a".into()).is_empty());
        }
        // Another caller's first refusal is shown all the same.
        assert_eq!(log.refused(at(200), "b", "b", "from b".into()), ["from b"]);
        assert!(log.refused(at(201), "b", "b", "from b".into()).is_empty());
        // Nothing more within the minute; when it is over, the count and the callers.
        assert!(log.tick(at(59_000)).is_empty());
        assert_eq!(
            log.tick(at(60_000)),
            ["101 more connection(s) from 2 caller(s) were refused in the last minute (not shown one by one)"]
        );
        // Told once.
        assert!(log.tick(at(130_000)).is_empty());
        // A new minute shows the first again.
        assert_eq!(
            log.refused(at(140_000), "a", "a", "a again".into()),
            ["a again"]
        );
    }

    #[test]
    fn only_a_few_keys_are_shown_a_minute() {
        let mut log = RefusalLog::new();
        let start = Instant::now();
        for i in 0..KEYS_A_MINUTE + 3 {
            let key = format!("caller {i}");
            let said = log.refused(start, &key, &key, key.clone());
            assert_eq!(said.len(), usize::from(i < KEYS_A_MINUTE), "{i}");
        }
        // The count comes with the next refusal after the minute too.
        assert_eq!(
            log.refused(start + MINUTE, "late", "late", "late".into()),
            [
                "3 more connection(s) from 3 caller(s) were refused in the last minute (not shown one by one)"
                    .to_string(),
                "late".to_string()
            ]
        );
    }

    #[test]
    fn a_minute_with_nothing_held_back_says_nothing_more() {
        let mut log = RefusalLog::new();
        let start = Instant::now();
        assert_eq!(log.refused(start, "a", "a", "a".into()), ["a"]);
        assert!(log.tick(start + MINUTE).is_empty());
    }
}
