//! What the test application and the tools that measure it agree on.
//!
//! The test application (`istesso-flash`) fills its window with one of two
//! shades and switches to the other on every input. A measuring client looks
//! at the frames it decodes and takes the first one in the new shade as the
//! frame that reacted to its input.
//!
//! For measurements the application can also take *synthetic inputs*: small
//! UDP datagrams that stand in for a key press, so that a first end-to-end
//! path can be measured before real input injection exists. That path is
//! neither authenticated nor encrypted. It is for measurements on a trusted
//! network only and must not become part of the product (`docs/m1-plan.md`,
//! step 3).

/// UDP port on which the test application takes synthetic inputs when asked to.
pub const DEFAULT_FLASH_PORT: u16 = 47811;

/// A colour, eight bits per channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Brightness from 0 to 255 (BT.709 weights).
    pub fn luma(self) -> u8 {
        let weighted =
            2126 * u32::from(self.r) + 7152 * u32::from(self.g) + 722 * u32::from(self.b);
        ((weighted + 5_000) / 10_000) as u8
    }
}

/// The two states of the test application's window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shade {
    Dark,
    Light,
}

// Both shades are greys inside the range that video keeps (16 to 235), so
// that neither is clipped on the way and only brightness has to survive the
// colour conversion of a codec.
const DARK: Rgb = Rgb::new(0x20, 0x20, 0x20);
const LIGHT: Rgb = Rgb::new(0xE0, 0xE0, 0xE0);

impl Shade {
    /// The other shade.
    pub fn toggled(self) -> Shade {
        match self {
            Shade::Dark => Shade::Light,
            Shade::Light => Shade::Dark,
        }
    }

    /// The colour the window is filled with.
    pub fn rgb(self) -> Rgb {
        match self {
            Shade::Dark => DARK,
            Shade::Light => LIGHT,
        }
    }
}

/// Decides which shade a decoded frame shows, from the colour of a sample of
/// its pixels.
///
/// A frame counts as a shade when its brightness is within a quarter of the
/// way from that shade to the other. Anything in between (a blend of two
/// frames, or a picture that is not the test application) is `None`.
pub fn classify(sample: Rgb) -> Option<Shade> {
    let dark = u32::from(DARK.luma());
    let light = u32::from(LIGHT.luma());
    let quarter = (light - dark) / 4;
    let luma = u32::from(sample.luma());
    if luma <= dark + quarter {
        Some(Shade::Dark)
    } else if luma >= light - quarter {
        Some(Shade::Light)
    } else {
        None
    }
}

const INPUT_MAGIC: &[u8; 4] = b"IFI1";
const ACK_MAGIC: &[u8; 4] = b"IFA1";

/// A synthetic input: the client's number for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlashInput {
    pub id: u64,
}

impl FlashInput {
    pub const LEN: usize = 12;

    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut bytes = [0u8; Self::LEN];
        bytes[..4].copy_from_slice(INPUT_MAGIC);
        bytes[4..].copy_from_slice(&self.id.to_be_bytes());
        bytes
    }

    /// `None` for anything that is not exactly one synthetic input.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::LEN || &bytes[..4] != INPUT_MAGIC {
            return None;
        }
        Some(Self {
            id: read_u64(&bytes[4..12]),
        })
    }
}

/// The test application's answer to a synthetic input, sent once the frame
/// that shows it has been committed. Both times are on the host's clock, so
/// their difference is the application's own response time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlashAck {
    pub id: u64,
    pub input_ns: u64,
    pub commit_ns: u64,
}

