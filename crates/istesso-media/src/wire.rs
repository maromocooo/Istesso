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
//!   1       1      kind: 1 frame, 2 nothing to show at the moment, 3 end, 4 the cursor,
//!                  5 the text input field (trial only), 6 the windows (trial only)
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
//! A packet of kind 4 says what the pointer's cursor looks like, so that the viewer can draw
//! it itself where its own pointer is (ADR 0015, "カーソル"). In place of the encoded frame it
//! carries:
//!
//! ```text
//!   offset  bytes  content
//!   0       1      shape: 0 hidden, 1 a cursor there is no image of, 2 an image
//!   1       1      reserved, 0
//!   2       2      length of this fixed part (16 in this version)
//!   4       2      width of the image in pixels (0 unless an image)
//!   6       2      height
//!   8       2      hot spot, x: the pixel of the image that lies on the pointer's position
//!   10      2      hot spot, y
//!   12      2      scale: pixels of the image to one pixel of the picture
//!   14      2      length of the name in bytes (shape 1 only)
//!   16      n      the name (UTF-8), or the pixels: four bytes each, red, green, blue,
//!                  alpha, the colours multiplied by alpha, rows from the top
//! ```
//!
//! The image comes from whatever application runs on the host. Neither end takes its numbers
//! on trust: a cursor is only written, and only read, when its sides are between 1 and 256
//! pixels, its hot spot lies inside it, and what follows the fixed part is exactly as long as
//! the numbers say. A viewer that does not know kind 4 passes the packet over, like any kind
//! it does not know.
//!
//! A packet of kind 5 (trial only, ADR 0015, "文字の入力の欄") says whether the application
//! with the keyboard has a text input field, whether it is a password field, and where its
//! text cursor is, so that the viewer's input method can put its candidates there:
//!
//! ```text
//!   offset  bytes  content
//!   0       1      state: 0 nothing known, 1 a field, 2 a password field, 3 not in a field
//!                  (the application has a text input there and none is enabled)
//!   1       1      the field's serial: one more (mod 256) whenever another field, or the
//!                  same one enabled again, has the keyboard (an earlier version wrote 0)
//!   2       2      length of this fixed part (16 in this version)
//!   4       4      text cursor, x, in the picture's pixels (signed)
//!   8       4      y
//!   12      2      width
//!   14      2      height
//!   16      2      pieces of committed text the host had nowhere to put so far (mod 65536;
//!                  added 2026-11-19, fixed part 18; an earlier writer's 16 reads as 0)
//! ```
//!
//! No typed text is in it. A viewer that does not know kind 5 passes the packet over.
//!
//! A packet of kind 6 (trial only, ADR 0027 proposal) says which other windows the
//! application has besides the first, so that the viewer can show each of them in a window of
//! its own. It carries JSON (UTF-8), at most 8192 bytes, in place of the encoded frame:
//!
//! ```text
//!   {"serial":N,"windows":[{"id":2,"port":P,"width":W,"height":H,"title":"...",
//!                           "modal":false,"key":false,"attention":false}]}
//! ```
//!
//! `serial` is one more whenever the program on the host says the list anew. `id` names the
//! window (2 or more; the first window is 1, and is not in the list), as window numbers in
//! input do. `port` is where the host shows that window's picture (0: not yet), `width` and
//! `height` its size in pixels (at most 8192), `title` its title (at most 256 bytes; never
//! logged), `modal` whether it keeps the rest of the application from being used, `key`
//! whether it is the one typed into, `attention` whether it asks for the person's attention,
//! and, when the host says it, `points` its size on the host in points (`[W, H]`, each 1 to
//! 8192), so that the viewer's window has that size whatever density its picture has.
//! At most 16 windows. Neither end takes a list that is not within these limits, and fields a
//! later version adds are passed over. The latest list is the whole of it: a window that is
//! not in it is gone. A viewer that does not know kind 6 passes the packet over.
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
/// Most pixels a cursor's image may have along one side.
pub const MAX_CURSOR_SIDE: u16 = 256;
/// Most pixels of a cursor's image to one pixel of the picture.
pub const MAX_CURSOR_SCALE: u16 = 8;
/// Longest name of a cursor there is no image of, in bytes.
pub const MAX_CURSOR_NAME: usize = 64;
/// Bytes in front of a cursor's name or pixels, as this version writes them.
pub const CURSOR_FIXED_LEN: usize = 16;
/// The fixed part of what a packet of kind 5 carries (16 in its first version).
pub const TEXT_FIELD_FIXED_LEN: usize = 18;
/// The fixed part of kind 5 as its first version wrote it: still read.
const TEXT_FIELD_FIRST_LEN: usize = 16;
/// Most bytes of what a packet of kind 6 carries (trial only).
pub const MAX_WINDOWS_BYTES: usize = 8192;
/// Most windows in a packet of kind 6, besides the first.
pub const MAX_WINDOWS: usize = 16;
/// Most pixels of a window's side in a packet of kind 6.
pub const MAX_WINDOW_SIDE: u32 = 8192;
/// Longest title of a window in a packet of kind 6, in bytes.
pub const MAX_WINDOW_TITLE: usize = 256;

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
    /// The device is paired and may look, but it is not the one that is
    /// shown this picture.
    NotShown,
    #[serde(other)]
    Other,
}

