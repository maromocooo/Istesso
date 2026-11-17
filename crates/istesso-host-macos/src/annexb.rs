//! From what VideoToolbox hands out to what the picture's transport carries.
//!
//! The encoder gives a frame as NAL units, each behind its length, and keeps
//! the parameter sets (VPS, SPS, PPS) aside in the format description. The
//! transport (`istesso-media`, ADR 0015) carries one access unit per frame
//! with the NAL units behind start codes (Annex B), and a key frame with its
//! parameter sets in front.

/// The start code put in front of every NAL unit.
pub const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// What can be wrong with the encoder's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The length field is not 1, 2 or 4 bytes wide.
    LengthSize(usize),
    /// A length field, or the unit it announces, runs past the end.
    Truncated,
    /// A NAL unit of no bytes.
    Empty,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::LengthSize(n) => write!(f, "NAL lengths of {n} bytes are not known"),
            Error::Truncated => write!(f, "the encoder's frame ends in the middle of a NAL unit"),
            Error::Empty => write!(f, "the encoder's frame holds an empty NAL unit"),
        }
    }
}

impl std::error::Error for Error {}

/// Appends `sample` (NAL units behind big-endian lengths of `length_size`
/// bytes) to `out` as Annex B. For a key frame, `parameter_sets` go first.
/// On an error `out` is left as it was.
pub fn append_access_unit(
    sample: &[u8],
    length_size: usize,
    parameter_sets: &[Vec<u8>],
    out: &mut Vec<u8>,
) -> Result<(), Error> {
    if !matches!(length_size, 1 | 2 | 4) {
        return Err(Error::LengthSize(length_size));
    }
    let before = out.len();
    let result = (|| {
        for set in parameter_sets {
            if set.is_empty() {
                return Err(Error::Empty);
            }
            out.extend_from_slice(&START_CODE);
            out.extend_from_slice(set);
        }
        let mut rest = sample;
        while !rest.is_empty() {
            if rest.len() < length_size {
                return Err(Error::Truncated);
            }
            let (length, tail) = rest.split_at(length_size);
            let length = length.iter().fold(0usize, |n, &b| (n << 8) | b as usize);
            if length == 0 {
                return Err(Error::Empty);
            }
            if tail.len() < length {
                return Err(Error::Truncated);
            }
            out.extend_from_slice(&START_CODE);
            out.extend_from_slice(&tail[..length]);
            rest = &tail[length..];
        }
        Ok(())
    })();
    if result.is_err() {
        out.truncate(before);
    }
    result
}

/// The type of an H.265 NAL unit (the six bits after the first).
pub fn nal_type(nal: &[u8]) -> Option<u8> {
    nal.first().map(|b| (b >> 1) & 0x3f)
}

/// NAL unit type of a sequence parameter set.
pub const NAL_SPS: u8 = 33;

/// `general_profile_idc` of an H.265 sequence parameter set: 1 is Main, 2
/// Main 10, 4 the range extensions (which 4:4:4 needs). `None` when `sps` is
/// not a sequence parameter set or is too short.
pub fn general_profile_idc(sps: &[u8]) -> Option<u8> {
    if nal_type(sps)? != NAL_SPS {
        return None;
    }
    // Two bytes of NAL header, one byte of ids, then profile space (2 bits),
    // tier (1 bit) and the profile (5 bits). No emulation prevention can
    // fall this early: it needs two zero bytes in front.
    sps.get(3).map(|b| b & 0x1f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_get_start_codes_and_parameter_sets_go_first() {
        let sample = [0, 0, 0, 2, 0x26, 0x01, 0, 0, 0, 1, 0x02];
        let sets = vec![vec![0x40, 0x01], vec![0x42, 0x01], vec![0x44, 0x01]];
        let mut out = vec![9];
        append_access_unit(&sample, 4, &sets, &mut out).unwrap();
        assert_eq!(
            out,
            [
                9, 0, 0, 0, 1, 0x40, 0x01, 0, 0, 0, 1, 0x42, 0x01, 0, 0, 0, 1, 0x44, 0x01, 0, 0, 0,
                1, 0x26, 0x01, 0, 0, 0, 1, 0x02
            ]
        );
    }

    #[test]
    fn shorter_length_fields_are_read() {
        let mut out = Vec::new();
        append_access_unit(&[0, 1, 0xaa, 0, 2, 0xbb, 0xcc], 2, &[], &mut out).unwrap();
        assert_eq!(out, [0, 0, 0, 1, 0xaa, 0, 0, 0, 1, 0xbb, 0xcc]);
    }

    #[test]
    fn a_cut_frame_leaves_the_output_alone() {
        let mut out = vec![1, 2, 3];
        assert_eq!(
            append_access_unit(&[0, 0, 0, 5, 1, 2], 4, &[vec![0x40]], &mut out),
            Err(Error::Truncated)
        );
        assert_eq!(out, [1, 2, 3]);
        assert_eq!(
            append_access_unit(&[0, 0, 0], 4, &[], &mut out),
            Err(Error::Truncated)
        );
        assert_eq!(
            append_access_unit(&[0, 0, 0, 0], 4, &[], &mut out),
            Err(Error::Empty)
        );
        assert_eq!(
            append_access_unit(&[], 3, &[], &mut out),
            Err(Error::LengthSize(3))
        );
        assert_eq!(out, [1, 2, 3]);
    }

    #[test]
    fn the_profile_is_read_from_a_sequence_parameter_set() {
        // NAL type 33 (0x42), then ids, then space 0, tier 0, profile 4.
        assert_eq!(
            general_profile_idc(&[0x42, 0x01, 0x01, 0x04, 0x08]),
            Some(4)
        );
        assert_eq!(general_profile_idc(&[0x42, 0x01, 0x01, 0x21]), Some(1));
        assert_eq!(general_profile_idc(&[0x40, 0x01, 0x01, 0x04]), None);
        assert_eq!(general_profile_idc(&[0x42, 0x01]), None);
        assert_eq!(general_profile_idc(&[]), None);
    }
}
