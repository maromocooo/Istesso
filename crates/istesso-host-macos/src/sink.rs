//! Where this crate meets the parts that are not in `main` yet.
//!
//! * [`FrameSink`] is what takes encoded frames. It has the shape of the
//!   host end of `istesso-media` (`Host::send_frame`, `Host::key_frame_wanted`
//!   on the branch of ADR 0015); the real thing will be a few lines that
//!   pass a frame on. Until then the sinks here stand in for it.
//! * [`FrameSource`] is what gives pictures to the encoder: a generated
//!   picture now, an application's window once capture may be used.

/// One encoded frame: an H.265 access unit in Annex B, a key frame with its
/// parameter sets in front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodedFrame<'a> {
    pub data: &'a [u8],
    /// Presentation time stamp, nanoseconds on this machine's clock.
    pub pts_ns: u64,
    /// When the encoder had finished the frame, on the same clock.
    pub encoded_ns: u64,
    pub key_frame: bool,
}

/// Takes encoded frames and says when a key frame is needed.
pub trait FrameSink: Send {
    /// Takes one frame. Must not wait for a viewer.
    fn send(&mut self, frame: &EncodedFrame<'_>) -> Result<(), String>;

    /// Whether a key frame is needed (a viewer has connected, asked for one,
    /// or was held back). True once for each time it is needed.
    fn key_frame_wanted(&mut self) -> bool;

    /// Whether a device is being shown the session now. The encoder is kept
    /// awake only then ([`crate::keep_awake`]). Sinks that cannot tell say yes.
    fn viewed(&self) -> bool {
        true
    }
}

/// A sink that counts and drops. For measuring the encoder alone.
#[derive(Debug, Default)]
pub struct Discard {
    pub frames: u64,
    pub bytes: u64,
    pub key_frames: u64,
    /// Set from outside to ask for a key frame once.
    pub want_key_frame: bool,
}

impl FrameSink for Discard {
    fn send(&mut self, frame: &EncodedFrame<'_>) -> Result<(), String> {
        self.frames += 1;
        self.bytes += frame.data.len() as u64;
        self.key_frames += u64::from(frame.key_frame);
        Ok(())
    }

    fn key_frame_wanted(&mut self) -> bool {
        std::mem::take(&mut self.want_key_frame)
    }
}

/// The form pixels are handed over in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelLayout {
    /// Four bytes a pixel: blue, green, red, alpha. Rows from the top.
    Bgra,
}

/// Gives the pictures of one application, one after the other.
pub trait FrameSource: Send {
    /// Size of the picture in its own pixels.
    fn size(&self) -> (usize, usize);

    fn layout(&self) -> PixelLayout {
        PixelLayout::Bgra
    }

    /// Writes the next picture into `dst` (rows `stride` bytes apart) and
    /// returns its number. `None` when there is none any more.
    fn next_into(&mut self, dst: &mut [u8], stride: usize) -> Option<u64>;
}

/// A source that draws scrolling text (see [`crate::picture`]).
pub struct Generated {
    text: crate::picture::ScrollingText,
    next: u64,
    /// Stops after this many frames; `None` goes on for ever.
    pub limit: Option<u64>,
}

impl Generated {
    pub fn new(width: usize, height: usize) -> Self {
        Generated {
            text: crate::picture::ScrollingText::new(width, height),
            next: 0,
            limit: None,
        }
    }
}

impl FrameSource for Generated {
    fn size(&self) -> (usize, usize) {
        (self.text.width(), self.text.height())
    }

    fn next_into(&mut self, dst: &mut [u8], stride: usize) -> Option<u64> {
        if self.limit.is_some_and(|limit| self.next >= limit) {
            return None;
        }
        let number = self.next;
        self.text.fill(number, dst, stride);
        self.next += 1;
        Some(number)
    }
}

/// What changes in a picture that differs from the one before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// The text scrolls by a few rows: nearly every pixel changes.
    Scroll,
    /// One more character appears on the bottom line.
    Type,
}

