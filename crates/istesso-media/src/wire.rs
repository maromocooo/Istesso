//! What travels between a host and a viewer.
//!
//! A connection starts with one JSON message from the viewer ([`Ask::Watch`])
//! and one from the host ([`Reply`]). After a `stream` reply, everything the
//! host sends is a binary packet: a header followed by one encoded frame.
//! The viewer may send further JSON messages ([`Ask::KeyFrame`]).
//!
//! ```text
//!   offset  bytes  content
//!   0       1      version of this layout (1)
//!   1       1      kind: 1 frame, 2 nothing to show at the moment, 3 end
//!   2       2      length of the header (40 in this version)
//!   4       4      flags: bit 0, a key frame; bit 1, does not follow on from
//!                  the frame sent before it on this connection
//!   8       8      sequence number: one more for every frame the host encodes
//!   16      8      presentation time stamp (the pipeline's clock, ns)
//!   24      8      when the encoder had finished (the host's clock, ns)
//!   32      4      length of the encoded frame
//!   36      4      reserved, 0
//!   40      n      the encoded frame
//! ```
//!
//! All numbers are little-endian. A reader goes by the length of the header
//! that the packet states and skips what lies between the fields it knows and
//! the frame: a later version can add fields without a new layout version.
//! Over a reliable connection the sequence number and the length carry no
//! news; they are there so that the same frames can later travel as
//! datagrams (ADR 0015).

use serde::{Deserialize, Serialize};

/// Port a host listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 47803;
/// Version of the packet layout, and of the first exchange.
pub const VERSION: u8 = 1;
/// Bytes in front of the encoded frame, as this version writes them.
pub const HEADER_LEN: usize = 40;
/// Largest encoded frame. Bounds what a host can make a viewer hold.
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
/// Largest message a viewer may send once it has been answered.
pub const MAX_ASK_BYTES: usize = 1024;

const FLAG_KEY_FRAME: u32 = 1;
const FLAG_DISCONTINUITY: u32 = 2;

/// What a viewer says to a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Ask {
    /// The first message of a connection: show me your picture.
    Watch { version: u32 },
    /// The picture cannot be decoded from here on: send a key frame.
    KeyFrame,
}

/// What the host answers to [`Ask::Watch`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    /// Frames follow, of this form.
    Stream {
        version: u32,
        #[serde(flatten)]
        info: StreamInfo,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
}

/// The form of a host's picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamInfo {
    pub codec: Codec,
    /// Size of the picture in its own pixels.
    pub width: u32,
    pub height: u32,
    /// Most frames in any one second.
    pub fps: u32,
    pub chroma: Chroma,
    pub colour: Colour,
}

/// How the frames are encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Codec {
    /// H.265, one access unit per frame, NAL units behind start codes
    /// (Annex B). Key frames come with their parameter sets.
    H265,
    #[serde(other)]
    Unknown,
}

/// How much colour the stream keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Chroma {
    /// One colour sample for every four pixels.
    #[serde(rename = "420")]
    Quarter,
    /// One colour sample for every pixel.
    #[serde(rename = "444")]
    Full,
    #[serde(other, rename = "unknown")]
    Unknown,
}

/// What the stream says about its colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Colour {
    /// What any window holds: to be shown like the viewer's own windows.
    Srgb,
    /// Nothing is said; a decoder then takes the picture for video.
    #[serde(other)]
    Unspecified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The first message was not understood.
    Unsupported,
    /// The device is paired but may not look at this host.
    Forbidden,
    /// The host shows its picture to as many devices as it can.
    Busy,
    #[serde(other)]
    Other,
}

/// What a packet is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Frame,
    /// The host is there but has no new picture. Sent now and then, so that
    /// a picture that stands still is not mistaken for a broken connection.
    Idle,
    /// The host stops sending on this connection.
    End,
}

/// The fixed part of every packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub kind: Kind,
    pub key_frame: bool,
    /// The frame does not follow on from the one sent before it on this
    /// connection: the host held frames back in between.
    pub discontinuity: bool,
    pub seq: u64,
    /// Presentation time stamp, on the clock of the host's pipeline.
    pub pts_ns: u64,
    /// When the encoder had finished the frame, on the host's clock.
    pub encoded_ns: u64,
}

impl Header {
    /// The header of a packet that carries no frame.
    pub fn without_frame(kind: Kind, seq: u64) -> Self {
        Header {
            kind,
            key_frame: false,
            discontinuity: false,
            seq,
            pts_ns: 0,
            encoded_ns: 0,
        }
    }
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
    #[error("the packet's lengths do not add up, or its frame is too large")]
    Length,
}

