//! Secure pairing between devices.
//!
//! Pairing turns "these two machines are in front of me" into a lasting,
//! verifiable relationship:
//!
//! 1. One device shows a short one-time code ([`PairCode`]).
//! 2. Both devices run SPAKE2 with that code. An eavesdropper learns nothing
//!    that lets them test guesses offline, and a device that does not know
//!    the code ends up with a different key.
//! 3. Under the agreed key each device sends its long-term public key and a
//!    signature proving it owns the matching private key ([`handshake`]).
//! 4. Each side stores the other's public key ([`store`]). Later connections
//!    are authenticated against these pinned keys; removing a peer from the
//!    store revokes it.
//!
//! The device's own key and the list of paired devices are reached through
//! one door, [`Keeping`], whatever form they are kept in.
//!
//! The cryptographic building blocks come from the RustCrypto and dalek
//! crates. `spake2` has not had an independent audit; this crate keeps the
//! protocol small on purpose and should be reviewed before a public release
//! (see `docs/adr/0006-pairing.md`).

pub mod code;
pub mod handshake;
pub mod identity;
pub mod keeping;
pub mod store;
pub mod tcp;

pub use code::PairCode;
pub use handshake::{Hello, Pairing, Role, Session};
pub use identity::{fingerprint, Identity};
pub use keeping::{Form, Keeping};
pub use store::{Peer, PeerStore, Permission};

/// Everything that can go wrong while pairing.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the pairing code is not valid: {0}")]
    InvalidCode(String),
    #[error("the device name is not valid: {0}")]
    InvalidName(String),
    #[error("pairing failed: the codes did not match or a message was altered")]
    Mismatch,
    #[error("the other device sent a malformed message")]
    Malformed,
    #[error("the other device could not prove that it owns its key")]
    BadSignature,
    #[error("this pairing session was already used")]
    AlreadyUsed,
    #[error("timed out waiting for the other device")]
    Timeout,
    #[error("stored data is corrupt: {0}")]
    Corrupt(String),
    /// A key or a list of paired devices lies where this build does not
    /// keep them: it is not used, and nothing goes on until the person has
    /// taken it away.
    #[error("{0}")]
    LeftInAFile(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

pub(crate) fn from_hex(text: &str) -> Option<Vec<u8>> {
    // Checking every character also keeps out the sign that `from_str_radix`
    // would otherwise accept ("+f" is not hex).
    if text.len() % 2 == 1 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

pub(crate) fn hex_array<const N: usize>(text: &str) -> Option<[u8; N]> {
    from_hex(text)?.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        let bytes = [0x00, 0x0f, 0xa5, 0xff];
        assert_eq!(to_hex(&bytes), "000fa5ff");
        assert_eq!(from_hex("000fa5ff").unwrap(), bytes);
        assert_eq!(hex_array::<4>("000fa5ff"), Some(bytes));
    }

    #[test]
    fn hex_rejects_bad_input() {
        assert_eq!(from_hex("abc"), None);
        assert_eq!(from_hex("zz"), None);
        assert_eq!(from_hex("é1"), None);
        assert_eq!(from_hex("+f"), None);
        assert_eq!(from_hex("0x"), None);
        assert_eq!(hex_array::<4>("0011"), None);
    }
}