/// A source paced by a [`Pace`](crate::pace::Pace): the picture changes only
/// where the pace says so (a probe), and stays the same in between.
pub struct Paced {
    text: crate::picture::ScrollingText,
    pace: crate::pace::Pace,
    change: Change,
    next: u64,
    changes: u64,
    /// Stops after this many changed pictures; `None` goes on for ever.
    pub probes: Option<u64>,
}

impl Paced {
    pub fn new(width: usize, height: usize, pace: crate::pace::Pace, change: Change) -> Self {
        Paced {
            text: crate::picture::ScrollingText::new(width, height),
            pace,
            change,
            next: 0,
            changes: 0,
            probes: None,
        }
    }
}

impl FrameSource for Paced {
    fn size(&self) -> (usize, usize) {
        (self.text.width(), self.text.height())
    }

    fn next_into(&mut self, dst: &mut [u8], stride: usize) -> Option<u64> {
        if self.probes.is_some_and(|probes| self.changes >= probes) {
            return None;
        }
        let number = self.next;
        if self.pace.is_probe(number) {
            self.changes += 1;
        }
        match self.change {
            Change::Scroll => self.text.fill(self.changes, dst, stride),
            Change::Type => self.text.fill_typed(0, self.changes, dst, stride),
        }
        self.next += 1;
        Some(number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pace::Pace;
    use std::time::Duration;

    #[test]
    fn a_discarding_sink_counts() {
        let mut sink = Discard::default();
        let frame = EncodedFrame {
            data: &[1, 2, 3],
            pts_ns: 1,
            encoded_ns: 2,
            key_frame: true,
        };
        sink.send(&frame).unwrap();
        sink.send(&EncodedFrame {
            key_frame: false,
            ..frame
        })
        .unwrap();
        assert_eq!((sink.frames, sink.bytes, sink.key_frames), (2, 6, 1));
    }

    #[test]
    fn a_key_frame_is_wanted_once_for_each_time_it_is_asked() {
        let mut sink = Discard::default();
        assert!(!sink.key_frame_wanted());
        sink.want_key_frame = true;
        assert!(sink.key_frame_wanted());
        assert!(!sink.key_frame_wanted());
    }

    #[test]
    fn a_generated_source_numbers_its_frames_and_stops_at_its_limit() {
        let mut source = Generated::new(64, 40);
        source.limit = Some(2);
        let mut buffer = vec![0u8; 64 * 4 * 40];
        assert_eq!(source.next_into(&mut buffer, 64 * 4), Some(0));
        assert_eq!(source.next_into(&mut buffer, 64 * 4), Some(1));
        assert_eq!(source.next_into(&mut buffer, 64 * 4), None);
        assert_eq!(source.size(), (64, 40));
    }

    #[test]
    fn a_paced_source_changes_its_picture_only_on_probes() {
        let pace = Pace::Sparse {
            still: Duration::from_millis(300),
            awake_fps: 10,
        };
        for change in [Change::Scroll, Change::Type] {
            let mut source = Paced::new(320, 200, pace, change);
            source.probes = Some(2);
            let stride = 320 * 4;
            let mut pictures = Vec::new();
            loop {
                let mut buffer = vec![0u8; stride * 200];
                let Some(number) = source.next_into(&mut buffer, stride) else {
                    break;
                };
                pictures.push((number, buffer));
            }
            // Two unchanged pictures and a probe, twice.
            let numbers: Vec<u64> = pictures.iter().map(|(n, _)| *n).collect();
            assert_eq!(numbers, vec![0, 1, 2, 3, 4, 5], "{change:?}");
            assert!(pictures[0].1 == pictures[1].1, "{change:?}");
            assert!(pictures[1].1 != pictures[2].1, "{change:?}");
            assert!(pictures[2].1 == pictures[3].1 && pictures[3].1 == pictures[4].1);
            assert!(pictures[4].1 != pictures[5].1, "{change:?}");
        }
    }
}
