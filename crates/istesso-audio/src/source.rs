//! The source's side: from the thread that captures audio to packets.
//!
//! The capturing thread belongs to the sound system and must never wait,
//! least of all for a network. It only writes into a ring
//! ([`CaptureWriter`]). A second thread takes the audio out again, one
//! packet at a time ([`Packetizer`]), and sends it.
//!
//! When sending stalls, audio piles up in the ring. Sending all of it later
//! would make the exit play late for ever after, so the packetizer lets the
//! oldest audio go instead. Positions keep counting what was captured, so
//! the exit sees the jump and knows that audio is missing.

use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rtrb::{Consumer, Producer, RingBuffer};

use crate::probe::Tone;
use crate::wire::{Header, Kind, MAX_PACKET_FRAMES};

/// Pieces of captured audio the ring can describe.
const QUANTA: usize = 4096;

/// One piece of audio as it was captured.
#[derive(Debug, Clone, Copy)]
struct Quantum {
    position: u64,
    frames: u32,
    capture_ns: u64,
}

/// The capturing thread's end of the ring.
pub struct CaptureWriter {
    samples: Producer<f32>,
    quanta: Producer<Quantum>,
    channels: usize,
    rate: f64,
    /// Frames captured so far, whether or not they fitted into the ring.
    position: u64,
    dropped: Arc<AtomicU64>,
}

/// The sending thread's end of the ring.
pub struct Packetizer {
    samples: Consumer<f32>,
    quanta: Consumer<Quantum>,
    channels: usize,
    seq: u32,
    /// Position the next packet has if nothing is missing.
    expected: Option<u64>,
    max_backlog_frames: usize,
    keep_frames: usize,
    packet: Vec<f32>,
    trimmed_frames: u64,
    breaks: u64,
    dropped: Arc<AtomicU64>,
}

/// What the source's side let go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct SourceStats {
    pub packets: u64,
    /// Frames the capturing thread could not put into the ring.
    pub dropped_frames: u64,
    /// Frames let go because sending had fallen behind.
    pub trimmed_frames: u64,
    /// Packets marked as not following on from the one before.
    pub breaks: u64,
}

/// A ring for `capacity_s` seconds of audio. When more than `max_backlog_s`
/// seconds wait to be sent, the oldest audio is let go until `keep_s`
/// seconds are left.
pub fn capture_ring(
    channels: usize,
    rate: f64,
    capacity_s: f64,
    max_backlog_s: f64,
    keep_s: f64,
) -> (CaptureWriter, Packetizer) {
    assert!(channels > 0, "audio has at least one channel");
    let frames = |seconds: f64| (seconds * rate) as usize;
    let (samples_in, samples_out) = RingBuffer::new(frames(capacity_s) * channels);
    let (quanta_in, quanta_out) = RingBuffer::new(QUANTA);
    let dropped = Arc::new(AtomicU64::new(0));
    (
        CaptureWriter {
            samples: samples_in,
            quanta: quanta_in,
            channels,
            rate,
            position: 0,
            dropped: dropped.clone(),
        },
        Packetizer {
            samples: samples_out,
            quanta: quanta_out,
            channels,
            seq: 0,
            expected: None,
            max_backlog_frames: frames(max_backlog_s),
            keep_frames: frames(keep_s),
            packet: Vec::with_capacity(MAX_PACKET_FRAMES * channels),
            trimmed_frames: 0,
            breaks: 0,
            dropped,
        },
    )
}

impl CaptureWriter {
    /// Hands over captured frames (channels interleaved). `capture_ns` is
    /// when the first of them was captured. Never waits: frames that do not
    /// fit are dropped, and `false` is returned.
    pub fn push(&mut self, samples: &[f32], capture_ns: u64) -> bool {
        let mut fitted = true;
        let mut capture_ns = capture_ns;
        for piece in samples.chunks(MAX_PACKET_FRAMES * self.channels) {
            let frames = piece.len() / self.channels;
            let position = self.position;
            self.position += frames as u64;
            if self.quanta.is_full() || self.samples.slots() < piece.len() {
                self.dropped.fetch_add(frames as u64, Ordering::Relaxed);
                fitted = false;
            } else {
                // The description first: the reader waits for the samples
                // that belong to it.
                let quantum = Quantum {
                    position,
                    frames: frames as u32,
                    capture_ns,
                };
                if self.quanta.push(quantum).is_ok() {
                    let _ = self.samples.push_entire_slice(piece);
                }
            }
            capture_ns += (frames as f64 / self.rate * 1e9) as u64;
        }
        fitted
    }

