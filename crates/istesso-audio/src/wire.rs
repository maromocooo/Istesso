//! What travels between a source and an exit.
//!
//! A connection starts with one JSON message from the exit ([`Hello`]) and
//! one from the source ([`Reply`]). After a `subscribe`, everything the
//! source sends is a binary packet: a fixed header ([`Header`]) followed by
//! the samples.
//!
//! ```text
//!   offset  bytes  content
//!   0       1      version of this layout (1)
//!   1       1      kind: 1 audio, 2 the source has nothing to play, 3 end
//!   2       1      flags: bit 0, this packet does not follow on from the last
//!   3       1      reserved, 0
//!   4       4      sequence number of the packet
//!   8       8      position of the first frame, counted from the stream's start
//!   16      8      when the first frame was captured (the source's clock, ns)
//!   24      n      samples: 32-bit floats, channels interleaved
//! ```
//!
//! All numbers are little-endian. The number of frames is what the length of
//! the packet says it is. Over a reliable connection the sequence number
//! carries no news; it is there so that the same packets can later travel as
//! datagrams.

use serde::{Deserialize, Serialize};

/// Port a source listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 47802;
/// Version of the packet layout, and of the first exchange.
pub const VERSION: u8 = 1;
/// Bytes in front of the samples of every packet.
pub const HEADER_LEN: usize = 24;
/// Most frames one packet may carry. Bounds what a source can make an exit
/// copy in one go.
pub const MAX_PACKET_FRAMES: usize = 8192;

const FLAG_DISCONTINUITY: u8 = 1;

/// First message of a connection, from the exit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Hello {
    /// Send me your sound.
    Subscribe { version: u32 },
    /// What time is it on your clock? `t1` is the exit's clock when it asked.
    Time { t1: u64 },
}

/// What the source answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    /// Packets follow, in this form.
    Stream {
        version: u32,
        /// Changes whenever the source starts counting positions afresh.
        stream_id: String,
        sample_rate: u32,
        channels: u16,
        format: Format,
        /// Frames in a typical packet.
        packet_frames: u32,
    },
    /// `t2`: the source's clock when the question arrived; `t3`: when this
    /// answer left.
    Time {
        t1: u64,
        t2: u64,
        t3: u64,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
}

/// How the samples are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// 32-bit floats, little-endian.
    F32le,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The first message was not understood.
    Unsupported,
    /// The device is paired but may not have this host's sound.
    Forbidden,
    #[serde(other)]
    Other,
}

/// What a packet is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Audio,
    /// The source is there but has nothing to play. Sent now and then, so
    /// that silence is not mistaken for a broken connection.
    Idle,
    /// The source stops sending on this connection.
    End,
}

/// The fixed part of every packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub kind: Kind,
    /// The audio does not follow on from the previous packet: the source
    /// let some go, or started again.
    pub discontinuity: bool,
    pub seq: u32,
    /// Index of the first frame, counted from the start of the stream.
    pub position: u64,
    /// When the first frame was captured, on the source's monotonic clock.
    pub capture_ns: u64,
}

/// Why a packet could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WireError {
    #[error("the packet is shorter than its header")]
    TooShort,
    #[error("the packet uses layout version {0}, which this version of istesso does not know")]
    Version(u8),
    #[error("the packet is of a kind this version does not know ({0})")]
    Kind(u8),
    #[error("the samples of the packet are not a whole number of frames, or too many")]
    Payload,
}