/// Writes one packet into `out`, replacing what was there.
pub fn encode(header: &Header, frame: &[u8], out: &mut Vec<u8>) {
    out.clear();
    out.reserve(HEADER_LEN + frame.len());
    out.push(VERSION);
    out.push(match header.kind {
        Kind::Frame => 1,
        Kind::Idle => 2,
        Kind::End => 3,
    });
    out.extend_from_slice(&(HEADER_LEN as u16).to_le_bytes());
    out.extend_from_slice(&flags(header).to_le_bytes());
    out.extend_from_slice(&header.seq.to_le_bytes());
    out.extend_from_slice(&header.pts_ns.to_le_bytes());
    out.extend_from_slice(&header.encoded_ns.to_le_bytes());
    out.extend_from_slice(&(frame.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(frame);
}

fn flags(header: &Header) -> u32 {
    (if header.key_frame { FLAG_KEY_FRAME } else { 0 })
        | (if header.discontinuity {
            FLAG_DISCONTINUITY
        } else {
            0
        })
}

/// Changes, in a packet made by [`encode`], whether it follows on from the
/// frame before it. The same frame goes to several viewers, and each of them
/// has its own history.
pub fn set_discontinuity(packet: &mut [u8], discontinuity: bool) {
    if packet.len() >= HEADER_LEN {
        let mut flags = u32::from_le_bytes([packet[4], packet[5], packet[6], packet[7]]);
        if discontinuity {
            flags |= FLAG_DISCONTINUITY;
        } else {
            flags &= !FLAG_DISCONTINUITY;
        }
        packet[4..8].copy_from_slice(&flags.to_le_bytes());
    }
}

/// Reads a packet: its header, and where in the packet its frame lies.
pub fn decode(packet: &[u8]) -> Result<(Header, std::ops::Range<usize>), WireError> {
    if packet.len() < HEADER_LEN {
        return Err(WireError::TooShort);
    }
    if packet[0] != VERSION {
        return Err(WireError::Version(packet[0]));
    }
    let header_len = usize::from(u16::from_le_bytes([packet[2], packet[3]]));
    let word = |at: usize| -> [u8; 8] {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&packet[at..at + 8]);
        bytes
    };
    let quad = |at: usize| -> u32 {
        u32::from_le_bytes([packet[at], packet[at + 1], packet[at + 2], packet[at + 3]])
    };
    let frame_len = quad(32) as usize;
    // Fields this version does not know lie between its own header and the
    // frame, and are passed over.
    if header_len < HEADER_LEN
        || header_len > packet.len()
        || frame_len > MAX_FRAME_BYTES
        || packet.len() - header_len != frame_len
    {
        return Err(WireError::Length);
    }
    let kind = match packet[1] {
        1 => Kind::Frame,
        2 => Kind::Idle,
        3 => Kind::End,
        other => return Err(WireError::Kind(other)),
    };
    let flags = quad(4);
    let header = Header {
        kind,
        key_frame: flags & FLAG_KEY_FRAME != 0,
        discontinuity: flags & FLAG_DISCONTINUITY != 0,
        seq: u64::from_le_bytes(word(8)),
        pts_ns: u64::from_le_bytes(word(16)),
        encoded_ns: u64::from_le_bytes(word(24)),
    };
    Ok((header, header_len..packet.len()))
}

/// How many messages of the encrypted channel a frame of `frame_len` bytes
/// travels in. One message holds 65535 bytes less its tag; the channel puts
/// four bytes of length in front of what it carries.
pub fn channel_messages(frame_len: usize) -> u32 {
    const PER_MESSAGE: usize = 65535 - 16;
    (4 + HEADER_LEN + frame_len).div_ceil(PER_MESSAGE) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_header() -> Header {
        Header {
            kind: Kind::Frame,
            key_frame: true,
            discontinuity: false,
            seq: 0x0102_0304_0506_0708,
            pts_ns: 11_764_705,
            encoded_ns: 987_654_321_012,
        }
    }

    #[test]
    fn a_packet_comes_back_as_it_was_written() {
        let frame: Vec<u8> = (0..7500u32).map(|i| (i % 251) as u8).collect();
        let mut packet = Vec::new();
        encode(&frame_header(), &frame, &mut packet);
        assert_eq!(packet.len(), HEADER_LEN + frame.len());
        let (header, at) = decode(&packet).unwrap();
        assert_eq!(header, frame_header());
        assert_eq!(&packet[at], frame.as_slice());

        // The same packet for a viewer that missed the frames before it.
        set_discontinuity(&mut packet, true);
        let (header, at) = decode(&packet).unwrap();
        assert!(header.discontinuity && header.key_frame);
        assert_eq!(&packet[at], frame.as_slice());
        set_discontinuity(&mut packet, false);
        assert_eq!(decode(&packet).unwrap().0, frame_header());
    }

    #[test]
    fn packets_without_a_frame_are_read() {
        let mut packet = Vec::new();
        for kind in [Kind::Idle, Kind::End] {
            encode(&Header::without_frame(kind, 7), &[], &mut packet);
            let (header, at) = decode(&packet).unwrap();
            assert_eq!(header.kind, kind);
            assert_eq!(header.seq, 7);
            assert!(at.is_empty());
        }
    }

    #[test]
    fn fields_a_later_version_adds_are_passed_over() {
        let mut packet = Vec::new();
        encode(&frame_header(), b"picture", &mut packet);
        // A later version writes twelve more bytes of header.
        let mut longer = packet[..HEADER_LEN].to_vec();
        longer[2..4].copy_from_slice(&((HEADER_LEN + 12) as u16).to_le_bytes());
        longer.extend_from_slice(&[0xee; 12]);
        longer.extend_from_slice(b"picture");
        let (header, at) = decode(&longer).unwrap();
        assert_eq!(header, frame_header());
        assert_eq!(&longer[at], b"picture");
    }

    #[test]
    fn packets_that_do_not_add_up_are_refused() {
        let mut packet = Vec::new();
        encode(&frame_header(), b"picture", &mut packet);
        assert_eq!(decode(&packet[..HEADER_LEN - 1]), Err(WireError::TooShort));
        // Cut short, and with bytes after its end.
        assert_eq!(decode(&packet[..packet.len() - 1]), Err(WireError::Length));
        let mut longer = packet.clone();
        longer.push(0);
        assert_eq!(decode(&longer), Err(WireError::Length));
        // A header that claims to be shorter than this version's, or longer
        // than the packet.
        for claimed in [HEADER_LEN as u16 - 1, 4000] {
            let mut odd = packet.clone();
            odd[2..4].copy_from_slice(&claimed.to_le_bytes());
            assert_eq!(decode(&odd), Err(WireError::Length));
        }
        // A frame larger than any frame may be.
        let mut huge = packet.clone();
        huge[32..36].copy_from_slice(&(MAX_FRAME_BYTES as u32 + 1).to_le_bytes());
        assert_eq!(decode(&huge), Err(WireError::Length));

        let mut other_version = packet.clone();
        other_version[0] = 2;
        assert_eq!(decode(&other_version), Err(WireError::Version(2)));
        let mut other_kind = packet.clone();
        other_kind[1] = 9;
        assert_eq!(decode(&other_kind), Err(WireError::Kind(9)));
    }

    #[test]
    fn the_first_exchange_reads_as_written_down() {
        let watch: Ask = serde_json::from_str(r#"{"type":"watch","version":1}"#).unwrap();
        assert_eq!(watch, Ask::Watch { version: 1 });
        assert_eq!(
            serde_json::to_string(&Ask::KeyFrame).unwrap(),
            r#"{"type":"key_frame"}"#
        );
        let reply = Reply::Stream {
            version: 1,
            info: StreamInfo {
                codec: Codec::H265,
                width: 3440,
                height: 1440,
                fps: 85,
                chroma: Chroma::Full,
                colour: Colour::Srgb,
            },
        };
        assert_eq!(
            serde_json::to_string(&reply).unwrap(),
            r#"{"type":"stream","version":1,"codec":"h265","width":3440,"height":1440,"fps":85,"chroma":"444","colour":"srgb"}"#
        );
        assert_eq!(
            serde_json::from_str::<Reply>(&serde_json::to_string(&reply).unwrap()).unwrap(),
            reply
        );
    }

    #[test]
    fn what_a_later_version_says_is_read_as_far_as_it_is_known() {
        // Fields that are new are ignored; values that are new are read as
        // "not known", and it is the reader who decides what to do then.
        let reply: Reply = serde_json::from_str(
            r#"{"type":"stream","version":2,"codec":"av1","width":1,"height":2,"fps":3,
                "chroma":"422","colour":"display_p3","bit_depth":10}"#,
        )
        .unwrap();
        let Reply::Stream { version, info } = reply else {
            panic!("not a stream");
        };
        assert_eq!(version, 2);
        assert_eq!(info.codec, Codec::Unknown);
        assert_eq!(info.chroma, Chroma::Unknown);
        assert_eq!(info.colour, Colour::Unspecified);

        let error: Reply =
            serde_json::from_str(r#"{"type":"error","code":"on_fire","message":"m"}"#).unwrap();
        assert!(matches!(
            error,
            Reply::Error {
                code: ErrorCode::Other,
                ..
            }
        ));
        // A request of a kind this version does not know is not one of ours.
        assert!(serde_json::from_str::<Ask>(r#"{"type":"teleport"}"#).is_err());
        assert!(serde_json::from_str::<Ask>(r#"{"type":"key_frame","why":"x"}"#).is_ok());
    }

    #[test]
    fn frames_are_counted_in_channel_messages() {
        assert_eq!(channel_messages(0), 1);
        assert_eq!(channel_messages(7_500), 1);
        assert_eq!(channel_messages(65_519 - 44), 1);
        assert_eq!(channel_messages(65_519 - 43), 2);
        assert_eq!(channel_messages(140_000), 3);
    }
}