    /// Frames captured so far.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Channels per frame.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Frames per second of the audio handed over.
    pub fn rate(&self) -> f64 {
        self.rate
    }
}

impl Packetizer {
    /// Frames that wait to be sent.
    pub fn backlog_frames(&self) -> usize {
        self.samples.slots() / self.channels
    }

    /// The next packet, if a whole one has been captured.
    pub fn next_packet(&mut self) -> Option<(Header, &[f32])> {
        if self.backlog_frames() > self.max_backlog_frames {
            self.let_go(self.keep_frames, true);
        }
        let quantum = *self.quanta.peek().ok()?;
        let len = quantum.frames as usize * self.channels;
        if self.samples.slots() < len {
            // Described already, but its samples are still being written.
            return None;
        }
        let _ = self.quanta.pop();
        self.packet.resize(len, 0.0);
        if self.samples.pop_entire_slice(&mut self.packet).is_err() {
            return None;
        }
        let discontinuity = self
            .expected
            .is_some_and(|expected| expected != quantum.position);
        if discontinuity {
            self.breaks += 1;
        }
        self.expected = Some(quantum.position + u64::from(quantum.frames));
        let header = Header {
            kind: Kind::Audio,
            discontinuity,
            seq: self.next_seq(),
            position: quantum.position,
            capture_ns: quantum.capture_ns,
        };
        Some((header, &self.packet))
    }

    /// A packet that says the source is there but has nothing to play.
    pub fn idle(&mut self, now_ns: u64) -> Header {
        self.without_audio(Kind::Idle, now_ns)
    }

    /// A packet that says the source stops sending on this connection.
    pub fn end(&mut self, now_ns: u64) -> Header {
        self.without_audio(Kind::End, now_ns)
    }

    /// The same word for a listener that another one has replaced. From the
    /// moment the new listener asked, the sequence is the new one's: a
    /// number taken here would be a hole in what the new one receives, and
    /// it would take that for a lost packet. So this packet takes none; it
    /// carries the number after the last one this listener was sent.
    pub fn end_for_replaced(&self, last_sent: Option<u32>, now_ns: u64) -> Header {
        Header {
            kind: Kind::End,
            discontinuity: false,
            seq: last_sent.map_or(0, |seq| seq.wrapping_add(1)),
            position: self.expected.unwrap_or(0),
            capture_ns: now_ns,
        }
    }

    fn without_audio(&mut self, kind: Kind, now_ns: u64) -> Header {
        Header {
            kind,
            discontinuity: false,
            seq: self.next_seq(),
            position: self.expected.unwrap_or(0),
            capture_ns: now_ns,
        }
    }

    fn next_seq(&mut self) -> u32 {
        let seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        seq
    }

    /// Lets go of everything captured so far. A listener that has just
    /// arrived is given what is captured from now on; and while nobody
    /// listens, nothing needs to be kept. This is not counted as audio that
    /// had to be let go.
    pub fn flush(&mut self) {
        self.let_go(0, false);
    }

    /// Lets the oldest audio go until at most `keep` frames wait.
    fn let_go(&mut self, keep: usize, count: bool) {
        while self.backlog_frames() > keep {
            let Ok(quantum) = self.quanta.peek().copied() else {
                return;
            };
            let len = quantum.frames as usize * self.channels;
            let Ok(chunk) = self.samples.read_chunk(len) else {
                return;
            };
            chunk.commit_all();
            let _ = self.quanta.pop();
            if count {
                self.trimmed_frames += u64::from(quantum.frames);
            }
        }
    }

    pub fn stats(&self) -> SourceStats {
        SourceStats {
            packets: u64::from(self.seq),
            dropped_frames: self.dropped.load(Ordering::Relaxed),
            trimmed_frames: self.trimmed_frames,
            breaks: self.breaks,
        }
    }
}

/// Feeds a test tone into the ring at the pace of this machine's clock, in
/// pieces of `quantum` frames, until `stop` is set or `seconds` have passed.
///
/// The pace is that of the system clock, not of a sound card: an exit that
/// plays this through a real device has two different clocks to keep
/// together, as it has with a real source.
pub fn feed_tone(
    writer: &mut CaptureWriter,
    tone: &mut Tone,
    quantum: usize,
    seconds: Option<f64>,
    stop: &AtomicBool,
) {
    let channels = writer.channels;
    let rate = writer.rate;
    let piece_s = quantum as f64 / rate;
    let piece_ns = (piece_s * 1e9) as u64;
    let mut samples = vec![0.0f32; quantum * channels];
    let begin = Instant::now();
    let begin_ns = crate::clock::now_ns();
    let mut pieces = 0u64;
    while !stop.load(Ordering::Relaxed) {
        // A piece exists once its last frame does.
        let due = begin + Duration::from_secs_f64((pieces + 1) as f64 * piece_s);
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        tone.fill(&mut samples, channels);
        // Stamped with the time the piece belongs to, not the time this
        // thread got round to it: a sound card's clock does not hesitate
        // when a thread is late.
        writer.push(&samples, begin_ns + pieces * piece_ns);
        pieces += 1;
        if seconds.is_some_and(|limit| pieces as f64 * piece_s >= limit) {
            break;
        }
    }
}