/// Why a host stops sending on a connection of its own accord: the first
/// byte of what a packet of the kind [`Kind::End`] carries.
///
/// Whatever it says, the viewer does not connect again by itself. The
/// difference is in what a person is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// The stream is over: the application has ended, or the host has
    /// stopped. Also what a packet without that byte means, and a byte
    /// this version does not know.
    Over,
    /// This device is not shown the picture any more: another one is, or
    /// nobody.
    NotShown,
    /// The picture is put aside for the time being and shown to nobody.
    SetAside,
}

impl Ending {
    fn byte(self) -> u8 {
        match self {
            Ending::Over => 0,
            Ending::NotShown => 1,
            Ending::SetAside => 2,
        }
    }
}

/// The packet that says a stream is over, and why.
pub fn encode_end(why: Ending, seq: u64, out: &mut Vec<u8>) {
    encode(&Header::without_frame(Kind::End, seq), &[why.byte()], out);
}

/// Why the stream is over, from what a packet of the kind [`Kind::End`]
/// carries.
pub fn decode_end(carried: &[u8]) -> Ending {
    match carried.first() {
        Some(1) => Ending::NotShown,
        Some(2) => Ending::SetAside,
        _ => Ending::Over,
    }
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
    /// What the pointer's cursor looks like from now on ([`Cursor`]).
    Cursor,
    /// The text input field of the application with the keyboard ([`TextField`]). Trial only.
    TextField,
    /// The application's other windows ([`Windows`]). Trial only.
    Windows,
}

/// The text input field of the application with the keyboard, and where its text cursor
/// is, in the picture's pixels. Trial only (ADR 0015, "文字の入力の欄").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextField {
    /// 0 nothing known, 1 a field, 2 a password field, 3 not in a field.
    pub state: u8,
    /// One more (mod 256) whenever another field has the keyboard: the viewer ends its
    /// composition when it changes.
    pub serial: u8,
    pub x: i32,
    pub y: i32,
    pub width: u16,
    pub height: u16,
    /// Pieces of committed text the host had nowhere to put so far (mod 65536): when it grows,
    /// the viewer keeps what it sent last for the person.
    pub dropped: u16,
}

/// The application's windows besides the first, as the host says them (trial only, ADR 0027
/// proposal): what a packet of kind 6 carries. See the module's description for the limits,
/// which [`Windows::check`] holds it to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Windows {
    /// One more whenever the program on the host says the list anew (it counts; this crate
    /// passes the number on).
    pub serial: u64,
    pub windows: Vec<Window>,
}

/// One of the application's other windows (trial only).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    /// 2 or more: the window's number in input events.
    pub id: u32,
    /// Where the host shows this window's picture; 0: not yet.
    pub port: u16,
    pub width: u32,
    pub height: u32,
    /// Never logged: `{:?}` does not show it.
    #[serde(default)]
    pub title: String,
    /// It keeps the rest of the application from being used until it is closed.
    #[serde(default)]
    pub modal: bool,
    /// It is the window typed into.
    #[serde(default)]
    pub key: bool,
    /// It asks for the person's attention.
    #[serde(default)]
    pub attention: bool,
    /// Its size on the host, in the host's points (each 1 to [`MAX_WINDOW_SIDE`]): the viewer's
    /// window takes this size, whatever the density of the screen it was captured from (added
    /// 2026-11-20; left out by an earlier writer, and then not written).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub points: Option<[u32; 2]>,
}

