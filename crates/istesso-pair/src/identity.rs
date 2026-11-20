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
                if let Some(parent) = path.parent() {
                    warn_once_if_open(parent);
                }
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
                    create_private_dir(parent)?;
                }
                write_private(path, to_hex(&identity.signing.to_bytes()).as_bytes())?;
                Ok(identity)
            }
            Err(err) => Err(err.into()),
        }
    }
}

/// Makes the folder that holds the identity and the list of paired devices,
/// for its owner only (0700), when it is not there yet. A folder that is
/// already there is left as it is, and warned about once when others may write
/// into it.
pub(crate) fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    if dir.as_os_str().is_empty() {
        return Ok(());
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)?;
    warn_once_if_open(dir);
    Ok(())
}

/// The mode of `dir` when its group or other users may write into it: whoever
/// can do that can put a device of their own into the list of paired devices,
/// or replace this device's identity.
#[cfg(unix)]
pub(crate) fn open_to_others(dir: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(dir).ok()?.permissions().mode() & 0o7777;
    (mode & 0o022 != 0).then_some(mode)
}

/// Says once in a process, on the standard error, that the folder `dir` is
/// open to others. Nothing is changed: the folder is the owner's.
pub(crate) fn warn_once_if_open(dir: &Path) {
    #[cfg(unix)]
    {
        static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if WARNED.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        if let Some(mode) = open_to_others(dir) {
            if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!(
                    "warning: other users may write into {} (mode {mode:o}); `chmod 700` it so that nobody else can add a paired device or replace this device's identity",
                    dir.display()
                );
            }
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
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

    #[cfg(unix)]
    #[test]
    fn a_new_folder_is_made_for_its_owner_only_and_an_old_one_is_left_alone() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o7777;
        let base =
            std::env::temp_dir().join(format!("istesso-pair-identity-mode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // Made here, two levels deep: both levels are the owner's only.
        let fresh = base.join("made").join("istesso");
        Identity::load_or_create(&fresh.join("identity.key")).unwrap();
        assert_eq!(mode(&fresh) & 0o077, 0, "{:o}", mode(&fresh));
        assert_eq!(mode(fresh.parent().unwrap()) & 0o077, 0);
        assert_eq!(open_to_others(&fresh), None);

        // Already there and open to the group: kept as it was, and told as open.
        let open = base.join("open");
        std::fs::create_dir_all(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o770)).unwrap();
        Identity::load_or_create(&open.join("identity.key")).unwrap();
        Identity::load_or_create(&open.join("identity.key")).unwrap();
        assert_eq!(mode(&open), 0o770);
        assert_eq!(open_to_others(&open), Some(0o770));
        // Writable by others only, too.
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o702)).unwrap();
        assert_eq!(open_to_others(&open), Some(0o702));
        // Readable by others is not what this is about: they cannot change anything.
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(open_to_others(&open), None);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn load_or_create_persists_the_identity() {
        let dir =
            std::env::temp_dir().join(format!("istesso-pair-identity-{}", std::process::id()));
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
