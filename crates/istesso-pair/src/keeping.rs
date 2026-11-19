//! The one door to this device's key and to its list of paired devices.
//!
//! Whoever holds the key is this device to every device it is paired with,
//! and whoever can write the list decides which devices may see and steer
//! this one. Both are therefore reached in one place only, [`Keeping`], so
//! that the form they are kept in can change without anything else changing
//! with it (`docs/adr/0006-pairing.md`, "鍵とペアの一覧の置き場所").
//!
//! Two forms:
//!
//! * **Files** ([`Form::Files`]), the default: `identity.key` and
//!   `peers.json`, readable by their owner only. Every program of the same
//!   user can read and change them; [`Keeping::open_to_the_same_user`] says
//!   so, and `istesso doctor` and the agent pass it on.
//! * **The login keychain** ([`Form::LoginKeychain`]), macOS only, in builds
//!   made with the `login-keychain` feature: the key and the list are two
//!   items of the login keychain, made with the system's default rule (the
//!   program that made an item reads it without a question; any other
//!   program makes the system ask the person). A key and a list found in the
//!   files of an earlier build are moved in and the files removed; the key
//!   is the same, so pairings stay.
//!
//! The form is decided when the program is built, never by an argument, an
//! environment variable or a file: a program that holds rights takes nothing
//! from whoever starts it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::store::{PeerStore, Place, StoreLock};
use crate::{Error, Identity, Result};

/// The form the key and the list are kept in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// Two files that only their owner can read: every program of the same
    /// user.
    Files,
    /// Two items of the login keychain (macOS).
    LoginKeychain,
}

/// The two things that are kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Item {
    /// The seed of the device's key, as 64 hexadecimal digits.
    Key,
    /// The list of paired devices, as the JSON the file form writes.
    Peers,
}

/// A place other than files that keeps the two items.
pub(crate) trait Vault: Send + Sync + std::fmt::Debug {
    /// What is kept as `item`; `None` when nothing is.
    fn get(&self, item: Item) -> Result<Option<Vec<u8>>>;
    /// Keeps `value` as `item`, in place of what was kept before.
    fn set(&self, item: Item, value: &[u8]) -> Result<()>;
}

/// Where this device's key and its list of paired devices are kept, and the
/// only way to them.
#[derive(Debug, Clone)]
pub struct Keeping {
    identity: PathBuf,
    peers: PathBuf,
    vault: Option<Arc<dyn Vault>>,
}

impl Keeping {
    /// The file form: the key in the file `identity`, the list in the file
    /// `peers`. What tests and development builds use, and every build that
    /// was not made to keep them elsewhere.
    pub fn in_files(identity: impl Into<PathBuf>, peers: impl Into<PathBuf>) -> Self {
        Keeping {
            identity: identity.into(),
            peers: peers.into(),
            vault: None,
        }
    }

    /// The file form, with both files in `dir` under their usual names.
    pub fn in_dir(dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref();
        Keeping::in_files(dir.join("identity.key"), dir.join("peers.json"))
    }

    /// The form this build keeps the account's own key and list in: the
    /// login keychain in a macOS build made with `login-keychain`, the two
    /// files otherwise. `identity` and `peers` are where the file form keeps
    /// them; a build that keeps them elsewhere moves in what it finds there.
    pub fn of_this_build(identity: impl Into<PathBuf>, peers: impl Into<PathBuf>) -> Self {
        let files = Keeping::in_files(identity, peers);
        #[cfg(all(feature = "login-keychain", target_os = "macos"))]
        let files = Keeping {
            vault: Some(Arc::new(login_keychain::LoginKeychain::of_this_device())),
            ..files
        };
        files
    }

    /// The form [`Keeping::of_this_build`] gives.
    pub const FORM_OF_THIS_BUILD: Form =
        if cfg!(all(feature = "login-keychain", target_os = "macos")) {
            Form::LoginKeychain
        } else {
            Form::Files
        };

    #[cfg(test)]
    pub(crate) fn in_vault(
        identity: impl Into<PathBuf>,
        peers: impl Into<PathBuf>,
        vault: Arc<dyn Vault>,
    ) -> Self {
        Keeping {
            vault: Some(vault),
            ..Keeping::in_files(identity, peers)
        }
    }

    pub fn form(&self) -> Form {
        match self.vault {
            None => Form::Files,
            Some(_) => Form::LoginKeychain,
        }
    }