impl FlashAck {
    pub const LEN: usize = 28;

    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut bytes = [0u8; Self::LEN];
        bytes[..4].copy_from_slice(ACK_MAGIC);
        bytes[4..12].copy_from_slice(&self.id.to_be_bytes());
        bytes[12..20].copy_from_slice(&self.input_ns.to_be_bytes());
        bytes[20..28].copy_from_slice(&self.commit_ns.to_be_bytes());
        bytes
    }

    /// `None` for anything that is not exactly one answer.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != Self::LEN || &bytes[..4] != ACK_MAGIC {
            return None;
        }
        Some(Self {
            id: read_u64(&bytes[4..12]),
            input_ns: read_u64(&bytes[12..20]),
            commit_ns: read_u64(&bytes[20..28]),
        })
    }
}

fn read_u64(bytes: &[u8]) -> u64 {
    let mut array = [0u8; 8];
    array.copy_from_slice(bytes);
    u64::from_be_bytes(array)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_input_switches_the_shade() {
        assert_eq!(Shade::Dark.toggled(), Shade::Light);
        assert_eq!(Shade::Light.toggled(), Shade::Dark);
        assert_ne!(Shade::Dark.rgb(), Shade::Light.rgb());
    }

    #[test]
    fn the_exact_shades_are_recognised() {
        assert_eq!(classify(Shade::Dark.rgb()), Some(Shade::Dark));
        assert_eq!(classify(Shade::Light.rgb()), Some(Shade::Light));
    }

    #[test]
    fn shades_survive_what_a_codec_does_to_them() {
        // Brightness off by a few steps and a colour cast, as after a
        // conversion to limited-range YUV and back.
        assert_eq!(classify(Rgb::new(0x29, 0x24, 0x1c)), Some(Shade::Dark));
        assert_eq!(classify(Rgb::new(0xd6, 0xdc, 0xe4)), Some(Shade::Light));
        assert_eq!(classify(Rgb::new(0, 0, 0)), Some(Shade::Dark));
        assert_eq!(classify(Rgb::new(255, 255, 255)), Some(Shade::Light));
    }

    #[test]
    fn a_blend_of_both_shades_is_neither() {
        assert_eq!(classify(Rgb::new(0x80, 0x80, 0x80)), None);
        assert_eq!(classify(Rgb::new(0x60, 0x60, 0x60)), None);
        assert_eq!(classify(Rgb::new(0xa0, 0xa0, 0xa0)), None);
    }

    #[test]
    fn luma_follows_green_most() {
        assert_eq!(Rgb::new(255, 255, 255).luma(), 255);
        assert_eq!(Rgb::new(0, 0, 0).luma(), 0);
        assert!(Rgb::new(0, 255, 0).luma() > Rgb::new(255, 0, 0).luma());
        assert!(Rgb::new(255, 0, 0).luma() > Rgb::new(0, 0, 255).luma());
    }

    #[test]
    fn datagrams_read_back_as_written() {
        let input = FlashInput {
            id: 0x0102_0304_0506_0708,
        };
        assert_eq!(FlashInput::decode(&input.encode()), Some(input));
        assert_eq!(
            input.encode(),
            [b'I', b'F', b'I', b'1', 1, 2, 3, 4, 5, 6, 7, 8]
        );

        let ack = FlashAck {
            id: 9,
            input_ns: 1_000_000,
            commit_ns: 1_250_000,
        };
        assert_eq!(FlashAck::decode(&ack.encode()), Some(ack));
    }

    #[test]
    fn anything_else_is_rejected() {
        let input = FlashInput { id: 1 }.encode();
        let ack = FlashAck {
            id: 1,
            input_ns: 2,
            commit_ns: 3,
        }
        .encode();
        assert_eq!(FlashInput::decode(&[]), None);
        assert_eq!(FlashInput::decode(&input[..11]), None);
        assert_eq!(FlashInput::decode(&ack), None);
        assert_eq!(FlashAck::decode(&input), None);
        assert_eq!(FlashAck::decode(&ack[..27]), None);

        let mut longer = input.to_vec();
        longer.push(0);
        assert_eq!(FlashInput::decode(&longer), None);

        let mut wrong_magic = input;
        wrong_magic[3] = b'2';
        assert_eq!(FlashInput::decode(&wrong_magic), None);
    }
}
