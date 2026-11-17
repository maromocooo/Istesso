//! The spacing of inputs during a measurement.

use std::time::Duration;

/// Intervals spread evenly between two bounds.
///
/// Inputs at a fixed interval lock onto the refresh cycle of the display and
/// give a biased result, so the measurement guide asks for intervals that
/// vary between 200 and 400 ms. The same seed gives the same sequence, which
/// makes a run repeatable.
///
/// The generator is SplitMix64. It only has to be even, not unpredictable: do
/// not use it for anything related to security.
#[derive(Clone, Debug)]
pub struct Jitter {
    state: u64,
    min_ns: u64,
    span_ns: u64,
}

impl Jitter {
    /// Intervals between `min` and `max`, both included. The bounds may be
    /// given in either order.
    pub fn new(seed: u64, min: Duration, max: Duration) -> Self {
        let a = nanos(min);
        let b = nanos(max);
        let (low, high) = if a <= b { (a, b) } else { (b, a) };
        Self {
            state: seed,
            min_ns: low,
            span_ns: high - low,
        }
    }

    /// The spacing the measurement guide asks for: 200 to 400 ms.
    pub fn standard(seed: u64) -> Self {
        Self::new(seed, Duration::from_millis(200), Duration::from_millis(400))
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

impl Iterator for Jitter {
    type Item = Duration;

    fn next(&mut self) -> Option<Duration> {
        let random = self.next_u64();
        // Scale to 0..=span without the bias of a remainder.
        let offset = if self.span_ns == u64::MAX {
            random
        } else {
            ((u128::from(random) * (u128::from(self.span_ns) + 1)) >> 64) as u64
        };
        Some(Duration::from_nanos(self.min_ns + offset))
    }
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intervals_stay_within_the_bounds() {
        let min = Duration::from_millis(200);
        let max = Duration::from_millis(400);
        for interval in Jitter::standard(1).take(10_000) {
            assert!(interval >= min && interval <= max, "{interval:?}");
        }
    }

    #[test]
    fn intervals_cover_the_whole_range_evenly() {
        // Ten equal bins between 200 and 400 ms; each should hold about a tenth.
        let mut bins = [0u32; 10];
        for interval in Jitter::standard(42).take(20_000) {
            let offset = interval.as_nanos() as u64 - 200_000_000;
            bins[((offset / 20_000_001) as usize).min(9)] += 1;
        }
        for (index, count) in bins.iter().enumerate() {
            assert!((1_800..=2_200).contains(count), "bin {index}: {count}");
        }
    }

    #[test]
    fn the_same_seed_repeats_and_another_seed_differs() {
        let a: Vec<Duration> = Jitter::standard(7).take(50).collect();
        let b: Vec<Duration> = Jitter::standard(7).take(50).collect();
        let c: Vec<Duration> = Jitter::standard(8).take(50).collect();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn bounds_in_either_order_and_equal_bounds() {
        let swapped = Jitter::new(3, Duration::from_millis(400), Duration::from_millis(200));
        for interval in swapped.take(100) {
            assert!(interval >= Duration::from_millis(200));
            assert!(interval <= Duration::from_millis(400));
        }
        let fixed = Jitter::new(3, Duration::from_millis(250), Duration::from_millis(250));
        for interval in fixed.take(10) {
            assert_eq!(interval, Duration::from_millis(250));
        }
    }
}
