//! The queue between the thread that receives packets and the thread of the
//! sound card.
//!
//! Neither side ever waits for the other: the samples and a note about each
//! packet ([`Mark`]) travel through two lock-free rings. The receiving thread
//! owns the [`Writer`], the sound card's thread owns the [`Reader`].
//!
//! Frames are counted in two ways. The *position* is the source's count and
//! jumps when the source lets audio go. The *index* counts the frames that
//! were put into this queue, without gaps; it is what tells how many frames
//! lie ahead of a packet.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rtrb::{Consumer, Producer, RingBuffer};

/// Notes the queue can hold. A second of audio in the smallest packets a
/// source sends is far fewer.
const MARKS: usize = 8192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkKind {
    Audio,
    /// The source said that it has nothing to play.
    Idle,
}

/// What is known about one packet in the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    pub kind: MarkKind,
    /// The audio does not follow on from what came before it.
    pub discontinuity: bool,
    pub frames: u32,
    /// Counts the notes, so that each is looked at once.
    pub serial: u64,
    /// Index of the first frame among all frames put into the queue.
    pub index: u64,
    /// Position of the first frame in the source's stream.
    pub position: u64,
    /// When the source captured the first frame (the source's clock).
    pub capture_ns: u64,
    /// When the packet arrived here (this machine's clock).
    pub arrival_ns: u64,
}

struct Shared {
    /// Frames that were put in.
    written: AtomicU64,
    /// Frames that found the queue full.
    refused: AtomicU64,
}

/// The receiving end's side of the queue.
pub struct Writer {
    samples: Producer<f32>,
    marks: Producer<Mark>,
    channels: usize,
    written: u64,
    serial: u64,
    /// Audio was refused since the last packet that went in.
    gap: bool,
    shared: Arc<Shared>,
}

/// The sound card's side of the queue.
pub struct Reader {
    samples: Consumer<f32>,
    marks: Consumer<Mark>,
    channels: usize,
    /// Frames taken out, read or discarded.
    taken: u64,
    /// The packet the next frame comes from, and how many of its frames are
    /// left.
    head: Option<Mark>,
    head_left: u32,
    /// The packet at the head starts after a break, and nothing of it has
    /// been taken yet.
    boundary: bool,
    /// The source said it has nothing to play, and no audio came after.
    idle: bool,
    /// Serial of the first note that has not been shown to `scan_new`.
    unseen: u64,
    shared: Arc<Shared>,
}

/// A queue for `capacity_frames` frames of `channels` channels.
///
/// With no channels the queue carries no samples and only counts frames;
/// tests use that to run hours of audio in seconds.
pub fn queue(channels: usize, capacity_frames: usize) -> (Writer, Reader) {
    let (samples_in, samples_out) = RingBuffer::new(capacity_frames * channels);
    let (marks_in, marks_out) = RingBuffer::new(MARKS);
    let shared = Arc::new(Shared {
        written: AtomicU64::new(0),
        refused: AtomicU64::new(0),
    });
    (
        Writer {
            samples: samples_in,
            marks: marks_in,
            channels,
            written: 0,
            serial: 0,
            gap: false,
            shared: shared.clone(),
        },
        Reader {
            samples: samples_out,
            marks: marks_out,
            channels,
            taken: 0,
            head: None,
            head_left: 0,
            boundary: false,
            idle: false,
            unseen: 0,
            shared,
        },
    )
}

impl Writer {
    /// Puts the audio of one packet into the queue. `samples` holds
    /// `frames` frames.
    ///
    /// Returns `false` when there was no room (the sound card's side has
    /// stopped taking audio out). The packet is then dropped, and the next
    /// one that fits is marked as not following on.
    pub fn push_audio(
        &mut self,
        frames: usize,
        samples: &[f32],
        discontinuity: bool,
        position: u64,
        capture_ns: u64,
        arrival_ns: u64,
    ) -> bool {
        debug_assert_eq!(samples.len(), frames * self.channels);
        if frames == 0 {
            return true;
        }
        if self.marks.is_full() || self.samples.slots() < samples.len() {
            self.shared
                .refused
                .fetch_add(frames as u64, Ordering::Relaxed);
            self.gap = true;
            return false;
        }
        let mark = Mark {
            kind: MarkKind::Audio,
            discontinuity: discontinuity || self.gap,
            frames: frames as u32,
            serial: self.serial,
            index: self.written,
            position,
            capture_ns,
            arrival_ns,
        };
        // The note first, the samples second, the count last: the reader
        // never sees a count that its two rings cannot honour.
        if self.marks.push(mark).is_err() {
            return false;
        }
        if !samples.is_empty() && self.samples.push_entire_slice(samples).is_err() {
            // Cannot happen: the room was checked, and only this side adds.
            return false;
        }
        self.gap = false;
        self.serial += 1;
        self.written += frames as u64;
        self.shared.written.store(self.written, Ordering::Release);
        true
    }

