//! The short one-time code a person reads from one screen and types into
//! another.

use std::fmt;

use rand_core::{OsRng, RngCore};

use crate::{Error, Result};

/// Crockford base32: no I, L, O or U, so codes survive being read aloud.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Number of characters in a code. Eight base32 characters carry 40 bits.
/// That is enough because SPAKE2 allows only one online guess per code.
pub const CODE_LEN: usize = 8;

/// A pairing code in canonical form (upper case, no separator).
#[derive(Clone, PartialEq, Eq)]
pub struct PairCode(String);

impl PairCode {
    /// Creates a fresh random code.
    pub fn generate() -> Self {
        let mut bytes = [0u8; CODE_LEN];
        OsRng.fill_bytes(&mut bytes);
        // 256 is a multiple of 32, so masking keeps the distribution uniform.
        let code = bytes
            .iter()
            .map(|b| ALPHABET[(b & 31) as usize] as char)
            .collect();
        PairCode(code)
    }

    /// Accepts what a person typed: any case, with or without the dash, and
    /// with the look-alike letters O, I and L.
    pub fn parse(input: &str) -> Result<Self> {
        let mut code = String::with_capacity(CODE_LEN);
        for c in input.chars() {
            let c = match c.to_ascii_uppercase() {
                '-' | ' ' => continue,
                'O' => '0',
                'I' | 'L' => '1',
                other => other,
            };
            if !c.is_ascii() || !ALPHABET.contains(&(c as u8)) {
                return Err(Error::InvalidCode(format!("unexpected character '{c}'")));
            }
            code.push(c);
        }
        if code.len() != CODE_LEN {
            return Err(Error::InvalidCode(format!(
                "expected {CODE_LEN} characters, got {}",
                code.len()
            )));
        }
        Ok(PairCode(code))
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

/// Shows the code the way it should be read: `ABCD-EFGH`.
impl fmt::Display for PairCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (left, right) = self.0.split_at(CODE_LEN / 2);
        write!(f, "{left}-{right}")
    }
}

/// Keeps the secret out of logs and panic messages.
impl fmt::Debug for PairCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PairCode(****-****)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_codes_parse_back() {
        for _ in 0..50 {
            let code = PairCode::generate();
            let shown = code.to_string();
            assert_eq!(shown.len(), CODE_LEN + 1);
            assert_eq!(PairCode::parse(&shown).unwrap(), code);
        }
    }

    #[test]
    fn generated_codes_differ() {
        assert_ne!(PairCode::generate(), PairCode::generate());
    }

    #[test]
    fn parse_forgives_typing_habits() {
        let canonical = PairCode::parse("0123-ABCD").unwrap();
        assert_eq!(PairCode::parse("0123abcd").unwrap(), canonical);
        assert_eq!(PairCode::parse(" o12 3-a bcd ").unwrap(), canonical);
        assert_eq!(
            PairCode::parse("OI23-ABCD").unwrap(),
            PairCode::parse("0123-ABCD").unwrap()
        );
        assert_eq!(PairCode::parse("0L23-ABCD").unwrap(), canonical);
    }

    #[test]
    fn parse_rejects_bad_codes() {
        assert!(matches!(PairCode::parse(""), Err(Error::InvalidCode(_))));
        assert!(matches!(PairCode::parse("ABC"), Err(Error::InvalidCode(_))));
        assert!(matches!(
            PairCode::parse("0123-ABCD-9"),
            Err(Error::InvalidCode(_))
        ));
        // U is not part of the alphabet.
        assert!(matches!(
            PairCode::parse("UUUU-UUUU"),
            Err(Error::InvalidCode(_))
        ));
        assert!(matches!(
            PairCode::parse("あいうえ-おかきく"),
            Err(Error::InvalidCode(_))
        ));
    }

    #[test]
    fn debug_output_hides_the_code() {
        let code = PairCode::parse("0123-ABCD").unwrap();
        let debug = format!("{code:?}");
        assert!(!debug.contains("0123"));
        assert!(!debug.contains("ABCD"));
    }
}
