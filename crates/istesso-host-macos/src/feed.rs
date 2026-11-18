//! Pictures as they come — from capture, only when something changed — to
//! the encoder and on to the sink, keeping the encoder awake in between
//! ([`crate::keep_awake`]).
//!
//! The capture loop (not written yet: it waits for the permissions, ADR 0017)
//! calls [`Feeder::picture`] for every new picture, [`Feeder::input`] for
//! every input it delivers to the application, with its kind (pointer motion
//! included: [`InputKind`] decides that it holds nothing back), and
//! [`Feeder::tick`] whenever [`Feeder::next_wake`] comes round.
//!
//! The encoder's callback hands each frame to the sink while it holds the
//! feeder's lock, and the capture thread takes the same lock to hand over a
//! picture. That is kept, because the sink is the host end of the picture's
//! transport, whose `send` only queues ([`FrameSink::send`] must not block).
//! The longest `send` is counted ([`FeedCounts::longest_send_ns`]), so that
//! a sink that blocks shows in the records.
//!
//! The feeder does not rely on being woken on time. A [`Feeder::tick`] that
//! comes late hands over one repeat, not one for every beat it missed, and
//! the next one is back on the 20-a-second beat (see [`KeepAwake`]). The
//! capture loop should still run at the normal priority: woken too late,
//! the rate falls towards 10 a second, where the encoder slows down again.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::encoder::{Encoder, Error, Outcome, PixelBuffer, Settings};
use crate::keep_awake::{InputKind, KeepAwake, KeepAwakeSettings};
use crate::sink::{EncodedFrame, FrameSink};

/// Counts, for the records.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FeedCounts {
    /// New pictures handed over.
    pub pictures: u64,
    /// The last picture handed over again to keep the encoder awake.
    pub repeats: u64,
    /// Frames the sink took.
    pub sent: u64,
    /// Frames the sink would not take.
    pub unsent: u64,
    pub dropped: u64,
    pub failed: u64,
    /// The longest time one `send` to the sink took, in nanoseconds. The
    /// next picture's hand-over may have waited that long.
    pub longest_send_ns: u64,
}

struct Shared {
    sink: Box<dyn FrameSink>,
    counts: FeedCounts,
    started: Instant,
}

/// One session's encoder, fed with pictures as they come.
pub struct Feeder {
    encoder: Encoder,
    shared: Arc<Mutex<Shared>>,
    keep_awake: KeepAwake,
    last: Option<PixelBuffer>,
    last_pts_ns: u64,
}

impl Feeder {
    pub fn new(
        settings: &Settings,
        keep_awake: KeepAwakeSettings,
        sink: Box<dyn FrameSink>,
    ) -> Result<Self, Error> {
        let shared = Arc::new(Mutex::new(Shared {
            sink,
            counts: FeedCounts::default(),
            started: Instant::now(),
        }));
        let for_encoder = Arc::clone(&shared);
        let encoder = Encoder::new(settings, move |outcome| {
            let Ok(mut shared) = for_encoder.lock() else {
                return;
            };
            let shared = &mut *shared;
            match outcome {
                Outcome::Frame {
                    number,
                    data,
                    key_frame,
                    encoded_at,
                } => {
                    // The number handed to the encoder is the presentation time itself.
                    let frame = EncodedFrame {
                        data,
                        pts_ns: number,
                        encoded_ns: encoded_at.duration_since(shared.started).as_nanos() as u64,
                        key_frame,
                    };
                    let sending = Instant::now();
                    let sent = shared.sink.send(&frame).is_ok();
                    let took = sending.elapsed().as_nanos() as u64;
                    shared.counts.longest_send_ns = shared.counts.longest_send_ns.max(took);
                    if sent {
                        shared.counts.sent += 1;
                    } else {
                        shared.counts.unsent += 1;
                    }
                }
                Outcome::Dropped { .. } => shared.counts.dropped += 1,
                Outcome::Failed { .. } => shared.counts.failed += 1,
            }
        })?;
        Ok(Feeder {
            encoder,
            shared,
            keep_awake: KeepAwake::new(keep_awake),
            last: None,
            last_pts_ns: 0,
        })
    }

