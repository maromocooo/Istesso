//! The clock that arrivals and playing are measured with.

use std::sync::OnceLock;
use std::time::Instant;

static BEGIN: OnceLock<Instant> = OnceLock::new();

/// Nanoseconds on this machine's monotonic clock, counted from the first
/// call in this process. Meaningful only as a difference, and only within
/// one process; two machines compare their clocks with an exchange
/// (`session::clock_offset`).
pub fn now_ns() -> u64 {
    let begin = *BEGIN.get_or_init(Instant::now);
    Instant::now().saturating_duration_since(begin).as_nanos() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_does_not_run_backwards() {
        let first = now_ns();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = now_ns();
        assert!(second >= first + 2_000_000);
        // Another thread reads the same clock.
        let other = std::thread::spawn(now_ns).join().unwrap();
        assert!(other >= second);
    }
}
