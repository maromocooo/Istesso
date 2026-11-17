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
    /// After an input that is likely answered by a picture, no repeat for
    /// this long, so that the frame that answers it does not wait behind one
    /// (0: no holding back). Never more than this past a repeat's own beat.
    pub hold_after_input: Duration,
}

/// An input delivered to the application, as far as keeping awake cares.
///
/// Only presses and scrolling hold the repeats back: they are likely to be
/// answered by a new picture. Pointer motion must not hold anything back —
/// it comes 60 or more times a second over a still window, and holding back
/// on it would stop every repeat and leave the encoder asleep for the click
/// that follows. Releases are rarely answered by a picture either. The
/// capture loop reports every input with its kind; this type decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    PointerMotion,
    KeyDown,
    KeyUp,
    ButtonDown,
    ButtonUp,
    Scroll,
}

impl InputKind {
    /// Whether this input holds the repeats back.
    pub fn holds(self) -> bool {
        matches!(
            self,
            InputKind::KeyDown | InputKind::ButtonDown | InputKind::Scroll
        )
    }
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
///
/// The repeats keep a beat of `1 / fps` that starts at the last new picture.
/// Nothing here depends on the caller waking exactly on time: a thread may be
/// woken late (on the Air, at nice 19, a 50 ms sleep took about 115 ms).
/// When a repeat is handed over late, the next one goes back onto the beat:
/// it is the first beat at least half an interval after the late one.
/// Beats that were missed are dropped, never made up for, so pictures are
/// never bunched — no two repeats closer together than half an interval.
/// While the caller is late by less than half an interval, the rate stays
/// `fps`; when it is later, the rate falls, but the beat does not drift.
///
/// An input that holds (see [`InputKind`]) delays the next repeat to
/// `hold_after_input` after it, but never further than `hold_after_input`
/// past the repeat's own beat: under a continuous stream of key presses the
/// repeats still come, at 10 a second with the defaults. That is where the
/// encoder slows down again, so it is better than nothing, not enough.
#[derive(Debug, Clone)]
pub struct KeepAwake {
    settings: KeepAwakeSettings,
    next_beat: Option<Instant>,
    held_until: Option<Instant>,
}

impl KeepAwake {
    pub fn new(settings: KeepAwakeSettings) -> Self {
        KeepAwake {
            settings,
            next_beat: None,
            held_until: None,
        }
    }

    pub fn settings(&self) -> KeepAwakeSettings {
        self.settings
    }

    /// `None` when switched off or at 0 a second.
    fn interval(&self) -> Option<Duration> {
        if !self.settings.enabled {
            return None;
        }
        1_000_000_000u64
            .checked_div(u64::from(self.settings.fps))
            .map(Duration::from_nanos)
    }

    /// A new picture was handed to the encoder: the beat starts again from it.
    pub fn picture(&mut self, at: Instant) {
        self.next_beat = self.interval().map(|interval| at + interval);
    }

    /// The last picture was handed over again, at `at` (on time or late).
    pub fn repeated(&mut self, at: Instant) {
        let (Some(interval), Some(beat)) = (self.interval(), self.next_beat) else {
            return;
        };
        // The first beat after `beat` that is at least half an interval after `at`, in one
        // step: after days without a hand-over, walking beat by beat would take millions.
        let first = beat + interval;
        let earliest = at + interval / 2;
        let next = match earliest.checked_duration_since(first) {
            None => first,
            Some(short) => {
                let step = interval.as_nanos();
                let beats = short.as_nanos().div_ceil(step);
                first + Duration::from_nanos(u64::try_from(beats * step).unwrap_or(u64::MAX))
            }
        };
        self.next_beat = Some(next);
    }

    /// An input was delivered to the application. Only the kinds that
    /// [hold](InputKind::holds) change anything.
    pub fn input(&mut self, at: Instant, kind: InputKind) {
        if kind.holds() && !self.settings.hold_after_input.is_zero() {
            let until = at + self.settings.hold_after_input;
            self.held_until = Some(self.held_until.map_or(until, |held| held.max(until)));
        }
    }