/// Written without the title: a window's title may say what a person is working on.
impl std::fmt::Debug for Window {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Window")
            .field("id", &self.id)
            .field("port", &self.port)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("modal", &self.modal)
            .field("key", &self.key)
            .field("attention", &self.attention)
            .field("points", &self.points)
            .finish_non_exhaustive()
    }
}

impl Windows {
    /// Whether the list is within the limits: at most [`MAX_WINDOWS`] windows, each of number
    /// 2 or more, of sides of at most [`MAX_WINDOW_SIDE`], with a title of at most
    /// [`MAX_WINDOW_TITLE`] bytes. Any port is taken (0: no picture yet).
    pub fn check(&self) -> Result<(), WireError> {
        let fits = self.windows.len() <= MAX_WINDOWS
            && self.windows.iter().all(|window| {
                window.id >= 2
                    && window.width <= MAX_WINDOW_SIDE
                    && window.height <= MAX_WINDOW_SIDE
                    && window.title.len() <= MAX_WINDOW_TITLE
                    && window
                        .points
                        .is_none_or(|p| p.iter().all(|&side| (1..=MAX_WINDOW_SIDE).contains(&side)))
            });
        if fits {
            Ok(())
        } else {
            Err(WireError::Windows)
        }
    }

    /// Reads a list from JSON of the form a packet of kind 6 carries, and checks it: at most
    /// [`MAX_WINDOWS_BYTES`] bytes, of the form, within the limits. Fields this version does
    /// not know are passed over.
    pub fn from_json(json: &[u8]) -> Result<Windows, WireError> {
        if json.len() > MAX_WINDOWS_BYTES {
            return Err(WireError::Windows);
        }
        let windows: Windows = serde_json::from_slice(json).map_err(|_| WireError::Windows)?;
        windows.check()?;
        Ok(windows)
    }

    /// The list as JSON, with only what this version knows in it. A list that is not within
    /// the limits, or that comes to more than [`MAX_WINDOWS_BYTES`] bytes written out, is not
    /// written.
    pub fn to_json(&self) -> Result<Vec<u8>, WireError> {
        self.check()?;
        let json = serde_json::to_vec(self).map_err(|_| WireError::Windows)?;
        if json.len() > MAX_WINDOWS_BYTES {
            return Err(WireError::Windows);
        }
        Ok(json)
    }
}

/// What the pointer's cursor looks like on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor<'a> {
    /// The application under the pointer has hidden it: nothing is drawn.
    Hidden,
    /// A cursor the host has no image of. The viewer shows a cursor of its own.
    Named(&'a str),
    Image(CursorImage<'a>),
}

/// A cursor's image, as an application on the host drew it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorImage<'a> {
    pub width: u16,
    pub height: u16,
    /// The pixel of the image that lies on the pointer's position.
    pub hotspot_x: u16,
    pub hotspot_y: u16,
    /// Pixels of the image to one pixel of the picture.
    pub scale: u16,
    /// Four bytes a pixel: red, green, blue, alpha, the colours multiplied by alpha.
    pub rgba: &'a [u8],
}

impl Cursor<'_> {
    /// Whether the cursor is one that may travel: an image no larger than the limit, with
    /// its hot spot inside it and exactly as many bytes as its size says; a name of a
    /// reasonable length.
    pub fn check(&self) -> Result<(), WireError> {
        match self {
            Cursor::Hidden => Ok(()),
            Cursor::Named(name) if name.len() <= MAX_CURSOR_NAME => Ok(()),
            Cursor::Named(_) => Err(WireError::Cursor),
            Cursor::Image(image) => {
                let sides = 1..=MAX_CURSOR_SIDE;
                let fits = sides.contains(&image.width)
                    && sides.contains(&image.height)
                    && image.hotspot_x < image.width
                    && image.hotspot_y < image.height
                    && (1..=MAX_CURSOR_SCALE).contains(&image.scale)
                    && image.rgba.len() == usize::from(image.width) * usize::from(image.height) * 4;
                if fits {
                    Ok(())
                } else {
                    Err(WireError::Cursor)
                }
            }
        }
    }
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
    #[error(
        "the cursor is not one that may travel: too large, of no size, its hot spot outside \
         it, or not as long as its size says"
    )]
    Cursor,
    #[error("the text field is not one this version knows")]
    TextField,
    #[error(
        "the list of windows is not one that may travel: not of the form, too long, or with \
         numbers out of range"
    )]
    Windows,
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
        Kind::Cursor => 4,
        Kind::TextField => 5,
        Kind::Windows => 6,
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
        4 => Kind::Cursor,
        5 => Kind::TextField,
        6 => Kind::Windows,
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

