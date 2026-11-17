//! When pictures are handed to the encoder.
//!
//! A display that is used for typing does not change every frame: the screen
//! stays still, one character appears, and it stays still again. The encoder
//! may take longer for a picture that comes after a pause than for one in a
//! steady stream, so the pump can be paced either way.

use std::time::Duration;

const NANOS_PER_SECOND: u128 = 1_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pace {
    /// One picture every `1 / fps` seconds, each different from the one before.
    Even { fps: u32 },
    /// A still screen with a change now and then: one changed picture (a
    /// probe) at the end of every `still`. In between, if `awake_fps` is not
    /// 0, the unchanged picture is handed over again at that rate, which may
    /// keep the encoder from resting.
    Sparse { still: Duration, awake_fps: u32 },
}

impl Pace {
    /// Unchanged pictures handed over in each still stretch.
    pub fn fillers(&self) -> u64 {
        match *self {
            Pace::Even { .. } | Pace::Sparse { awake_fps: 0, .. } => 0,
            Pace::Sparse { still, awake_fps } => {
                let slots = (still.as_nanos() * u128::from(awake_fps)).div_ceil(NANOS_PER_SECOND);
                // The last slot falls on the probe itself (or after it).
                slots.saturating_sub(1) as u64
            }
        }
    }

    /// When the picture with this index is due, counted from the start.
    pub fn due(&self, index: u64) -> Duration {
        match *self {
            Pace::Even { fps } => {
                nanos(u128::from(index) * NANOS_PER_SECOND / u128::from(fps.max(1)))
            }
            Pace::Sparse { still, awake_fps } => {
                let per = self.fillers() + 1;
                let (cycle, position) = (index / per, index % per);
                let start = still.as_nanos() * u128::from(cycle);
                if position + 1 == per {
                    nanos(start + still.as_nanos())
                } else {
                    nanos(
                        start
                            + u128::from(position + 1) * NANOS_PER_SECOND
                                / u128::from(awake_fps.max(1)),
                    )
                }
            }
        }
    }

    /// Whether the picture with this index differs from the one before it.
    /// In an even pace every picture does.
    pub fn is_probe(&self, index: u64) -> bool {
        match self {
            Pace::Even { .. } => true,
            Pace::Sparse { .. } => (index + 1) % (self.fillers() + 1) == 0,
        }
    }
}

fn nanos(value: u128) -> Duration {
    Duration::from_nanos(u64::try_from(value).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    #[test]
    fn an_even_pace_hands_over_every_interval_and_every_picture_changes() {
        let pace = Pace::Even { fps: 85 };
        assert_eq!(pace.due(0), Duration::ZERO);
        assert_eq!(pace.due(85), ms(1000));
        assert_eq!(pace.due(1), Duration::from_nanos(11_764_705));
        assert!((0..10).all(|i| pace.is_probe(i)));
        assert_eq!(pace.fillers(), 0);
    }

    #[test]
    fn a_sparse_pace_without_filling_hands_over_one_picture_per_still_stretch() {
        let pace = Pace::Sparse {
            still: ms(430),
            awake_fps: 0,
        };
        assert_eq!(pace.fillers(), 0);
        assert_eq!(pace.due(0), ms(430));
        assert_eq!(pace.due(1), ms(860));
        assert!((0..5).all(|i| pace.is_probe(i)));
    }

    #[test]
    fn a_sparse_pace_that_keeps_awake_fills_the_stretch_and_ends_it_with_the_probe() {
        let pace = Pace::Sparse {
            still: ms(1000),
            awake_fps: 10,
        };
        // 100, 200, ... 900 ms: the same picture again; 1000 ms: the change.
        assert_eq!(pace.fillers(), 9);
        assert_eq!(pace.due(0), ms(100));
        assert_eq!(pace.due(8), ms(900));
        assert_eq!(pace.due(9), ms(1000));
        assert_eq!(pace.due(10), ms(1100));
        assert_eq!(pace.due(19), ms(2000));
        let probes: Vec<u64> = (0..30).filter(|&i| pace.is_probe(i)).collect();
        assert_eq!(probes, vec![9, 19, 29]);
    }

    #[test]
    fn filling_that_does_not_divide_the_stretch_stays_before_the_probe() {
        let pace = Pace::Sparse {
            still: ms(430),
            awake_fps: 30,
        };
        // 30 a second for 430 ms is 12.9 slots: 12 unchanged pictures, at 33..400 ms.
        assert_eq!(pace.fillers(), 12);
        assert!(pace.due(11) < pace.due(12));
        assert_eq!(pace.due(12), ms(430));
        assert!(pace.is_probe(12) && !pace.is_probe(11));
    }
}
