//! Length-prefixed frames: two bytes of length, then that many bytes.

use std::io::{Read, Write};

use crate::{Error, Result};

/// Largest frame on the wire. The limit comes from the Noise specification,
/// which caps every message at 65535 bytes.
pub(crate) const MAX_FRAME: usize = 65535;

/// Appends one frame to `wire`.
pub(crate) fn encode_frame(wire: &mut Vec<u8>, payload: &[u8]) {
    debug_assert!(payload.len() <= MAX_FRAME);
    wire.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    wire.extend_from_slice(payload);
}

/// Writes one frame in a single call, so that it leaves in one packet.
pub(crate) fn write_frame<W: Write>(writer: &mut W, payload: &[u8]) -> Result<()> {
    let mut wire = Vec::with_capacity(2 + payload.len());
    encode_frame(&mut wire, payload);
    write_all(writer, &wire)
}

pub(crate) fn write_all<W: Write>(writer: &mut W, bytes: &[u8]) -> Result<()> {
    writer
        .write_all(bytes)
        .and_then(|_| writer.flush())
        .map_err(Error::from_io)
}

/// Reads one frame, refusing frames longer than `limit`.
pub(crate) fn read_frame<R: Read>(reader: &mut R, limit: usize) -> Result<Vec<u8>> {
    let mut len = [0u8; 2];
    reader.read_exact(&mut len).map_err(Error::from_io)?;
    let len = u16::from_be_bytes(len) as usize;
    if len > limit {
        return Err(Error::Malformed);
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).map_err(Error::from_io)?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let mut wire = Vec::new();
        write_frame(&mut wire, b"hello").unwrap();
        write_frame(&mut wire, b"").unwrap();
        assert_eq!(wire, [0, 5, b'h', b'e', b'l', b'l', b'o', 0, 0]);

        let mut reader = wire.as_slice();
        assert_eq!(read_frame(&mut reader, MAX_FRAME).unwrap(), b"hello");
        assert_eq!(read_frame(&mut reader, MAX_FRAME).unwrap(), b"");
        assert!(matches!(
            read_frame(&mut reader, MAX_FRAME),
            Err(Error::Closed)
        ));
    }

    #[test]
    fn long_and_cut_off_frames_are_refused() {
        let mut wire = Vec::new();
        write_frame(&mut wire, &[7u8; 100]).unwrap();
        assert!(matches!(
            read_frame(&mut wire.as_slice(), 99),
            Err(Error::Malformed)
        ));
        // The length promises more than what arrives.
        assert!(matches!(
            read_frame(&mut &wire[..50], MAX_FRAME),
            Err(Error::Closed)
        ));
    }
}