/// Writes one packet that says what the cursor is into `out`, replacing what was there. A
/// cursor that may not travel ([`Cursor::check`]) is not written.
pub fn encode_cursor(cursor: &Cursor, seq: u64, out: &mut Vec<u8>) -> Result<(), WireError> {
    cursor.check()?;
    let mut body = Vec::with_capacity(CURSOR_FIXED_LEN);
    let (shape, image, name): (u8, Option<&CursorImage>, &str) = match cursor {
        Cursor::Hidden => (0, None, ""),
        Cursor::Named(name) => (1, None, name),
        Cursor::Image(image) => (2, Some(image), ""),
    };
    body.push(shape);
    body.push(0);
    body.extend_from_slice(&(CURSOR_FIXED_LEN as u16).to_le_bytes());
    for number in [
        image.map_or(0, |image| image.width),
        image.map_or(0, |image| image.height),
        image.map_or(0, |image| image.hotspot_x),
        image.map_or(0, |image| image.hotspot_y),
        image.map_or(0, |image| image.scale),
        name.len() as u16,
    ] {
        body.extend_from_slice(&number.to_le_bytes());
    }
    body.extend_from_slice(name.as_bytes());
    body.extend_from_slice(image.map_or(&[][..], |image| image.rgba));
    encode(&Header::without_frame(Kind::Cursor, seq), &body, out);
    Ok(())
}

/// Reads what a packet of kind [`Kind::Cursor`] carries in place of a frame. Nothing is
/// taken from the numbers in it before they have been checked against its real length and
/// against the limits, and nothing is copied: the result points into `body`.
pub fn decode_cursor(body: &[u8]) -> Result<Cursor<'_>, WireError> {
    if body.len() < CURSOR_FIXED_LEN {
        return Err(WireError::Cursor);
    }
    let number = |at: usize| u16::from_le_bytes([body[at], body[at + 1]]);
    let fixed = usize::from(number(2));
    if fixed < CURSOR_FIXED_LEN || fixed > body.len() {
        return Err(WireError::Cursor);
    }
    // Fields a later version adds lie between the fixed part this version knows and the
    // name or the pixels, and are passed over.
    let rest = &body[fixed..];
    let name_len = usize::from(number(14));
    let cursor = match body[0] {
        0 if rest.is_empty() => Cursor::Hidden,
        1 if rest.len() == name_len => {
            Cursor::Named(std::str::from_utf8(rest).map_err(|_| WireError::Cursor)?)
        }
        2 => Cursor::Image(CursorImage {
            width: number(4),
            height: number(6),
            hotspot_x: number(8),
            hotspot_y: number(10),
            scale: number(12),
            rgba: rest,
        }),
        _ => return Err(WireError::Cursor),
    };
    cursor.check()?;
    Ok(cursor)
}

/// Writes one packet that says what the text field is into `out`, replacing what was there.
/// A state above 3 is not written.
pub fn encode_text_field(field: &TextField, seq: u64, out: &mut Vec<u8>) -> Result<(), WireError> {
    if field.state > 3 {
        return Err(WireError::TextField);
    }
    let mut body = Vec::with_capacity(TEXT_FIELD_FIXED_LEN);
    body.push(field.state);
    body.push(field.serial);
    body.extend_from_slice(&(TEXT_FIELD_FIXED_LEN as u16).to_le_bytes());
    body.extend_from_slice(&field.x.to_le_bytes());
    body.extend_from_slice(&field.y.to_le_bytes());
    body.extend_from_slice(&field.width.to_le_bytes());
    body.extend_from_slice(&field.height.to_le_bytes());
    body.extend_from_slice(&field.dropped.to_le_bytes());
    encode(&Header::without_frame(Kind::TextField, seq), &body, out);
    Ok(())
}

