//! The last step before the output device: nothing that is not a number,
//! nothing beyond full scale, and a short fade to silence before the device
//! is stopped, so that stopping never cuts the sound off at a level.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// How long the fade to silence lasts when the outlet is told to close.
pub const CLOSING_FADE_S: f64 = 0.005;

/// How often the outlet had to act. A source that sends bad values shows
/// here.
#[derive(Debug, Default)]
pub struct OutletCounts {
    /// Samples that were not numbers (NaN, infinities) and became silence.
    pub not_numbers: AtomicU64,
    /// Samples beyond full scale that were held at full scale.
    pub limited: AtomicU64,
}

/// What every block of samples passes on its way to the device.
pub struct Outlet {
    channels: usize,
    fade_frames: usize,
    faded: usize,
    closing: Arc<AtomicBool>,
    counts: Arc<OutletCounts>,
}

impl Outlet {
    /// For a device that takes `channels` channels at `device_rate`. When
    /// `closing` is set, what follows fades to silence within
    /// [`CLOSING_FADE_S`] and stays silent.
    pub fn new(
        channels: usize,
        device_rate: f64,
        closing: Arc<AtomicBool>,
        counts: Arc<OutletCounts>,
    ) -> Self {
        Outlet {
            channels: channels.max(1),
            fade_frames: ((device_rate * CLOSING_FADE_S).round() as usize).max(1),
            faded: 0,
            closing,
            counts,
        }
    }

    /// First of all, before any gain: what is not a number becomes
    /// silence. A NaN multiplied by anything stays a NaN, so it must not
    /// reach any arithmetic that follows.
    pub fn clean(&mut self, out: &mut [f32]) {
        let mut not_numbers = 0u64;
        for sample in out.iter_mut() {
            if !sample.is_finite() {
                *sample = 0.0;
                not_numbers += 1;
            }
        }
        if not_numbers > 0 {
            self.counts
                .not_numbers
                .fetch_add(not_numbers, Ordering::Relaxed);
        }
    }

    /// Last of all: nothing beyond full scale, and the fade to silence once
    /// the outlet is closing.
    pub fn limit_and_close(&mut self, out: &mut [f32]) {
        let mut limited = 0u64;
        for sample in out.iter_mut() {
            // A NaN that came to be after `clean` is silence here too.
            if !(*sample >= -1.0 && *sample <= 1.0) {
                *sample = if *sample > 1.0 {
                    1.0
                } else if *sample < -1.0 {
                    -1.0
                } else {
                    0.0
                };
                limited += 1;
            }
        }
        if limited > 0 {
            self.counts.limited.fetch_add(limited, Ordering::Relaxed);
        }
        if !self.closing.load(Ordering::Relaxed) {
            return;
        }
        for frame in out.chunks_exact_mut(self.channels) {
            let gain = if self.faded >= self.fade_frames {
                0.0
            } else {
                1.0 - (self.faded + 1) as f32 / self.fade_frames as f32
            };
            for sample in frame {
                *sample *= gain;
            }
            self.faded = (self.faded + 1).min(self.fade_frames);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outlet(closing: &Arc<AtomicBool>) -> (Outlet, Arc<OutletCounts>) {
        let counts = Arc::new(OutletCounts::default());
        (
            Outlet::new(2, 48_000.0, closing.clone(), counts.clone()),
            counts,
        )
    }

    fn steepest(played: &[f32]) -> f32 {
        // The first channel of two.
        let first: Vec<f32> = played.iter().step_by(2).copied().collect();
        first
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn full_scale_fades_to_silence_before_the_device_is_stopped() {
        let closing = Arc::new(AtomicBool::new(false));
        let (mut outlet, _) = outlet(&closing);
        let mut played = Vec::new();
        for block in 0..6 {
            // The player asks to stop after the second block; the device
            // is stopped after the last one.
            if block == 2 {
                closing.store(true, Ordering::Relaxed);
            }
            let mut out = vec![1.0f32; 512 * 2];
            outlet.clean(&mut out);
            outlet.limit_and_close(&mut out);
            played.extend_from_slice(&out);
        }
        // Untouched until asked, silent 5 ms (240 frames) later, and no
        // step on the way larger than one 240th of full scale.
        assert!(played[..2 * 512 * 2].iter().all(|sample| *sample == 1.0));
        assert!(played[(2 * 512 + 240) * 2..]
            .iter()
            .all(|sample| *sample == 0.0));
        assert!(
            steepest(&played) <= 1.0 / 240.0 + 1e-6,
            "{}",
            steepest(&played)
        );
        // The device that takes over begins with an outlet of its own,
        // which lets everything through until it is closed in its turn.
        let (mut next, _) = self::outlet(&Arc::new(AtomicBool::new(false)));
        let mut out = vec![1.0f32; 64];
        next.limit_and_close(&mut out);
        assert!(out.iter().all(|sample| *sample == 1.0));
    }

    #[test]
    fn nothing_beyond_full_scale_reaches_the_device() {
        let (mut outlet, counts) = outlet(&Arc::new(AtomicBool::new(false)));
        let mut out = vec![1.5, -1.5, 0.25, -0.25, 1.0, -1.0];
        outlet.clean(&mut out);
        outlet.limit_and_close(&mut out);
        assert_eq!(out, vec![1.0, -1.0, 0.25, -0.25, 1.0, -1.0]);
        assert_eq!(counts.limited.load(Ordering::Relaxed), 2);
        assert_eq!(counts.not_numbers.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn what_is_not_a_number_becomes_silence_and_touches_nothing_else() {
        let (mut outlet, counts) = outlet(&Arc::new(AtomicBool::new(false)));
        let mut out = vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.3, -0.7, 0.3];
        outlet.clean(&mut out);
        // Cleaned before any gain: a gain applied now finds numbers only.
        for sample in &mut out {
            *sample *= 0.5;
        }
        outlet.limit_and_close(&mut out);
        assert_eq!(out, vec![0.0, 0.0, 0.0, 0.15, -0.35, 0.15]);
        assert_eq!(counts.not_numbers.load(Ordering::Relaxed), 3);
        assert_eq!(counts.limited.load(Ordering::Relaxed), 0);
        // The outlet keeps nothing of a sample: the next block is untouched.
        let mut next = vec![0.3, -0.7];
        outlet.clean(&mut next);
        outlet.limit_and_close(&mut next);
        assert_eq!(next, vec![0.3, -0.7]);
    }
}
