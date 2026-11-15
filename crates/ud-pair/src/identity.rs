//! The long-term identity of a device: an Ed25519 key pair.

use std::path::Path;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rand_core::OsRng;
use sha2::{Digest, Sha256};

use crate::{from_hex, to_hex, Error, Result};

/// A device's private identity key. Never leaves the device.
pub struct Identity {
    signing: SigningKey,
}

impl Identity {
    /// Creates a new random identity.
    pub fn generate() -> Self {
        Identity {
            signing: SigningKey::generate(&mut OsRng),
        }
    }

    /// Rebuilds an identity from its 32-byte seed.
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Identity {
            signing: SigningKey::from_bytes(&seed),
        }
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.signing.verifying_key().to_bytes()
    }

    /// The short form of the public key that people can compare.
    pub fn fingerprint(&self) -> String {
        fingerprint(&self.public_key())
    }

    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.signing.sign(message).to_bytes()
    }

    /// Loads the identity stored at `path`, creating and saving a new one
    /// when the file does not exist yet.
    ///
    /// The seed is kept in a file readable only by its owner. Moving it into
    /// the OS keychain is planned (see `docs/adr/0006-pairing.md`).
    pub fn load_or_create(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let seed: [u8; 32] = from_hex(text.trim())
                    .and_then(|bytes| bytes.try_into().ok())
                    .ok_or_else(|| {
                        Error::Corrupt(format!("identity file {} is damaged", path.display()))
                    })?;
                Ok(Identity::from_seed(seed))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let identity = Identity::generate();
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                write_private(path, to_hex(&identity.signing.to_bytes()).as_bytes())?;
                Ok(identity)
            }
            Err(err) => Err(err.into()),
        }
    }
}

/// Writes a file that only its owner can read.
pub(crate) fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

/// Checks `signature` over `message` against `public_key`.
pub fn verify(public_key: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool {
    let Ok(key) = VerifyingKey::from_bytes(public_key) else {
        return false;
    };
    key.verify_strict(message, &Signature::from_bytes(signature))
        .is_ok()
}

/// A short, comparable form of a public key: `1a2b-3c4d-5e6f-7a8b`.
pub fn fingerprint(public_key: &[u8; 32]) -> String {
    let digest = Sha256::digest(public_key);
    let hex = to_hex(&digest[..8]);
    hex.as_bytes()
        .chunks(4)
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_verify_only_with_the_right_key_and_message() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let signature = alice.sign(b"hello");
        assert!(verify(&alice.public_key(), b"hello", &signature));
        assert!(!verify(&alice.public_key(), b"hellp", &signature));
        assert!(!verify(&bob.public_key(), b"hello", &signature));
    }

    #[test]
    fn seed_restores_the_same_identity() {
        let identity = Identity::from_seed([7u8; 32]);
        let again = Identity::from_seed([7u8; 32]);
        assert_eq!(identity.public_key(), again.public_key());
        assert_eq!(identity.fingerprint(), again.fingerprint());
    }

    #[test]
    fn fingerprint_is_short_and_grouped() {
        let fp = Identity::from_seed([1u8; 32]).fingerprint();
        assert_eq!(fp.len(), 19);
        assert_eq!(fp.matches('-').count(), 3);
        assert_ne!(fp, Identity::from_seed([2u8; 32]).fingerprint());
    }

    #[test]
    fn load_or_create_persists_the_identity() {
        let dir = std::env::temp_dir().join(format!("ud-pair-identity-{}", std::process::id()));
        let path = dir.join("nested/identity.key");
        let first = Identity::load_or_create(&path).unwrap();
        let second = Identity::load_or_create(&path).unwrap();
        assert_eq!(first.public_key(), second.public_key());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        std::fs::write(&path, "not hex").unwrap();
        assert!(matches!(
            Identity::load_or_create(&path),
            Err(Error::Corrupt(_))
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