/// Reads what a packet of kind [`Kind::TextField`] carries. Fields a later version adds after
/// the fixed part are passed over; a state this version does not know is refused.
pub fn decode_text_field(body: &[u8]) -> Result<TextField, WireError> {
    if body.len() < TEXT_FIELD_FIRST_LEN {
        return Err(WireError::TextField);
    }
    let fixed = usize::from(u16::from_le_bytes([body[2], body[3]]));
    if fixed < TEXT_FIELD_FIRST_LEN || fixed > body.len() || body[0] > 3 {
        return Err(WireError::TextField);
    }
    let quad = |at: usize| i32::from_le_bytes([body[at], body[at + 1], body[at + 2], body[at + 3]]);
    let pair = |at: usize| u16::from_le_bytes([body[at], body[at + 1]]);
    Ok(TextField {
        state: body[0],
        serial: body[1],
        x: quad(4),
        y: quad(8),
        width: pair(12),
        height: pair(14),
        dropped: if fixed >= TEXT_FIELD_FIXED_LEN {
            pair(16)
        } else {
            0
        },
    })
}

/// Writes one packet that says what the application's other windows are into `out`, replacing
/// what was there (trial only). A list that may not travel ([`Windows::to_json`]) is not
/// written; what is written is the list as this version knows it, not what it was read from.
pub fn encode_windows(windows: &Windows, seq: u64, out: &mut Vec<u8>) -> Result<(), WireError> {
    let body = windows.to_json()?;
    encode(&Header::without_frame(Kind::Windows, seq), &body, out);
    Ok(())
}