    /// A new picture: encode it, and keep it for repeating.
    pub fn picture(&mut self, picture: PixelBuffer) -> Result<(), Error> {
        let at = self.hand_over(&picture)?;
        self.keep_awake.picture(at);
        self.with_counts(|c| c.pictures += 1);
        self.last = Some(picture);
        Ok(())
    }

    /// An input for the session was delivered to the application. Report
    /// every one with its kind; only presses and scrolling hold the repeats
    /// back ([`InputKind::holds`]).
    pub fn input(&mut self, kind: InputKind) {
        self.keep_awake.input(Instant::now(), kind);
    }

    /// Hands the last picture over again if it is time to. Returns whether
    /// it did.
    pub fn tick(&mut self) -> Result<bool, Error> {
        let viewed = self.viewed();
        if !self.keep_awake.repeat_due(Instant::now(), viewed) {
            return Ok(false);
        }
        let Some(last) = self.last.take() else {
            return Ok(false);
        };
        let outcome = self.hand_over(&last);
        self.last = Some(last);
        self.keep_awake.repeated(outcome?);
        self.with_counts(|c| c.repeats += 1);
        Ok(true)
    }

    /// When [`Feeder::tick`] has something to do next; `None` while the
    /// encoder is not to be kept awake (switched off, no device shown the
    /// session, nothing handed over yet).
    pub fn next_wake(&self) -> Option<Instant> {
        self.keep_awake.next_repeat(self.viewed())
    }

    pub fn counts(&self) -> FeedCounts {
        self.shared.lock().map(|s| s.counts).unwrap_or_default()
    }

    /// Waits until every picture handed over so far has come out.
    pub fn finish(&mut self) -> Result<(), Error> {
        self.encoder.finish()
    }

    fn viewed(&self) -> bool {
        self.shared.lock().map(|s| s.sink.viewed()).unwrap_or(false)
    }

    fn with_counts(&self, change: impl FnOnce(&mut FeedCounts)) {
        if let Ok(mut shared) = self.shared.lock() {
            change(&mut shared.counts);
        }
    }