    /// Notes that the source has nothing to play.
    pub fn push_idle(&mut self, arrival_ns: u64) {
        let mark = Mark {
            kind: MarkKind::Idle,
            discontinuity: false,
            frames: 0,
            serial: self.serial,
            index: self.written,
            position: 0,
            capture_ns: 0,
            arrival_ns,
        };
        if self.marks.push(mark).is_ok() {
            self.serial += 1;
        }
    }

    /// Frames put into the queue so far.
    pub fn written(&self) -> u64 {
        self.written
    }

    /// Frames that were dropped because the queue was full.
    pub fn refused(&self) -> u64 {
        self.shared.refused.load(Ordering::Relaxed)
    }
}

impl Reader {
    /// Frames that can be taken out now.
    pub fn available(&self) -> usize {
        (self.shared.written.load(Ordering::Acquire) - self.taken) as usize
    }

    /// Frames taken out so far, read or discarded. Equal to the index of the
    /// next frame.
    pub fn taken(&self) -> u64 {
        self.taken
    }

    /// Frames that were dropped because the queue was full.
    pub fn refused(&self) -> u64 {
        self.shared.refused.load(Ordering::Relaxed)
    }

    /// True when the source said that it has nothing to play, and no audio
    /// has come after that.
    pub fn idle(&mut self) -> bool {
        self.settle();
        self.idle
    }

    /// True when the next frame is the first one after a break in the
    /// stream. Stays true until [`Reader::clear_boundary`].
    pub fn at_boundary(&mut self) -> bool {
        self.settle();
        self.boundary
    }

    pub fn clear_boundary(&mut self) {
        self.boundary = false;
    }

    /// Makes `head` the packet the next frame belongs to.
    fn settle(&mut self) {
        while self.head_left == 0 {
            match self.marks.pop() {
                Ok(mark) => {
                    // A note that leaves the queue before it was looked at
                    // is not looked at any more.
                    self.unseen = self.unseen.max(mark.serial + 1);
                    match mark.kind {
                        MarkKind::Idle => {
                            self.idle = true;
                            self.head = None;
                        }
                        MarkKind::Audio => {
                            self.idle = false;
                            self.boundary = mark.discontinuity;
                            self.head_left = mark.frames;
                            self.head = Some(mark);
                        }
                    }
                }
                Err(_) => {
                    self.head = None;
                    return;
                }
            }
        }
    }

    /// Takes `frames` frames out, into `out`. There must be that many
    /// ([`Reader::available`]).
    pub fn read(&mut self, frames: usize, out: &mut [f32]) {
        debug_assert_eq!(out.len(), frames * self.channels);
        debug_assert!(frames <= self.available());
        if self.channels > 0 && self.samples.pop_entire_slice(out).is_err() {
            // Cannot happen while the count is honoured; play silence
            // rather than what was in the buffer.
            out.fill(0.0);
        }
        self.advance(frames);
    }

    /// Takes `frames` frames out and throws them away. Takes fewer when
    /// there are fewer; returns how many.
    pub fn discard(&mut self, frames: usize) -> usize {
        let frames = frames.min(self.available());
        if self.channels > 0 {
            if let Ok(chunk) = self.samples.read_chunk(frames * self.channels) {
                chunk.commit_all();
            }
        }
        self.advance(frames);
        frames
    }

    fn advance(&mut self, frames: usize) {
        let mut left = frames as u64;
        self.taken += left;
        while left > 0 {
            self.settle();
            if self.head_left == 0 {
                // The count ran ahead of the notes; nothing more to walk.
                return;
            }
            let step = left.min(u64::from(self.head_left));
            self.head_left -= step as u32;
            left -= step;
            if step > 0 {
                self.boundary = false;
            }
        }
    }

