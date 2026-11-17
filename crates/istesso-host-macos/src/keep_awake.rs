//! Keeping the Mac's encoder awake while the screen is still.
//!
//! The Mac's encoder takes about 13 ms for a frame that comes after a pause,
//! against about 9 ms when it is handed pictures 20 or more times a second
//! (docs/measurements/2026-11-17-m4-encode-sparse-mac-mini.md). So while a
//! device is being shown the session, the last picture is handed over again
//! whenever nothing new has come for a twentieth of a second. Those frames
//! are sent like any other (decided 2026-11-17, docs/m4-plan.md 6.1b F).
//!
//! This module only decides *when*; `crate::feed` (macOS) does the handing over.

use std::time::{Duration, Instant};

/// The setting. On by default; people whose host is a Mac on battery may
/// turn it off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeepAwakeSettings {
    pub enabled: bool,
    /// Pictures a second while nothing changes.
    pub fps: u32,
    /// After an input, no repeat for this long, so that the frame that
    /// answers the input does not wait behind one (0: no holding back).
    pub hold_after_input: Duration,
}

impl Default for KeepAwakeSettings {
    fn default() -> Self {
        KeepAwakeSettings {
            enabled: true,
            fps: 20,
            hold_after_input: HOLD_AFTER_INPUT,
        }
    }
}

/// Measured on the Mac mini: holding back 50 ms after an input makes a
/// frame that answers it meet a repeat a quarter as often (docs/measurements/
/// 2026-11-17-m4-encode-keep-awake-mac-mini.md).
pub const HOLD_AFTER_INPUT: Duration = Duration::from_millis(50);

/// When the last picture is to be handed over again.
#[derive(Debug, Clone)]
pub struct KeepAwake {
    settings: KeepAwakeSettings,
    last_handed: Option<Instant>,
    held_until: Option<Instant>,
}

impl KeepAwake {
    pub fn new(settings: KeepAwakeSettings) -> Self {
        KeepAwake {
            settings,
            last_handed: None,
            held_until: None,
        }
    }

    pub fn settings(&self) -> KeepAwakeSettings {
        self.settings
    }

    /// A picture, new or repeated, was handed to the encoder.
    pub fn handed(&mut self, at: Instant) {
        self.last_handed = Some(at);
    }

    /// An input for the session arrived.
    pub fn input(&mut self, at: Instant) {
        if !self.settings.hold_after_input.is_zero() {
            let until = at + self.settings.hold_after_input;
            self.held_until = Some(self.held_until.map_or(until, |held| held.max(until)));
        }
    }

    /// The next time the last picture should be handed over again. `None`
    /// when it should not be at all: switched off, no device being shown
    /// the session (`viewed`), or no picture handed over yet.
    pub fn next_repeat(&self, viewed: bool) -> Option<Instant> {
        if !self.settings.enabled || !viewed || self.settings.fps == 0 {
            return None;
        }
        let interval = Duration::from_nanos(1_000_000_000 / u64::from(self.settings.fps));
        let after_last = self.last_handed? + interval;
        Some(match self.held_until {
            Some(held) => after_last.max(held),
            None => after_last,
        })
    }

    /// Whether the last picture should be handed over again now.
    pub fn repeat_due(&self, now: Instant, viewed: bool) -> bool {
        self.next_repeat(viewed).is_some_and(|due| due <= now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    #[test]
    fn it_is_on_by_default_at_twenty_a_second() {
        let settings = KeepAwakeSettings::default();
        assert!(settings.enabled);
        assert_eq!(settings.fps, 20);
        assert_eq!(settings.hold_after_input, Duration::from_millis(50));
    }

    #[test]
    fn the_last_picture_is_repeated_a_twentieth_of_a_second_after_it_was_handed_over() {
        let start = Instant::now();
        let mut keep = KeepAwake::new(KeepAwakeSettings::default());
        assert_eq!(keep.next_repeat(true), None, "nothing to repeat yet");
        keep.handed(start);
        assert_eq!(keep.next_repeat(true), Some(start + ms(50)));
        assert!(!keep.repeat_due(start + ms(49), true));
        assert!(keep.repeat_due(start + ms(50), true));
        // A new picture moves the next repeat.
        keep.handed(start + ms(30));
        assert_eq!(keep.next_repeat(true), Some(start + ms(80)));
    }

    #[test]
    fn nothing_is_repeated_while_no_device_is_shown_the_session_or_when_it_is_off() {
        let start = Instant::now();
        let mut keep = KeepAwake::new(KeepAwakeSettings::default());
        keep.handed(start);
        assert_eq!(keep.next_repeat(false), None);
        assert!(!keep.repeat_due(start + ms(500), false));
        let mut off = KeepAwake::new(KeepAwakeSettings {
            enabled: false,
            ..KeepAwakeSettings::default()
        });
        off.handed(start);
        assert_eq!(off.next_repeat(true), None);
    }

    #[test]
    fn an_input_holds_the_repeats_back_when_asked_to() {
        let start = Instant::now();
        let mut keep = KeepAwake::new(KeepAwakeSettings {
            hold_after_input: ms(50),
            ..KeepAwakeSettings::default()
        });
        keep.handed(start);
        keep.input(start + ms(40));
        assert_eq!(keep.next_repeat(true), Some(start + ms(90)));
        // A later input holds longer; an earlier one does not shorten it.
        keep.input(start + ms(60));
        keep.input(start + ms(10));
        assert_eq!(keep.next_repeat(true), Some(start + ms(110)));
        // Without holding back, an input changes nothing.
        let mut plain = KeepAwake::new(KeepAwakeSettings {
            hold_after_input: Duration::ZERO,
            ..KeepAwakeSettings::default()
        });
        plain.handed(start);
        plain.input(start + ms(40));
        assert_eq!(plain.next_repeat(true), Some(start + ms(50)));
    }
}