    /// The next time the last picture should be handed over again. `None`
    /// when it should not be at all: switched off, no device being shown
    /// the session (`viewed`), or no picture handed over yet.
    pub fn next_repeat(&self, viewed: bool) -> Option<Instant> {
        self.interval()?;
        if !viewed {
            return None;
        }
        let beat = self.next_beat?;
        Some(match self.held_until {
            Some(held) => beat.max(held).min(beat + self.settings.hold_after_input),
            None => beat,
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
        keep.picture(start);
        assert_eq!(keep.next_repeat(true), Some(start + ms(50)));
        assert!(!keep.repeat_due(start + ms(49), true));
        assert!(keep.repeat_due(start + ms(50), true));
        // On time: the next beat.
        keep.repeated(start + ms(50));
        assert_eq!(keep.next_repeat(true), Some(start + ms(100)));
        // A new picture starts the beat again.
        keep.picture(start + ms(70));
        assert_eq!(keep.next_repeat(true), Some(start + ms(120)));
    }

    #[test]
    fn nothing_is_repeated_while_no_device_is_shown_the_session_or_when_it_is_off() {
        let start = Instant::now();
        let mut keep = KeepAwake::new(KeepAwakeSettings::default());
        keep.picture(start);
        assert_eq!(keep.next_repeat(false), None);
        assert!(!keep.repeat_due(start + ms(500), false));
        let mut off = KeepAwake::new(KeepAwakeSettings {
            enabled: false,
            ..KeepAwakeSettings::default()
        });
        off.picture(start);
        assert_eq!(off.next_repeat(true), None);
    }

    /// Woken late every time: each repeat goes back onto the beat, missed
    /// beats are dropped, and no two repeats come closer than half an interval.
    fn repeats_when_woken(late: Duration, beats: u32) -> Vec<Duration> {
        let start = Instant::now();
        let mut keep = KeepAwake::new(KeepAwakeSettings::default());
        keep.picture(start);
        let mut handed = Vec::new();
        for _ in 0..beats {
            let due = keep.next_repeat(true).unwrap();
            let woke = due + late;
            assert!(keep.repeat_due(woke, true));
            keep.repeated(woke);
            handed.push(woke - start);
        }
        handed
    }

    #[test]
    fn a_little_late_keeps_twenty_a_second_on_the_beat() {
        let handed = repeats_when_woken(ms(7), 200);
        // Every repeat is 7 ms after its beat: 57, 107, 157 ... ms; the beat does not drift.
        for (n, at) in handed.iter().enumerate() {
            assert_eq!(*at, ms(50 * (n as u64 + 1) + 7));
        }
    }

    #[test]
    fn very_late_drops_beats_and_never_bunches() {
        // A 50 ms wait that takes 115 ms, as at nice 19 on the Air.
        let handed = repeats_when_woken(ms(65), 50);
        for pair in handed.windows(2) {
            let gap = pair[1] - pair[0];
            assert!(gap >= ms(25), "two repeats {gap:?} apart");
            // Always back on the beat (a multiple of 50 ms), then 65 ms late.
            assert_eq!((pair[1].as_millis() - 65) % 50, 0, "{:?}", pair[1]);
        }
        // Fewer than twenty a second, but never more.
        let span = *handed.last().unwrap() - handed[0];
        let rate = (handed.len() - 1) as f64 / span.as_secs_f64();
        assert!(rate < 20.0 && rate > 5.0, "{rate}");
    }

    #[test]
    fn an_input_holds_the_repeats_back_when_asked_to() {
        let start = Instant::now();
        let mut keep = KeepAwake::new(KeepAwakeSettings {
            hold_after_input: ms(50),
            ..KeepAwakeSettings::default()
        });
        keep.picture(start);
        keep.input(start + ms(40), InputKind::KeyDown);
        assert_eq!(keep.next_repeat(true), Some(start + ms(90)));
        // A later input holds longer, but never more than 50 ms past the beat (50 ms):
        // held to 110 ms, due at 100 ms. An earlier input does not shorten it.
        keep.input(start + ms(60), InputKind::ButtonDown);
        keep.input(start + ms(10), InputKind::Scroll);
        assert_eq!(keep.next_repeat(true), Some(start + ms(100)));
        // Pointer motion holds nothing.
        let mut moved = KeepAwake::new(KeepAwakeSettings::default());
        moved.picture(start);
        moved.input(start + ms(40), InputKind::PointerMotion);
        assert_eq!(moved.next_repeat(true), Some(start + ms(50)));
        // Without holding back, an input changes nothing.
        let mut plain = KeepAwake::new(KeepAwakeSettings {
            hold_after_input: Duration::ZERO,
            ..KeepAwakeSettings::default()
        });
        plain.picture(start);
        plain.input(start + ms(40), InputKind::KeyDown);
        assert_eq!(plain.next_repeat(true), Some(start + ms(50)));
    }

    /// Hands over every repeat on time, feeding `inputs` (time from the
    /// start, kind) as they come; returns the times of the repeats.
    fn drive(inputs: &[(Duration, InputKind)], until: Duration) -> Vec<Duration> {
        let start = Instant::now();
        let mut keep = KeepAwake::new(KeepAwakeSettings::default());
        keep.picture(start);
        let mut pending = inputs.iter().peekable();
        let mut repeats = Vec::new();
        loop {
            let due = keep.next_repeat(true).unwrap() - start;
            // Inputs that come before the repeat is due change when it is due.
            if let Some(&&(at, kind)) = pending.peek() {
                if at <= due {
                    keep.input(start + at, kind);
                    pending.next();
                    continue;
                }
            }
            if due > until {
                return repeats;
            }
            keep.repeated(start + due);
            repeats.push(due);
        }
    }

    #[test]
    fn pointer_motion_does_not_hold_the_repeats_back() {
        // 200 motions 8 ms apart, from 10 ms on: 1.6 s of moving over a still window.
        let motions: Vec<_> = (0..200)
            .map(|n| (ms(10 + 8 * n), InputKind::PointerMotion))
            .collect();
        let repeats = drive(&motions, ms(1_650));
        let expected: Vec<_> = (1..=33).map(|n| ms(50 * n)).collect();
        assert_eq!(repeats, expected, "twenty a second, on the beat");
        assert!(!InputKind::PointerMotion.holds());
        assert!(!InputKind::KeyUp.holds() && !InputKind::ButtonUp.holds());
    }

    #[test]
    fn key_presses_without_pause_still_let_a_repeat_through_every_100_ms() {
        // A key press every 20 ms for 2 s.
        let presses: Vec<_> = (0..100)
            .map(|n| (ms(5 + 20 * n), InputKind::KeyDown))
            .collect();
        let repeats = drive(&presses, ms(2_000));
        assert!(repeats[0] <= ms(100), "{repeats:?}");
        for pair in repeats.windows(2) {
            assert!(pair[1] - pair[0] <= ms(100), "{repeats:?}");
            assert!(pair[1] - pair[0] >= ms(25), "{repeats:?}");
        }
        assert!(repeats.len() >= 19, "{}", repeats.len());
    }

    #[test]
    fn after_thirty_days_without_a_hand_over_the_next_beat_comes_at_once_and_on_the_beat() {
        let start = Instant::now();
        let mut keep = KeepAwake::new(KeepAwakeSettings::default());
        keep.picture(start);
        let late = Duration::from_secs(30 * 24 * 3600) + ms(13);
        let began = Instant::now();
        keep.repeated(start + late);
        assert!(began.elapsed() < ms(5), "computed, not walked");
        let next = keep.next_repeat(true).unwrap() - start;
        assert_eq!(
            next.as_nanos() % ms(50).as_nanos(),
            0,
            "{next:?} is on the beat"
        );
        assert!(next >= late + ms(25) && next < late + ms(75), "{next:?}");
    }
}