    /// What this form does not guard against, in one sentence for the
    /// person; `None` for a form that asks the person before another program
    /// reads the key.
    pub fn open_to_the_same_user(&self) -> Option<&'static str> {
        match self.form() {
            Form::Files => Some(
                "this build keeps the device key and the list of paired devices in files \
                 that every program of the same user can read and change",
            ),
            Form::LoginKeychain => None,
        }
    }

    /// This device's identity. One is made, and kept, when there is none
    /// yet.
    pub fn identity(&self) -> Result<Identity> {
        match &self.vault {
            None => Identity::load_or_create(&self.identity),
            Some(vault) => self
                .identity_in(vault.as_ref(), true)?
                .ok_or_else(|| Error::Corrupt("no device key could be made".into())),
        }
    }

    /// This device's identity, if it has one. None is made: a device
    /// without one is paired with nobody.
    pub fn identity_if_any(&self) -> Result<Option<Identity>> {
        match &self.vault {
            None => Identity::read_file(&self.identity),
            Some(vault) => self.identity_in(vault.as_ref(), false),
        }
    }

    /// The list of paired devices, as it is kept now.
    pub fn peers(&self) -> Result<PeerStore> {
        PeerStore::open_in(match &self.vault {
            None => Place::File(self.peers.clone()),
            Some(vault) => Place::Vault {
                vault: vault.clone(),
                file: self.peers.clone(),
            },
        })
    }

    /// The key kept in `vault`. When nothing is kept there yet, the key an
    /// earlier build left in its file is moved in; without a file, one is
    /// made if `make` says so.
    fn identity_in(&self, vault: &dyn Vault, make: bool) -> Result<Option<Identity>> {
        let kept = |bytes: Vec<u8>| {
            std::str::from_utf8(&bytes)
                .ok()
                .and_then(Identity::from_seed_hex)
                .ok_or_else(|| Error::Corrupt("the kept device key is damaged".into()))
        };
        if let Some(bytes) = vault.get(Item::Key)? {
            return kept(bytes).map(Some);
        }
        if !make && !self.identity.exists() {
            return Ok(None);
        }
        // Moved in or made by one process at a time; another may have been
        // first.
        if let Some(parent) = self.identity.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _lock = StoreLock::take(&self.identity)?;
        if let Some(bytes) = vault.get(Item::Key)? {
            return kept(bytes).map(Some);
        }
        let (identity, from_file) = match Identity::read_file(&self.identity)? {
            Some(identity) => (identity, true),
            None if make => (Identity::generate(), false),
            None => return Ok(None),
        };
        let seed = identity.seed_hex();
        vault.set(Item::Key, seed.as_bytes())?;
        // The file goes only once the key can be read back.
        if vault.get(Item::Key)?.as_deref() != Some(seed.as_bytes()) {
            return Err(Error::Corrupt(
                "the device key did not arrive where it is kept; nothing was removed".into(),
            ));
        }
        if from_file {
            std::fs::remove_file(&self.identity)?;
        }
        Ok(Some(identity))
    }
}

/// The login keychain of the person's account, through the system's
/// Security framework, in this process. Nothing here is run by the tests:
/// it would add items to the keychain of whoever runs them.
#[cfg(all(feature = "login-keychain", target_os = "macos"))]
mod login_keychain {
    use security_framework::passwords::{get_generic_password, set_generic_password};

    use super::{Item, Vault};
    use crate::{Error, Result};

    /// The name both items are filed under ("where" in Keychain Access).
    const SERVICE: &str = "istesso";

    /// `errSecItemNotFound`: nothing is kept under that name.
    const NOT_FOUND: i32 = -25300;

    /// The name of one item ("account" in Keychain Access).
    pub(super) const fn account(item: Item) -> &'static str {
        match item {
            Item::Key => "device key",
            Item::Peers => "paired devices",
        }
    }

    /// Why the keychain did not do what was asked: the system's number
    /// only, never what is kept.
    fn refused(what: &str, item: Item, code: i32) -> Error {
        Error::Io(std::io::Error::other(format!(
            "the login keychain did not {what} \"{}\" (status {code})",
            account(item)
        )))
    }

    #[derive(Debug)]
    pub(super) struct LoginKeychain {
        /// The name the items are filed under: [`SERVICE`], except in the
        /// test that is run by hand.
        service: &'static str,
    }

    impl LoginKeychain {
        pub(super) fn of_this_device() -> Self {
            LoginKeychain { service: SERVICE }
        }
    }

    impl Vault for LoginKeychain {
        fn get(&self, item: Item) -> Result<Option<Vec<u8>>> {
            match get_generic_password(self.service, account(item)) {
                Ok(kept) => Ok(Some(kept)),
                Err(err) if err.code() == NOT_FOUND => Ok(None),
                Err(err) => Err(refused("give", item, err.code())),
            }
        }

        fn set(&self, item: Item, value: &[u8]) -> Result<()> {
            // Made with the system's default rule: this program reads the
            // item without a question, any other program makes the system
            // ask the person.
            set_generic_password(self.service, account(item), value)
                .map_err(|err| refused("take", item, err.code()))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_two_items_have_names_of_their_own() {
            assert_ne!(account(Item::Key), account(Item::Peers));
            let said = refused("give", Item::Key, -25293).to_string();
            assert!(said.contains("device key") && said.contains("-25293"));
        }

        #[test]
        #[ignore = "adds items to the login keychain of whoever runs it and can raise a dialog: \
                    only with the owner's word, on a machine set aside for it"]
        fn an_item_comes_back_as_it_was_kept() {
            // Under a name of the test's own, never the device's: see
            // docs/adr/0006-pairing.md, "確かめていないこと".
            let vault = LoginKeychain {
                service: "istesso test of the keeping",
            };
            let gone = |item| {
                let _ = security_framework::passwords::delete_generic_password(
                    vault.service,
                    account(item),
                );
            };
            gone(Item::Key);
            assert!(vault.get(Item::Key).unwrap().is_none());
            vault.set(Item::Key, b"first").unwrap();
            assert_eq!(vault.get(Item::Key).unwrap().unwrap(), b"first");
            vault.set(Item::Key, b"second").unwrap();
            assert_eq!(vault.get(Item::Key).unwrap().unwrap(), b"second");
            assert!(vault.get(Item::Peers).unwrap().is_none());
            gone(Item::Key);
            assert!(vault.get(Item::Key).unwrap().is_none());
        }
    }
}