/// Feeds raw audio read from `input` into the ring: 32-bit floats,
/// little-endian, channels interleaved, in pieces of `quantum` frames.
/// Returns when the input ends or `stop` is set.
///
/// This is how a program that captures audio and writes it to a pipe can
/// stand in for a capture that is built in.
pub fn feed_reader(
    writer: &mut CaptureWriter,
    mut input: impl Read,
    quantum: usize,
    stop: &AtomicBool,
) -> std::io::Result<()> {
    let channels = writer.channels;
    let piece_ns = (quantum as f64 / writer.rate * 1e9) as u64;
    let mut bytes = vec![0u8; quantum * channels * 4];
    let mut samples = vec![0.0f32; quantum * channels];
    while !stop.load(Ordering::Relaxed) {
        match input.read_exact(&mut bytes) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(err) => return Err(err),
        }
        let captured = crate::clock::now_ns().saturating_sub(piece_ns);
        for (sample, raw) in samples.iter_mut().zip(bytes.chunks_exact(4)) {
            let value = f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
            *sample = if value.is_finite() { value } else { 0.0 };
        }
        writer.push(&samples, captured);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring() -> (CaptureWriter, Packetizer) {
        // A second of room; more than 100 ms waiting is too much, and 20 ms
        // are kept then.
        capture_ring(2, 48_000.0, 1.0, 0.1, 0.02)
    }

    fn piece(value: f32, frames: usize) -> Vec<f32> {
        vec![value; frames * 2]
    }

    #[test]
    fn captured_pieces_become_packets_in_order() {
        let (mut writer, mut packets) = ring();
        assert!(packets.next_packet().is_none());
        writer.push(&piece(0.1, 256), 1000);
        writer.push(&piece(0.2, 256), 2000);
        assert_eq!(packets.backlog_frames(), 512);

        let (header, samples) = packets.next_packet().unwrap();
        assert_eq!(header.kind, Kind::Audio);
        assert_eq!(
            (header.seq, header.position, header.capture_ns),
            (0, 0, 1000)
        );
        assert!(!header.discontinuity);
        assert_eq!(samples, piece(0.1, 256));

        let (header, samples) = packets.next_packet().unwrap();
        assert_eq!(
            (header.seq, header.position, header.capture_ns),
            (1, 256, 2000)
        );
        assert!(!header.discontinuity);
        assert_eq!(samples, piece(0.2, 256));
        assert!(packets.next_packet().is_none());
        assert_eq!(packets.stats().packets, 2);
    }

    #[test]
    fn a_stalled_sender_lets_the_oldest_audio_go() {
        let (mut writer, mut packets) = ring();
        writer.push(&piece(0.0, 256), 0);
        let (first, _) = packets.next_packet().unwrap();
        assert_eq!(first.position, 0);

        // Sending stalls for 300 ms while capturing goes on.
        for n in 1..=56u64 {
            writer.push(&piece(n as f32, 256), n);
        }
        let (header, samples) = packets.next_packet().unwrap();
        // What is sent next is recent, and marked as not following on.
        assert!(header.discontinuity);
        assert!(header.position >= 56 * 256 - 960 - 256, "{header:?}");
        assert_eq!(samples[0], (header.position / 256) as f32);
        assert_eq!(header.seq, 1);
        let stats = packets.stats();
        assert!(stats.trimmed_frames >= 50 * 256, "{stats:?}");
        assert_eq!(stats.breaks, 1);
        assert_eq!(stats.dropped_frames, 0);

        // From there on the stream follows on again.
        let (header, _) = packets.next_packet().unwrap();
        assert!(!header.discontinuity);
    }

    #[test]
    fn capturing_never_waits_for_a_full_ring() {
        let (mut writer, mut packets) = ring();
        // Nobody takes anything out for two seconds.
        let mut fitted = 0;
        for n in 0..375u64 {
            if writer.push(&piece(n as f32, 256), n) {
                fitted += 1;
            }
        }
        assert!((180..=187).contains(&fitted), "{fitted}");
        assert_eq!(writer.position(), 375 * 256);
        // Positions go on counting, so the loss shows.
        let stats = packets.stats();
        assert_eq!(stats.dropped_frames, (375 - fitted) * 256);
        let (header, _) = packets.next_packet().unwrap();
        assert!(!header.discontinuity, "the very first packet joins nothing");
        writer.push(&piece(1.0, 256), 0);
        let mut last = header;
        while let Some((header, _)) = packets.next_packet() {
            last = header;
        }
        assert!(last.discontinuity);
        assert_eq!(last.position, 375 * 256);
    }

    #[test]
    fn a_new_listener_starts_with_what_comes_now() {
        let (mut writer, mut packets) = ring();
        for n in 0..10u64 {
            writer.push(&piece(n as f32, 256), n);
        }
        packets.flush();
        assert!(packets.next_packet().is_none());
        writer.push(&piece(10.0, 256), 10);
        let (header, samples) = packets.next_packet().unwrap();
        assert_eq!(header.position, 2560);
        assert_eq!(samples[0], 10.0);
    }

    #[test]
    fn the_end_for_a_replaced_listener_takes_no_number() {
        let (mut writer, mut packets) = ring();
        writer.push(&piece(0.0, 256), 0);
        let (first, _) = packets.next_packet().unwrap();
        let first_seq = first.seq;
        let end = packets.end_for_replaced(Some(first_seq), 77);
        assert_eq!(
            (end.kind, end.seq, end.position),
            (Kind::End, first_seq + 1, 256)
        );
        // The successor's next packet follows on from the last audio.
        let idle = packets.idle(78);
        assert_eq!(idle.seq, first_seq + 1);
    }

    #[test]
    fn idle_and_end_take_their_place_in_the_sequence() {
        let (mut writer, mut packets) = ring();
        writer.push(&piece(0.0, 256), 0);
        packets.next_packet().unwrap();
        let idle = packets.idle(77);
        assert_eq!((idle.kind, idle.seq, idle.position), (Kind::Idle, 1, 256));
        assert_eq!(idle.capture_ns, 77);
        let end = packets.end(78);
        assert_eq!((end.kind, end.seq), (Kind::End, 2));
    }

    #[test]
    fn large_pieces_are_cut_into_packets() {
        let (mut writer, mut packets) = capture_ring(1, 48_000.0, 1.0, 0.9, 0.5);
        let samples: Vec<f32> = (0..20_000).map(|i| i as f32).collect();
        assert!(writer.push(&samples, 0));
        let mut position = 0u64;
        while let Some((header, piece)) = packets.next_packet() {
            assert!(piece.len() <= MAX_PACKET_FRAMES);
            assert_eq!(header.position, position);
            assert_eq!(piece[0], position as f32);
            assert!(!header.discontinuity);
            position += piece.len() as u64;
        }
        assert_eq!(position, 20_000);
    }

    #[test]
    fn raw_audio_from_a_pipe_is_cut_into_pieces() {
        let (mut writer, mut packets) = ring();
        // Three pieces and half of a fourth.
        let mut bytes = Vec::new();
        for i in 0..(3 * 256 * 2 + 256) {
            bytes.extend_from_slice(&(i as f32).to_le_bytes());
        }
        bytes[4..8].copy_from_slice(&f32::NAN.to_le_bytes());
        let stop = AtomicBool::new(false);
        feed_reader(&mut writer, bytes.as_slice(), 256, &stop).unwrap();
        assert_eq!(writer.position(), 3 * 256);
        let (header, samples) = packets.next_packet().unwrap();
        assert_eq!(header.position, 0);
        assert_eq!(&samples[..4], &[0.0, 0.0, 2.0, 3.0]);
    }

    #[test]
    fn the_test_tone_arrives_at_the_pace_of_the_clock() {
        // Nothing is taken out while the tone runs, so allow a long backlog.
        let (mut writer, mut packets) = capture_ring(2, 48_000.0, 1.0, 0.9, 0.5);
        let mut tone = Tone::new(997.0, 0.25, 48_000.0);
        let stop = AtomicBool::new(false);
        let begin = Instant::now();
        feed_tone(&mut writer, &mut tone, 256, Some(0.2), &stop);
        let took = begin.elapsed().as_secs_f64();
        assert!((0.19..0.6).contains(&took), "{took}");
        // 0.2 s are 9600 frames: 38 pieces of 256.
        assert_eq!(writer.position(), 38 * 256);
        let (header, samples) = packets.next_packet().unwrap();
        assert_eq!(header.position, 0);
        assert!(samples.iter().any(|sample| sample.abs() > 0.2));
        assert!(samples.iter().all(|sample| sample.abs() <= 0.25));
    }
}