/// Writes one packet into `out`, replacing what was there.
pub fn encode(header: &Header, samples: &[f32], out: &mut Vec<u8>) {
    out.clear();
    out.reserve(HEADER_LEN + samples.len() * 4);
    out.push(VERSION);
    out.push(match header.kind {
        Kind::Audio => 1,
        Kind::Idle => 2,
        Kind::End => 3,
    });
    out.push(if header.discontinuity {
        FLAG_DISCONTINUITY
    } else {
        0
    });
    out.push(0);
    out.extend_from_slice(&header.seq.to_le_bytes());
    out.extend_from_slice(&header.position.to_le_bytes());
    out.extend_from_slice(&header.capture_ns.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
}

/// How many received samples were not numbers (NaN, infinities) and were
/// taken as silence, in this process: a source that sends such values
/// shows here. They are replaced as they are read, before they reach the
/// queue, the resampler or any fade.
static NOT_NUMBERS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// See [`NOT_NUMBERS`].
pub fn not_numbers_received() -> u64 {
    NOT_NUMBERS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Reads a packet: its header, and its samples into `samples` (replacing
/// what was there). Returns the header and the number of frames.
///
/// Values that are not numbers are replaced by silence: they must not reach
/// a resampler or a loudspeaker.
pub fn decode(
    packet: &[u8],
    channels: usize,
    samples: &mut Vec<f32>,
) -> Result<(Header, usize), WireError> {
    if packet.len() < HEADER_LEN {
        return Err(WireError::TooShort);
    }
    if packet[0] != VERSION {
        return Err(WireError::Version(packet[0]));
    }
    let kind = match packet[1] {
        1 => Kind::Audio,
        2 => Kind::Idle,
        3 => Kind::End,
        other => return Err(WireError::Kind(other)),
    };
    let word = |at: usize| -> [u8; 8] {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&packet[at..at + 8]);
        bytes
    };
    let header = Header {
        kind,
        discontinuity: packet[2] & FLAG_DISCONTINUITY != 0,
        seq: u32::from_le_bytes([packet[4], packet[5], packet[6], packet[7]]),
        position: u64::from_le_bytes(word(8)),
        capture_ns: u64::from_le_bytes(word(16)),
    };

    let payload = &packet[HEADER_LEN..];
    let frame_bytes = channels * 4;
    samples.clear();
    if kind != Kind::Audio {
        // Nothing to play in these; whatever follows the header is ignored.
        return Ok((header, 0));
    }
    if frame_bytes == 0 || payload.len() % frame_bytes != 0 {
        return Err(WireError::Payload);
    }
    let frames = payload.len() / frame_bytes;
    if frames == 0 || frames > MAX_PACKET_FRAMES {
        return Err(WireError::Payload);
    }
    samples.extend(payload.chunks_exact(4).map(|bytes| {
        let value = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        if value.is_finite() {
            value
        } else {
            NOT_NUMBERS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            0.0
        }
    }));
    Ok((header, frames))
}

/// A fingerprint of the samples of one packet, for checking that what was
/// received is what was sent (FNV-1a over the bytes as they travel).
pub fn digest(samples: &[f32]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for sample in samples {
        for byte in sample.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> Header {
        Header {
            kind: Kind::Audio,
            discontinuity: false,
            seq: 0x0102_0304,
            position: 0x1122_3344_5566_7788,
            capture_ns: 987_654_321_012,
        }
    }

    #[test]
    fn packets_round_trip() {
        let samples = [0.0f32, 1.0, -0.5, 0.25, f32::MIN_POSITIVE, -1.0];
        let mut packet = Vec::new();
        encode(&header(), &samples, &mut packet);
        assert_eq!(packet.len(), HEADER_LEN + samples.len() * 4);

        let mut back = vec![9.0f32; 3];
        let (decoded, frames) = decode(&packet, 2, &mut back).unwrap();
        assert_eq!(decoded, header());
        assert_eq!(frames, 3);
        assert_eq!(back, samples);

        // The same bytes read as three channels are two frames.
        let (_, frames) = decode(&packet, 3, &mut back).unwrap();
        assert_eq!(frames, 2);
    }

    #[test]
    fn the_layout_is_the_documented_one() {
        let mut packet = Vec::new();
        let mut header = header();
        header.discontinuity = true;
        encode(&header, &[1.0], &mut packet);
        assert_eq!(packet[0], 1, "version");
        assert_eq!(packet[1], 1, "kind");
        assert_eq!(packet[2], 1, "flags");
        assert_eq!(packet[3], 0, "reserved");
        assert_eq!(&packet[4..8], &[0x04, 0x03, 0x02, 0x01]);
        assert_eq!(packet[8], 0x88);
        assert_eq!(packet[15], 0x11);
        assert_eq!(&packet[16..24], &987_654_321_012u64.to_le_bytes());
        assert_eq!(&packet[24..], &1.0f32.to_le_bytes());
    }

    #[test]
    fn idle_and_end_carry_no_samples() {
        let mut packet = Vec::new();
        let mut samples = vec![1.0f32];
        for kind in [Kind::Idle, Kind::End] {
            let header = Header { kind, ..header() };
            encode(&header, &[], &mut packet);
            assert_eq!(packet.len(), HEADER_LEN);
            let (decoded, frames) = decode(&packet, 2, &mut samples).unwrap();
            assert_eq!(decoded.kind, kind);
            assert_eq!(frames, 0);
            assert!(samples.is_empty());
        }
    }

    #[test]
    fn damaged_packets_are_refused() {
        let mut packet = Vec::new();
        let mut samples = Vec::new();
        encode(&header(), &[0.5, 0.5, 0.5, 0.5], &mut packet);

        assert_eq!(
            decode(&packet[..HEADER_LEN - 1], 2, &mut samples),
            Err(WireError::TooShort)
        );
        // Half a frame.
        assert_eq!(
            decode(&packet[..packet.len() - 4], 2, &mut samples),
            Err(WireError::Payload)
        );
        // Audio without any audio.
        assert_eq!(
            decode(&packet[..HEADER_LEN], 2, &mut samples),
            Err(WireError::Payload)
        );
        // More than a packet may carry.
        let mut large = Vec::new();
        encode(&header(), &vec![0.0; MAX_PACKET_FRAMES + 1], &mut large);
        assert_eq!(decode(&large, 1, &mut samples), Err(WireError::Payload));

        let mut newer = packet.clone();
        newer[0] = 2;
        assert_eq!(decode(&newer, 2, &mut samples), Err(WireError::Version(2)));
        let mut unknown = packet.clone();
        unknown[1] = 9;
        assert_eq!(decode(&unknown, 2, &mut samples), Err(WireError::Kind(9)));
        // Flags this version does not know are ignored.
        let mut flagged = packet.clone();
        flagged[2] = 0b1111_0000;
        let (decoded, _) = decode(&flagged, 2, &mut samples).unwrap();
        assert!(!decoded.discontinuity);
    }

    #[test]
    fn values_that_are_not_numbers_become_silence() {
        let mut packet = Vec::new();
        encode(
            &header(),
            &[f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.75],
            &mut packet,
        );
        let mut samples = Vec::new();
        decode(&packet, 1, &mut samples).unwrap();
        assert_eq!(samples, [0.0, 0.0, 0.0, 0.75]);
    }

    #[test]
    fn the_first_exchange_is_small_tagged_json() {
        assert_eq!(
            serde_json::to_string(&Hello::Subscribe { version: 1 }).unwrap(),
            r#"{"type":"subscribe","version":1}"#
        );
        let reply = Reply::Stream {
            version: 1,
            stream_id: "abc".into(),
            sample_rate: 48_000,
            channels: 2,
            format: Format::F32le,
            packet_frames: 256,
        };
        let json = serde_json::to_string(&reply).unwrap();
        assert!(json.contains(r#""format":"f32le""#), "{json}");
        assert_eq!(serde_json::from_str::<Reply>(&json).unwrap(), reply);

        // Readers skip what they do not know and name what they cannot use.
        let newer = r#"{"type":"stream","version":2,"stream_id":"x","sample_rate":96000,
            "channels":2,"format":"s24le","packet_frames":480,"dither":"none"}"#;
        match serde_json::from_str::<Reply>(newer).unwrap() {
            Reply::Stream { format, .. } => assert_eq!(format, Format::Unknown),
            other => panic!("unexpected: {other:?}"),
        }
        let error = r#"{"type":"error","code":"too_many_listeners","message":"later"}"#;
        assert_eq!(
            serde_json::from_str::<Reply>(error).unwrap(),
            Reply::Error {
                code: ErrorCode::Other,
                message: "later".into()
            }
        );
        assert!(serde_json::from_str::<Hello>(r#"{"type":"teleport"}"#).is_err());
    }

    #[test]
    fn the_digest_tells_packets_apart() {
        assert_eq!(digest(&[]), 0xcbf2_9ce4_8422_2325);
        assert_ne!(digest(&[0.5, 0.25]), digest(&[0.25, 0.5]));
        assert_eq!(digest(&[0.5, 0.25]), digest(&[0.5, 0.25]));
    }
}
