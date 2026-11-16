//! The clock behind every timestamp in a log.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Counts nanoseconds from the moment it was created, on the machine's
/// monotonic clock.
///
/// Timestamps of one `Clock` are only ever compared with each other, so the
/// origin does not matter and two machines need not agree on the time of day.
///
/// Two programs on the same machine can still put their logs on one time
/// line: each says in the header of its log what the time of day was when
/// its clock read zero ([`Clock::origin_unix_ns`]).
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    origin: Instant,
    origin_unix_ns: u64,
}

impl Clock {
    pub fn new() -> Self {
        // The time of day is read between two readings of the monotonic
        // clock and counted as taken half way between them.
        let origin = Instant::now();
        let time_of_day = SystemTime::now();
        let half_gap = origin.elapsed() / 2;
        let origin_unix_ns = time_of_day
            .duration_since(UNIX_EPOCH)
            .map(|since_epoch| since_epoch.saturating_sub(half_gap))
            .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        Self {
            origin,
            origin_unix_ns,
        }
    }

    /// The time of day at which this clock read zero, in nanoseconds since
    /// the Unix epoch.
    ///
    /// Read once, when the clock was created. It lines up logs written on
    /// the same machine during the same minutes, as long as nobody set the
    /// machine's time of day in between. It is not for comparing two
    /// machines.
    pub fn origin_unix_ns(&self) -> u64 {
        self.origin_unix_ns
    }

    /// Nanoseconds since this clock was created.
    pub fn now_ns(&self) -> u64 {
        // 2^64 ns is more than 500 years.
        u64::try_from(self.origin.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_does_not_run_backwards() {
        let clock = Clock::new();
        let first = clock.now_ns();
        let second = clock.now_ns();
        assert!(second >= first);
    }

    #[test]
    fn the_origin_says_what_time_of_day_it_was() {
        let clock = Clock::new();
        std::thread::sleep(std::time::Duration::from_millis(3));
        let through_the_clock = clock.origin_unix_ns() + clock.now_ns();
        let time_of_day = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        // The two readings are a few microseconds apart; a loaded machine
        // may put more between them.
        assert!(
            time_of_day.abs_diff(through_the_clock) < 50_000_000,
            "{time_of_day} against {through_the_clock}"
        );
        assert_eq!(clock.origin_unix_ns(), { clock }.origin_unix_ns());
    }

    #[test]
    fn copies_share_the_origin() {
        let clock = Clock::new();
        let copy = clock;
        std::thread::sleep(std::time::Duration::from_millis(2));
        let a = clock.now_ns();
        let b = copy.now_ns();
        assert!(a >= 2_000_000);
        assert!(b >= a);
    }
}
