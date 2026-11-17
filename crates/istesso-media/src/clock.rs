//! The clock that the times in a frame's record are read from.
//!
//! It is the clock the measuring tools on each machine already use, so that
//! a time taken in this library can be set next to theirs without any
//! conversion: on macOS the uptime clock that the screen probe and the
//! viewer stamp with (mach absolute time, in nanoseconds), elsewhere the
//! monotonic clock (`time.monotonic_ns()` in the host's script).
//!
//! Meaningful only as a difference, and only on one machine. Two machines'
//! clocks are never compared.

#[cfg(target_os = "macos")]
const CLOCK: libc::clockid_t = libc::CLOCK_UPTIME_RAW;
#[cfg(not(target_os = "macos"))]
const CLOCK: libc::clockid_t = libc::CLOCK_MONOTONIC;

/// Nanoseconds on this machine's clock for measurements.
pub fn now_ns() -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `time` is a valid place for the one value the call writes.
    unsafe { libc::clock_gettime(CLOCK, &mut time) };
    time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_does_not_run_backwards() {
        let first = now_ns();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = now_ns();
        assert!(second >= first + 2_000_000, "{first} then {second}");
        // Another thread reads the same clock.
        let other = std::thread::spawn(now_ns).join().unwrap();
        assert!(other >= second);
    }
}