    /// Returns when the picture was handed over.
    fn hand_over(&mut self, picture: &PixelBuffer) -> Result<Instant, Error> {
        let (key_frame, started) = {
            let mut shared = self
                .shared
                .lock()
                .expect("the encoder's callback does not panic while it holds the lock");
            (shared.sink.key_frame_wanted(), shared.started)
        };
        let now = Instant::now();
        let pts_ns = (now.duration_since(started).as_nanos() as u64).max(self.last_pts_ns + 1);
        self.last_pts_ns = pts_ns;
        self.encoder.encode(picture, pts_ns, pts_ns, key_frame)?;
        Ok(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder::{Chroma, Feed, PixelBufferPool};
    use std::time::Duration;

    /// A sink that counts, and whose "a device is shown the session" can be set.
    struct Watched {
        viewed: Arc<Mutex<bool>>,
        frames: Arc<Mutex<u64>>,
    }

    impl FrameSink for Watched {
        fn send(&mut self, _frame: &EncodedFrame<'_>) -> Result<(), String> {
            *self.frames.lock().unwrap() += 1;
            Ok(())
        }
        fn key_frame_wanted(&mut self) -> bool {
            false
        }
        fn viewed(&self) -> bool {
            *self.viewed.lock().unwrap()
        }
    }

    fn settings() -> Settings {
        Settings {
            width: 320,
            height: 180,
            fps: 60,
            bitrate_bps: 2_000_000,
            chroma: Chroma::C420,
            feed: Feed::Direct,
            require_hardware: false,
            low_latency_rate_control: false,
            leave_unset: Vec::new(),
            extra: Vec::new(),
        }
    }

    /// Drives a feeder for `for_ms`: one picture at the start, then ticks.
    fn run(keep: KeepAwakeSettings, viewed: bool, for_ms: u64) -> Option<(FeedCounts, u64)> {
        let viewed = Arc::new(Mutex::new(viewed));
        let frames = Arc::new(Mutex::new(0));
        let sink = Watched {
            viewed: viewed.clone(),
            frames: frames.clone(),
        };
        let mut feeder = match Feeder::new(&settings(), keep, Box::new(sink)) {
            Ok(feeder) => feeder,
            Err(error) if error.what == "VTCompressionSessionCreate" => {
                eprintln!("no H.265 encoder on this machine ({error}); nothing was checked");
                return None;
            }
            Err(error) => panic!("{error}"),
        };
        let pool = PixelBufferPool::bgra(320, 180).unwrap();
        let mut picture = pool.next().unwrap();
        picture.with_bytes(|b, _| b.fill(0x40)).unwrap();
        feeder.picture(picture).unwrap();
        let end = Instant::now() + Duration::from_millis(for_ms);
        while Instant::now() < end {
            feeder.tick().unwrap();
            let wake = feeder.next_wake().unwrap_or(end).min(end);
            if let Some(wait) = wake.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
        feeder.finish().unwrap();
        let sent = *frames.lock().unwrap();
        Some((feeder.counts(), sent))
    }

    #[test]
    fn a_still_picture_is_handed_over_again_and_sent_while_a_device_is_shown_the_session() {
        let Some((counts, sent)) = run(KeepAwakeSettings::default(), true, 520) else {
            return;
        };
        assert_eq!(counts.pictures, 1);
        // Twenty a second for about half a second is ten, never more. How many come
        // depends on how promptly the machine wakes the thread and how fast its encoder
        // is (on the Air at nice 19 four; on a CI machine two), so only "some, and all
        // of them sent" is checked here; the beat itself is tested in keep_awake.
        assert!((1..=11).contains(&counts.repeats), "{counts:?}");
        assert_eq!(
            sent,
            counts.pictures + counts.repeats,
            "the repeats are sent too"
        );
    }

    #[test]
    fn nothing_is_repeated_while_no_device_is_shown_the_session() {
        let Some((counts, sent)) = run(KeepAwakeSettings::default(), false, 300) else {
            return;
        };
        assert_eq!((counts.pictures, counts.repeats, sent), (1, 0, 1));
    }

    #[test]
    fn nothing_is_repeated_when_it_is_switched_off() {
        let off = KeepAwakeSettings {
            enabled: false,
            ..KeepAwakeSettings::default()
        };
        let Some((counts, _)) = run(off, true, 300) else {
            return;
        };
        assert_eq!(counts.repeats, 0);
    }

    /// A sink that blocks in `send`, against the rule: it shows in the counts.
    struct Slow;

    impl FrameSink for Slow {
        fn send(&mut self, _frame: &EncodedFrame<'_>) -> Result<(), String> {
            std::thread::sleep(Duration::from_millis(30));
            Ok(())
        }
        fn key_frame_wanted(&mut self) -> bool {
            false
        }
    }

    #[test]
    fn a_sink_that_blocks_shows_in_the_longest_send() {
        let mut feeder =
            match Feeder::new(&settings(), KeepAwakeSettings::default(), Box::new(Slow)) {
                Ok(feeder) => feeder,
                Err(error) if error.what == "VTCompressionSessionCreate" => {
                    eprintln!("no H.265 encoder on this machine ({error}); nothing was checked");
                    return;
                }
                Err(error) => panic!("{error}"),
            };
        let pool = PixelBufferPool::bgra(320, 180).unwrap();
        let mut picture = pool.next().unwrap();
        picture.with_bytes(|b, _| b.fill(0x40)).unwrap();
        feeder.picture(picture).unwrap();
        feeder.input(InputKind::PointerMotion);
        feeder.finish().unwrap();
        let counts = feeder.counts();
        assert_eq!(counts.sent, 1);
        assert!(counts.longest_send_ns >= 30_000_000, "{counts:?}");
    }
}
