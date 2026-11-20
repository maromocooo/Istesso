//! How often a listener says that it refused a connection.
//!
//! A refused caller can call again at once, as often as it likes; each refusal
//! is one line of the log. A few are shown in a minute, and the rest are
//! counted and told in one line when the next minute begins.

use std::time::{Duration, Instant};

/// Refusals shown in one minute; the rest are counted.
pub const SHOWN_A_MINUTE: u32 = 5;

const MINUTE: Duration = Duration::from_secs(60);

/// The count of one listener's refusals. Keep one per listener, behind a lock.
#[derive(Debug, Default)]
pub struct RefusalLog {
    began: Option<Instant>,
    shown: u32,
    held: u32,
}

impl RefusalLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// The lines to log for one more refusal at `now`, `line` being what it would say:
    /// none when enough were shown this minute; first, when a minute with refusals that
    /// were not shown is over, how many there were.
    pub fn refused(&mut self, now: Instant, line: String) -> Vec<String> {
        let mut said = Vec::new();
        let over = self
            .began
            .is_none_or(|began| now.saturating_duration_since(began) >= MINUTE);
        if over {
            if self.held > 0 {
                said.push(format!(
                    "{} more connection(s) were refused in that minute (not shown one by one)",
                    self.held
                ));
            }
            *self = RefusalLog {
                began: Some(now),
                shown: 0,
                held: 0,
            };
        }
        if self.shown < SHOWN_A_MINUTE {
            self.shown += 1;
            said.push(line);
        } else {
            self.held += 1;
        }
        said
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_few_are_shown_a_minute_and_the_rest_are_counted() {
        let mut log = RefusalLog::new();
        let start = Instant::now();
        for i in 0..SHOWN_A_MINUTE {
            let at = start + Duration::from_secs(u64::from(i));
            assert_eq!(
                log.refused(at, format!("refused {i}")),
                [format!("refused {i}")]
            );
        }
        // A caller that keeps calling fills no log.
        for i in 0..100 {
            assert!(log
                .refused(start + Duration::from_millis(10_000 + i), "again".into())
                .is_empty());
        }
        // The next minute: how many there were, and the new one.
        assert_eq!(
            log.refused(start + MINUTE, "later".into()),
            [
                "100 more connection(s) were refused in that minute (not shown one by one)"
                    .to_string(),
                "later".to_string()
            ]
        );
        // A minute with none held back says nothing more.
        assert_eq!(
            log.refused(start + MINUTE * 2, "quiet".into()),
            ["quiet".to_string()]
        );
    }
}