    /// How many frames can be taken before the stream breaks, counting no
    /// further than `limit`.
    pub fn contiguous(&mut self, limit: usize) -> usize {
        self.settle();
        let limit = limit.min(self.available());
        let mut run = self.head_left as usize;
        if run >= limit {
            return limit;
        }
        let Ok(chunk) = self.marks.read_chunk(self.marks.slots()) else {
            return run;
        };
        let (first, second) = chunk.as_slices();
        for mark in first.iter().chain(second) {
            if mark.kind == MarkKind::Idle {
                continue;
            }
            if mark.discontinuity {
                break;
            }
            run += mark.frames as usize;
            if run >= limit {
                break;
            }
        }
        // Dropping the chunk without committing leaves the notes in place.
        run.min(limit)
    }

    /// Shows every audio packet that arrived since the last call, newest
    /// first.
    pub fn scan_new(&mut self, mut each: impl FnMut(&Mark)) {
        let Ok(chunk) = self.marks.read_chunk(self.marks.slots()) else {
            return;
        };
        let (first, second) = chunk.as_slices();
        let mut newest = None;
        for mark in second.iter().rev().chain(first.iter().rev()) {
            if mark.serial < self.unseen {
                break;
            }
            newest = newest.max(Some(mark.serial));
            if mark.kind == MarkKind::Audio {
                each(mark);
            }
        }
        if let Some(serial) = newest {
            self.unseen = serial + 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(first: f32, count: usize) -> Vec<f32> {
        // Two channels; the left one counts up.
        (0..count)
            .flat_map(|i| [first + i as f32, -(first + i as f32)])
            .collect()
    }

    #[test]
    fn audio_comes_out_in_the_order_it_went_in() {
        let (mut writer, mut reader) = queue(2, 64);
        assert_eq!(reader.available(), 0);
        assert!(writer.push_audio(4, &frames(0.0, 4), false, 100, 1, 10));
        assert!(writer.push_audio(4, &frames(4.0, 4), false, 104, 2, 20));
        assert_eq!(reader.available(), 8);
        assert_eq!(writer.written(), 8);

        // A read may end in the middle of a packet and span two.
        let mut out = vec![0.0; 12];
        reader.read(6, &mut out);
        assert_eq!(out, frames(0.0, 6));
        assert_eq!(reader.available(), 2);
        assert_eq!(reader.taken(), 6);
        let mut out = vec![0.0; 4];
        reader.read(2, &mut out);
        assert_eq!(out, frames(6.0, 2));
        assert_eq!(reader.available(), 0);
    }

    #[test]
    fn a_full_queue_drops_the_packet_and_marks_the_next() {
        let (mut writer, mut reader) = queue(2, 8);
        assert!(writer.push_audio(6, &frames(0.0, 6), false, 0, 0, 0));
        assert!(!writer.push_audio(4, &frames(6.0, 4), false, 6, 0, 0));
        assert_eq!(writer.refused(), 4);
        assert_eq!(reader.available(), 6);

        let mut out = vec![0.0; 12];
        reader.read(6, &mut out);
        assert!(writer.push_audio(4, &frames(10.0, 4), false, 10, 0, 0));
        // The audio in between is missing, and the reader is told.
        assert!(reader.at_boundary());
        assert_eq!(reader.contiguous(4), 4);
        reader.clear_boundary();
        assert!(!reader.at_boundary());
    }

    #[test]
    fn a_break_in_the_stream_is_found_before_it_is_read() {
        let (mut writer, mut reader) = queue(1, 64);
        writer.push_audio(4, &[0.0; 4], false, 0, 0, 0);
        writer.push_audio(4, &[0.0; 4], false, 4, 0, 0);
        writer.push_audio(4, &[1.0; 4], true, 100, 0, 0);
        writer.push_audio(4, &[1.0; 4], false, 104, 0, 0);

        assert!(!reader.at_boundary());
        // Eight frames follow on from each other, then the stream breaks.
        assert_eq!(reader.contiguous(6), 6);
        assert_eq!(reader.contiguous(100), 8);
        let mut out = [9.0; 3];
        reader.read(3, &mut out);
        assert_eq!(reader.contiguous(100), 5);
        assert!(!reader.at_boundary());

        // Letting the rest of the old audio go ends exactly at the break.
        assert_eq!(reader.discard(5), 5);
        assert!(reader.at_boundary());
        assert_eq!(reader.contiguous(100), 8);
        let mut out = [9.0; 2];
        reader.read(2, &mut out);
        assert_eq!(out, [1.0, 1.0]);
        // Once frames after the break are taken, it lies behind.
        assert!(!reader.at_boundary());
    }

    #[test]
    fn discarding_takes_no_more_than_there_is() {
        let (mut writer, mut reader) = queue(2, 64);
        writer.push_audio(4, &frames(0.0, 4), false, 0, 0, 0);
        assert_eq!(reader.discard(10), 4);
        assert_eq!(reader.available(), 0);
        assert_eq!(reader.taken(), 4);
        writer.push_audio(2, &frames(4.0, 2), false, 4, 0, 0);
        let mut out = vec![0.0; 4];
        reader.read(2, &mut out);
        assert_eq!(out, frames(4.0, 2));
    }

    #[test]
    fn each_arrival_is_shown_once() {
        let (mut writer, mut reader) = queue(1, 64);
        let mut seen: Vec<(u64, u64)> = Vec::new();
        reader.scan_new(|mark| seen.push((mark.index, mark.arrival_ns)));
        assert!(seen.is_empty());

        writer.push_audio(4, &[0.0; 4], false, 0, 0, 11);
        writer.push_idle(12);
        writer.push_audio(4, &[0.0; 4], false, 4, 0, 13);
        reader.scan_new(|mark| seen.push((mark.index, mark.arrival_ns)));
        assert_eq!(seen, [(4, 13), (0, 11)]);

        seen.clear();
        reader.scan_new(|mark| seen.push((mark.index, mark.arrival_ns)));
        assert!(seen.is_empty());

        // Reading does not make a packet new again, and a packet that was
        // read before anyone looked is not shown later.
        writer.push_audio(4, &[0.0; 4], false, 8, 0, 14);
        let mut out = [0.0; 12];
        reader.read(12, &mut out);
        writer.push_audio(4, &[0.0; 4], false, 12, 0, 15);
        reader.scan_new(|mark| seen.push((mark.index, mark.arrival_ns)));
        assert_eq!(seen, [(12, 15)]);
    }

    #[test]
    fn idle_is_known_once_the_audio_before_it_is_gone() {
        let (mut writer, mut reader) = queue(1, 64);
        writer.push_audio(4, &[0.0; 4], false, 0, 0, 0);
        writer.push_idle(5);
        assert!(!reader.idle());
        let mut out = [0.0; 4];
        reader.read(4, &mut out);
        assert!(reader.idle());
        // Audio after it ends the idleness.
        writer.push_audio(4, &[0.0; 4], false, 4, 0, 0);
        assert!(!reader.idle());
    }

    #[test]
    fn without_channels_only_frames_are_counted() {
        let (mut writer, mut reader) = queue(0, 1 << 20);
        for packet in 0..1000u64 {
            assert!(writer.push_audio(256, &[], false, packet * 256, 0, packet));
        }
        assert_eq!(reader.available(), 256_000);
        reader.read(1000, &mut []);
        assert_eq!(reader.discard(55_000), 55_000);
        assert_eq!(reader.available(), 200_000);
        assert_eq!(reader.contiguous(300), 300);
    }

    #[test]
    fn the_two_sides_can_be_on_different_threads() {
        let (mut writer, mut reader) = queue(1, 4096);
        let producer = std::thread::spawn(move || {
            let mut value = 0.0f32;
            let mut sent = 0u64;
            while sent < 200_000 {
                let packet: Vec<f32> = (0..64).map(|i| value + i as f32).collect();
                if writer.push_audio(64, &packet, false, sent, 0, 0) {
                    value += 64.0;
                    sent += 64;
                } else {
                    std::thread::yield_now();
                }
            }
        });
        let mut expected = 0.0f32;
        let mut out = [0.0f32; 48];
        let mut received = 0u64;
        while received < 200_000 {
            let n = reader.available().min(48);
            if n == 0 {
                std::thread::yield_now();
                continue;
            }
            reader.read(n, &mut out[..n]);
            for sample in &out[..n] {
                assert_eq!(*sample, expected);
                expected += 1.0;
            }
            received += n as u64;
        }
        producer.join().unwrap();
    }
}