/// Reads what a packet of kind [`Kind::Windows`] carries: a list that is not of the form or
/// not within the limits is refused ([`Windows::from_json`]).
pub fn decode_windows(body: &[u8]) -> Result<Windows, WireError> {
    Windows::from_json(body)
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
    fn why_a_stream_is_over_travels_in_one_byte_and_an_unknown_reason_is_just_over() {
        let mut packet = Vec::new();
        for why in [Ending::Over, Ending::NotShown, Ending::SetAside] {
            encode_end(why, 7, &mut packet);
            let (header, at) = decode(&packet).unwrap();
            assert_eq!(header.kind, Kind::End);
            assert_eq!(decode_end(&packet[at]), why);
        }
        // A host of an earlier version says it without a reason, a later one may
        // give a reason this version does not know.
        encode(&Header::without_frame(Kind::End, 7), &[], &mut packet);
        let (_, at) = decode(&packet).unwrap();
        assert_eq!(decode_end(&packet[at]), Ending::Over);
        assert_eq!(decode_end(&[200]), Ending::Over);
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

    fn arrow() -> Vec<u8> {
        (0..24 * 16 * 4).map(|i| (i % 249) as u8).collect()
    }

    fn image(rgba: &[u8]) -> CursorImage<'_> {
        CursorImage {
            width: 24,
            height: 16,
            hotspot_x: 3,
            hotspot_y: 5,
            scale: 1,
            rgba,
        }
    }

    /// What a cursor's packet carries in place of a frame.
    fn cursor_body(cursor: &Cursor) -> Vec<u8> {
        let mut packet = Vec::new();
        encode_cursor(cursor, 9, &mut packet).unwrap();
        let (header, at) = decode(&packet).unwrap();
        assert_eq!((header.kind, header.seq), (Kind::Cursor, 9));
        packet[at].to_vec()
    }

    #[test]
    fn a_text_field_comes_back_as_it_was_written_and_an_unknown_state_is_refused() {
        let field = TextField {
            state: 2,
            serial: 201,
            x: -3,
            y: 191,
            width: 0,
            height: 28,
            dropped: 513,
        };
        let mut packet = Vec::new();
        encode_text_field(&field, 7, &mut packet).unwrap();
        let (header, at) = decode(&packet).unwrap();
        assert_eq!((header.kind, header.seq), (Kind::TextField, 7));
        assert_eq!(packet.len() - at.start, TEXT_FIELD_FIXED_LEN);
        assert_eq!(decode_text_field(&packet[at.clone()]), Ok(field));
        // A later version's longer fixed part is passed over.
        let mut longer = packet[at.clone()].to_vec();
        longer[2] = 22;
        longer.extend_from_slice(&[9, 9, 9, 9]);
        assert_eq!(decode_text_field(&longer), Ok(field));
        // The first version's 16 bytes still read, with nothing dropped.
        let mut first = packet[at.clone()].to_vec();
        first.truncate(16);
        first[2] = 16;
        assert_eq!(
            decode_text_field(&first),
            Ok(TextField {
                dropped: 0,
                ..field
            })
        );
        let mut unknown = packet[at.clone()].to_vec();
        unknown[0] = 4;
        assert_eq!(decode_text_field(&unknown), Err(WireError::TextField));
        assert_eq!(
            decode_text_field(&packet[at][..15]),
            Err(WireError::TextField)
        );
        let odd = TextField { state: 4, ..field };
        assert_eq!(
            encode_text_field(&odd, 0, &mut Vec::new()),
            Err(WireError::TextField)
        );
    }

    fn a_dialog() -> Window {
        Window {
            id: 2,
            port: 47811,
            width: 480,
            height: 320,
            title: "Save changes?".into(),
            modal: true,
            key: true,
            attention: false,
            points: None,
        }
    }

    #[test]
    fn a_windows_size_in_points_travels_when_said_and_is_checked() {
        let sized = Windows {
            serial: 1,
            windows: vec![Window {
                points: Some([640, 400]),
                ..a_dialog()
            }],
        };
        let json = String::from_utf8(sized.to_json().unwrap()).unwrap();
        assert!(json.contains(r#""points":[640,400]"#), "{json}");
        assert_eq!(Windows::from_json(json.as_bytes()), Ok(sized));
        for odd in [[0, 400], [640, 9000]] {
            let wrong = Windows {
                serial: 1,
                windows: vec![Window {
                    points: Some(odd),
                    ..a_dialog()
                }],
            };
            assert_eq!(wrong.check(), Err(WireError::Windows));
        }
    }

    #[test]
    fn a_list_of_windows_comes_back_as_it_was_written_and_later_fields_are_passed_over() {
        let windows = Windows {
            serial: 3,
            windows: vec![
                a_dialog(),
                Window {
                    id: 3,
                    port: 0,
                    title: String::new(),
                    ..a_dialog()
                },
            ],
        };
        let mut packet = Vec::new();
        encode_windows(&windows, 7, &mut packet).unwrap();
        let (header, at) = decode(&packet).unwrap();
        assert_eq!((header.kind, header.seq), (Kind::Windows, 7));
        assert_eq!(decode_windows(&packet[at.clone()]), Ok(windows.clone()));
        assert!(packet[at].starts_with(
            br#"{"serial":3,"windows":[{"id":2,"port":47811,"width":480,"height":320,"title":"Save changes?","modal":true,"key":true,"attention":false}"#
        ));
        // What a later version adds is passed over, and not written on.
        let later = br#"{"serial":4,"later":[1,2],"windows":[{"id":2,"port":1,"width":1,"height":1,"title":"a","depth":3}]}"#;
        let read = Windows::from_json(later).unwrap();
        assert_eq!(read.windows[0].id, 2);
        assert!(!read.windows[0].modal);
        let written = String::from_utf8(read.to_json().unwrap()).unwrap();
        assert!(
            !written.contains("later") && !written.contains("depth"),
            "{written}"
        );
        // No window besides the first is a list too.
        assert_eq!(
            Windows::from_json(br#"{"serial":5,"windows":[]}"#)
                .unwrap()
                .windows
                .len(),
            0
        );
        // The title is never shown by `{:?}`.
        let shown = format!("{windows:?}");
        assert!(!shown.contains("Save changes"), "{shown}");
    }

    #[test]
    fn a_list_of_windows_out_of_bounds_or_not_of_the_form_is_neither_written_nor_read() {
        let with = |change: &dyn Fn(&mut Window)| {
            let mut window = a_dialog();
            change(&mut window);
            Windows {
                serial: 1,
                windows: vec![window],
            }
        };
        for odd in [
            with(&|w| w.id = 1),
            with(&|w| w.id = 0),
            with(&|w| w.width = MAX_WINDOW_SIDE + 1),
            with(&|w| w.height = u32::MAX),
            with(&|w| w.title = "x".repeat(MAX_WINDOW_TITLE + 1)),
            Windows {
                serial: 1,
                windows: vec![a_dialog(); MAX_WINDOWS + 1],
            },
        ] {
            assert_eq!(
                encode_windows(&odd, 0, &mut Vec::new()),
                Err(WireError::Windows)
            );
            // The same list as somebody else wrote it.
            let json = serde_json::to_vec(&odd).unwrap();
            assert_eq!(decode_windows(&json), Err(WireError::Windows));
        }
        // At the limits it is taken.
        let full = with(&|w| {
            w.width = MAX_WINDOW_SIDE;
            w.title = "x".repeat(MAX_WINDOW_TITLE);
        });
        assert!(encode_windows(&full, 0, &mut Vec::new()).is_ok());
        // Titles within the limit that come to more than 8192 bytes written out.
        let escaped = Windows {
            serial: 1,
            windows: vec![
                Window {
                    title: "\u{1}".repeat(MAX_WINDOW_TITLE),
                    ..a_dialog()
                };
                MAX_WINDOWS
            ],
        };
        assert_eq!(escaped.check(), Ok(()));
        assert_eq!(escaped.to_json(), Err(WireError::Windows));
        // Not of the form, and longer than any list may be.
        for odd in [
            &b"not json"[..],
            br#"{"windows":[]}"#,
            br#"{"serial":1,"windows":[{"id":2}]}"#,
            br#"{"serial":1,"windows":[{"id":2,"port":70000,"width":1,"height":1}]}"#,
            br#"{"serial":-1,"windows":[]}"#,
            &[0xff, 0xfe],
        ] {
            assert_eq!(decode_windows(odd), Err(WireError::Windows));
        }
        let mut long = br#"{"serial":1,"windows":[],"later":""#.to_vec();
        long.resize(9 * 1024, b'x');
        long.extend_from_slice(br#""}"#);
        assert!(serde_json::from_slice::<Windows>(&long).is_ok());
        assert_eq!(decode_windows(&long), Err(WireError::Windows));
    }

    #[test]
    fn a_cursor_comes_back_as_it_was_written() {
        let pixels = arrow();
        for cursor in [
            Cursor::Hidden,
            Cursor::Named("default"),
            Cursor::Named(""),
            Cursor::Image(image(&pixels)),
            Cursor::Image(CursorImage {
                width: 256,
                height: 256,
                hotspot_x: 255,
                hotspot_y: 255,
                scale: 8,
                rgba: &vec![7u8; 256 * 256 * 4],
            }),
        ] {
            let body = cursor_body(&cursor);
            assert_eq!(decode_cursor(&body).unwrap(), cursor);
        }
        assert_eq!(
            cursor_body(&Cursor::Image(image(&pixels))).len(),
            16 + 24 * 16 * 4
        );
    }

    #[test]
    fn a_cursor_that_is_too_large_or_of_no_size_is_neither_written_nor_read() {
        let pixels = vec![0u8; 257 * 257 * 4];
        for (width, height) in [
            (257u16, 16u16),
            (16, 257),
            (0, 16),
            (16, 0),
            (0, 0),
            (u16::MAX, 1),
        ] {
            let odd = Cursor::Image(CursorImage {
                width,
                height,
                hotspot_x: 0,
                hotspot_y: 0,
                scale: 1,
                rgba: &pixels[..usize::from(width) * usize::from(height) * 4 % (pixels.len() + 1)],
            });
            assert_eq!(
                encode_cursor(&odd, 0, &mut Vec::new()),
                Err(WireError::Cursor)
            );
            // The same numbers in a packet that somebody else wrote.
            let mut body = cursor_body(&Cursor::Image(image(&arrow())));
            body[4..6].copy_from_slice(&width.to_le_bytes());
            body[6..8].copy_from_slice(&height.to_le_bytes());
            body.truncate(16);
            body.resize(
                16 + usize::from(width) * usize::from(height) * 4 % (1 << 20),
                0,
            );
            assert_eq!(
                decode_cursor(&body),
                Err(WireError::Cursor),
                "{width}x{height}"
            );
        }
    }

    #[test]
    fn a_cursor_whose_length_does_not_fit_its_size_is_neither_written_nor_read() {
        let pixels = arrow();
        for len in [pixels.len() - 1, pixels.len() - 4, 0] {
            let short = Cursor::Image(image(&pixels[..len]));
            assert_eq!(
                encode_cursor(&short, 0, &mut Vec::new()),
                Err(WireError::Cursor)
            );
        }
        let body = cursor_body(&Cursor::Image(image(&pixels)));
        assert_eq!(
            decode_cursor(&body[..body.len() - 1]),
            Err(WireError::Cursor)
        );
        let mut longer = body.clone();
        longer.push(0);
        assert_eq!(decode_cursor(&longer), Err(WireError::Cursor));
        // Sides that claim more than is there: nothing is read beyond the body.
        let mut wider = body.clone();
        wider[4..6].copy_from_slice(&25u16.to_le_bytes());
        assert_eq!(decode_cursor(&wider), Err(WireError::Cursor));
        // Cut off inside the fixed part, and a fixed part that claims to be longer than the body.
        assert_eq!(decode_cursor(&body[..15]), Err(WireError::Cursor));
        assert_eq!(decode_cursor(&[]), Err(WireError::Cursor));
        let mut claims = body.clone();
        claims[2..4].copy_from_slice(&u16::MAX.to_le_bytes());
        assert_eq!(decode_cursor(&claims), Err(WireError::Cursor));
        let mut claims = body;
        claims[2..4].copy_from_slice(&15u16.to_le_bytes());
        assert_eq!(decode_cursor(&claims), Err(WireError::Cursor));
    }

    #[test]
    fn a_cursor_with_its_hot_spot_outside_or_a_strange_scale_is_neither_written_nor_read() {
        let pixels = arrow();
        for (x, y, scale) in [
            (24u16, 0u16, 1u16),
            (0, 16, 1),
            (u16::MAX, 0, 1),
            (0, 0, 0),
            (0, 0, 9),
        ] {
            let odd = Cursor::Image(CursorImage {
                hotspot_x: x,
                hotspot_y: y,
                scale,
                ..image(&pixels)
            });
            assert_eq!(
                encode_cursor(&odd, 0, &mut Vec::new()),
                Err(WireError::Cursor)
            );
            let mut body = cursor_body(&Cursor::Image(image(&pixels)));
            body[8..10].copy_from_slice(&x.to_le_bytes());
            body[10..12].copy_from_slice(&y.to_le_bytes());
            body[12..14].copy_from_slice(&scale.to_le_bytes());
            assert_eq!(
                decode_cursor(&body),
                Err(WireError::Cursor),
                "{x},{y} x{scale}"
            );
        }
    }

    #[test]
    fn other_cursors_that_do_not_add_up_are_not_read() {
        // A shape this version does not know.
        let mut body = cursor_body(&Cursor::Hidden);
        body[0] = 3;
        assert_eq!(decode_cursor(&body), Err(WireError::Cursor));
        // A hidden cursor that brings something along.
        let mut body = cursor_body(&Cursor::Hidden);
        body.push(1);
        assert_eq!(decode_cursor(&body), Err(WireError::Cursor));
        // A name that is longer or shorter than it says, is too long, or is no text.
        let named = cursor_body(&Cursor::Named("text"));
        assert_eq!(
            decode_cursor(&named[..named.len() - 1]),
            Err(WireError::Cursor)
        );
        let mut no_text = named.clone();
        no_text[16] = 0xff;
        assert_eq!(decode_cursor(&no_text), Err(WireError::Cursor));
        let long = "x".repeat(MAX_CURSOR_NAME + 1);
        assert_eq!(
            encode_cursor(&Cursor::Named(&long), 0, &mut Vec::new()),
            Err(WireError::Cursor)
        );
        let mut body = named;
        body[14..16].copy_from_slice(&((MAX_CURSOR_NAME + 1) as u16).to_le_bytes());
        body.truncate(16);
        body.extend_from_slice(long.as_bytes());
        assert_eq!(decode_cursor(&body), Err(WireError::Cursor));
    }

    #[test]
    fn fields_a_later_version_adds_to_a_cursor_are_passed_over() {
        let pixels = arrow();
        let body = cursor_body(&Cursor::Image(image(&pixels)));
        let mut longer = body[..16].to_vec();
        longer[2..4].copy_from_slice(&20u16.to_le_bytes());
        longer.extend_from_slice(&[0xee; 4]);
        longer.extend_from_slice(&body[16..]);
        assert_eq!(
            decode_cursor(&longer).unwrap(),
            Cursor::Image(image(&pixels))
        );
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
