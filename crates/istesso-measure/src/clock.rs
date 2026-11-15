//! The clock behind every timestamp in a log.

use std::time::Instant;

/// Counts nanoseconds from the moment it was created, on the machine's
/// monotonic clock.
///
/// Timestamps of one `Clock` are only ever compared with each other, so the
/// origin does not matter and two machines need not agree on the time of day.
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    origin: Instant,
}

impl Clock {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
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