#[cfg(all(feature = "login-keychain", not(target_os = "macos")))]
compile_error!(
    "the `login-keychain` feature keeps the device key in the macOS login keychain; \
     there is no such form on this system (build without it: the key stays in files)"
);

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;
    use crate::{Hello, Peer};

    /// A vault in memory, for the parts that do not care which vault it is.
    #[derive(Debug, Default)]
    struct Memory {
        items: Mutex<HashMap<Item, Vec<u8>>>,
        /// Takes what it is given and keeps nothing: a vault that fails
        /// without saying so.
        forgets: bool,
    }

    impl Vault for Memory {
        fn get(&self, item: Item) -> Result<Option<Vec<u8>>> {
            Ok(self.items.lock().unwrap().get(&item).cloned())
        }

        fn set(&self, item: Item, value: &[u8]) -> Result<()> {
            if !self.forgets {
                self.items.lock().unwrap().insert(item, value.to_vec());
            }
            Ok(())
        }
    }

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            let dir = std::env::temp_dir()
                .join(format!("istesso-pair-keeping-{tag}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Dir(dir)
        }

        fn key(&self) -> PathBuf {
            self.0.join("identity.key")
        }

        fn list(&self) -> PathBuf {
            self.0.join("peers.json")
        }

        fn in_vault(&self, vault: &Arc<Memory>) -> Keeping {
            Keeping::in_vault(self.key(), self.list(), vault.clone())
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn peer(name: &str, seed: u8) -> Peer {
        Peer::from_hello(&Hello {
            device_name: name.to_string(),
            public_key: Identity::from_seed([seed; 32]).public_key(),
        })
    }

    #[test]
    fn the_file_form_is_the_files_it_always_was() {
        let dir = Dir::new("files");
        let keeping = Keeping::in_dir(&dir.0);
        assert_eq!(keeping.form(), Form::Files);
        assert!(keeping.open_to_the_same_user().is_some());
        assert!(keeping.identity_if_any().unwrap().is_none());
        assert!(!dir.key().exists(), "asking for a key does not make one");

        let made = keeping.identity().unwrap();
        let again = keeping.identity_if_any().unwrap().unwrap();
        assert_eq!(made.public_key(), again.public_key());
        // The same file, in the same form, that the key was in before there
        // was a door.
        let text = std::fs::read_to_string(dir.key()).unwrap();
        assert_eq!(
            Identity::from_seed_hex(&text).unwrap().public_key(),
            made.public_key()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.key()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        keeping.peers().unwrap().add(peer("desk", 1)).unwrap();
        let direct = PeerStore::open(dir.list()).unwrap();
        assert_eq!(direct.peers().len(), 1);
        assert_eq!(keeping.peers().unwrap().peers(), direct.peers());
    }

    #[test]
    fn a_damaged_key_file_is_said_and_never_replaced() {
        let dir = Dir::new("damaged");
        std::fs::write(dir.key(), "not hex").unwrap();
        let keeping = Keeping::in_dir(&dir.0);
        assert!(matches!(keeping.identity(), Err(Error::Corrupt(_))));
        assert!(matches!(keeping.identity_if_any(), Err(Error::Corrupt(_))));
        assert_eq!(std::fs::read_to_string(dir.key()).unwrap(), "not hex");
    }

    #[test]
    fn a_vault_keeps_a_new_key_and_no_file_is_written() {
        let dir = Dir::new("vault-new");
        let vault = Arc::new(Memory::default());
        let keeping = dir.in_vault(&vault);
        assert_eq!(keeping.form(), Form::LoginKeychain);
        assert!(keeping.open_to_the_same_user().is_none());
        assert!(keeping.identity_if_any().unwrap().is_none());
        assert!(vault.get(Item::Key).unwrap().is_none());

        let made = keeping.identity().unwrap();
        assert!(!dir.key().exists());
        let again = dir.in_vault(&vault).identity().unwrap();
        assert_eq!(made.public_key(), again.public_key());
        let found = dir.in_vault(&vault).identity_if_any().unwrap().unwrap();
        assert_eq!(made.public_key(), found.public_key());
    }

    #[test]
    fn the_key_of_an_earlier_build_is_moved_in_and_its_file_removed() {
        let dir = Dir::new("vault-move");
        let before = Keeping::in_dir(&dir.0).identity().unwrap();
        assert!(dir.key().exists());

        let vault = Arc::new(Memory::default());
        let moved = dir.in_vault(&vault).identity_if_any().unwrap().unwrap();
        assert_eq!(moved.public_key(), before.public_key(), "pairings stay");
        assert!(!dir.key().exists(), "the file is gone");
        // Once moved, a file put there again is not looked at.
        std::fs::write(dir.key(), Identity::from_seed([9; 32]).seed_hex()).unwrap();
        let still = dir.in_vault(&vault).identity().unwrap();
        assert_eq!(still.public_key(), before.public_key());
    }

    #[test]
    fn a_vault_that_keeps_nothing_costs_no_key() {
        let dir = Dir::new("vault-forgets");
        let before = Keeping::in_dir(&dir.0).identity().unwrap();
        let vault = Arc::new(Memory {
            forgets: true,
            ..Memory::default()
        });
        assert!(matches!(
            dir.in_vault(&vault).identity(),
            Err(Error::Corrupt(_))
        ));
        let after = Keeping::in_dir(&dir.0).identity_if_any().unwrap().unwrap();
        assert_eq!(after.public_key(), before.public_key(), "the file stayed");
    }

    #[test]
    fn a_damaged_kept_key_is_said_and_never_replaced() {
        let dir = Dir::new("vault-damaged");
        let vault = Arc::new(Memory::default());
        vault.set(Item::Key, b"not hex").unwrap();
        assert!(matches!(
            dir.in_vault(&vault).identity(),
            Err(Error::Corrupt(_))
        ));
        assert_eq!(vault.get(Item::Key).unwrap().unwrap(), b"not hex");
    }

    #[test]
    fn the_list_lives_in_the_vault_and_changes_there() {
        let dir = Dir::new("vault-list");
        let vault = Arc::new(Memory::default());
        let keeping = dir.in_vault(&vault);
        assert!(keeping.peers().unwrap().peers().is_empty());

        let mut first = keeping.peers().unwrap();
        let mut second = keeping.peers().unwrap();
        first.add(peer("desk", 1)).unwrap();
        // A handle opened earlier does not undo what another wrote.
        second.add(peer("laptop", 2)).unwrap();
        assert_eq!(second.peers().len(), 2);
        assert!(!dir.list().exists(), "no list in a file");
        assert_eq!(keeping.peers().unwrap().peers().len(), 2);

        let mut store = keeping.peers().unwrap();
        assert!(store.remove("desk").unwrap().is_some());
        assert_eq!(keeping.peers().unwrap().peers().len(), 1);
    }

    #[test]
    fn the_list_of_an_earlier_build_is_moved_in_and_its_file_removed() {
        let dir = Dir::new("vault-list-move");
        let files = Keeping::in_dir(&dir.0);
        files.peers().unwrap().add(peer("desk", 1)).unwrap();
        assert!(dir.list().exists());

        let vault = Arc::new(Memory::default());
        let keeping = dir.in_vault(&vault);
        let moved = keeping.peers().unwrap();
        assert_eq!(moved.peers().len(), 1);
        assert_eq!(moved.peers()[0].name, "desk");
        assert!(!dir.list().exists());

        // A list put back into the file afterwards adds nobody.
        files.peers().unwrap().add(peer("stranger", 7)).unwrap();
        let mut store = keeping.peers().unwrap();
        assert_eq!(store.peers().len(), 1);
        store.add(peer("laptop", 2)).unwrap();
        let names: Vec<_> = store.peers().iter().map(|p| p.name.clone()).collect();
        assert_eq!(names, ["desk", "laptop"]);
    }

    #[test]
    fn a_list_that_cannot_be_read_is_not_moved_in() {
        let dir = Dir::new("vault-list-damaged");
        std::fs::write(dir.list(), "{").unwrap();
        let vault = Arc::new(Memory::default());
        assert!(matches!(
            dir.in_vault(&vault).peers(),
            Err(Error::Corrupt(_))
        ));
        assert!(vault.get(Item::Peers).unwrap().is_none());
        assert!(dir.list().exists());
    }
}
