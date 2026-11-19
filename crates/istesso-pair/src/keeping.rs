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
//!   program makes the system ask the person). **Nothing is ever taken in
//!   from a file**: a key file or a list file is something any program of
//!   the same user can put there, at the first start or after the items
//!   were removed, and taking it in would make that program's key this
//!   device's key and its list the devices this one trusts. A file at the
//!   place of the file form stops this form ([`Error::LeftInAFile`]) until
//!   the person has removed it; a device that changes form gets a new key
//!   and is paired again. What this form still does not guard is said by
//!   [`Keeping::open_to_the_same_user`].
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
    /// them; a build that keeps them elsewhere never reads what lies there
    /// and refuses to go on while something does.
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
    /// person. No form there is today guards everything, so every form has
    /// one.
    pub fn open_to_the_same_user(&self) -> Option<&'static str> {
        match self.form() {
            Form::Files => Some(
                "this build keeps the device key and the list of paired devices in files \
                 that every program of the same user can read and change",
            ),
            // Reading is asked about by the system. Making the items first,
            // changing them and removing them is not known to be: the
            // program does not look at who made an item.
            Form::LoginKeychain => Some(
                "this build keeps the device key and the list of paired devices in the \
                 login keychain: another program of the same user is asked about before \
                 it reads them, but may still be able to put its own there first, change \
                 them or remove them",
            ),
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

    /// The key kept in `vault`. When nothing is kept there, one is made if
    /// `make` says so. A key in a file is never taken in, whoever put it
    /// there: its being there is an error.
    fn identity_in(&self, vault: &dyn Vault, make: bool) -> Result<Option<Identity>> {
        let kept = |bytes: Vec<u8>| {
            std::str::from_utf8(&bytes)
                .ok()
                .and_then(Identity::from_seed_hex)
                .ok_or_else(|| Error::Corrupt("the kept device key is damaged".into()))
        };
        not_left_in_a_file(&self.identity, "a device key")?;
        if let Some(bytes) = vault.get(Item::Key)? {
            return kept(bytes).map(Some);
        }
        if !make {
            return Ok(None);
        }
        // Made by one process at a time; another may have been first.
        if let Some(parent) = self.identity.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _lock = StoreLock::take(&self.identity)?;
        if let Some(bytes) = vault.get(Item::Key)? {
            return kept(bytes).map(Some);
        }
        let identity = Identity::generate();
        let seed = identity.seed_hex();
        vault.set(Item::Key, seed.as_bytes())?;
        // A key that cannot be read back would be gone with this process,
        // and every pairing made with it.
        if vault.get(Item::Key)?.as_deref() != Some(seed.as_bytes()) {
            return Err(Error::Corrupt(
                "the device key did not arrive where it is kept".into(),
            ));
        }
        Ok(Some(identity))
    }
}

/// Refuses to go on while `file` is there, in a form that does not keep
/// `what` in files. Anything at that name counts, also a link or a
/// directory: nothing there is ever opened.
pub(crate) fn not_left_in_a_file(file: &Path, what: &str) -> Result<()> {
    match std::fs::symlink_metadata(file) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
        // The name only: the line goes into logs, and the folder says whose
        // account this is.
        Ok(_) => Err(Error::LeftInAFile(format!(
            "{what} lies in a file ({}) in the place where earlier builds kept it; this \
             build does not keep it there. It is not read and not used: a file can be put \
             there by any program of the same user. Remove the file to go on; a device \
             that had its key there gets a new one and is paired again",
            file.file_name()
                .map_or_else(|| "?".into(), |name| name.to_string_lossy())
        ))),
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

    // Not closed here (docs/adr/0006-pairing.md, "この形で守れないこと"): the
    // items are looked for in the keychains of the account's search list, in
    // its order, not in the login keychain alone. A program of the same user
    // can put a keychain of its own ahead in that list. Naming the login
    // keychain needs another, older family of calls; that change is made
    // together with the first run on a keychain the owner allows, not
    // before it could be run.
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
        // This form has limits of its own, and says them.
        let said = keeping.open_to_the_same_user().unwrap();
        assert!(said.contains("put its own there first"), "{said}");
        assert!(keeping.identity_if_any().unwrap().is_none());
        assert!(vault.get(Item::Key).unwrap().is_none());

        let made = keeping.identity().unwrap();
        assert!(!dir.key().exists());
        let again = dir.in_vault(&vault).identity().unwrap();
        assert_eq!(made.public_key(), again.public_key());
        let found = dir.in_vault(&vault).identity_if_any().unwrap().unwrap();
        assert_eq!(made.public_key(), found.public_key());
    }

    /// What the automatic move of an earlier draft did: whoever could put a
    /// key file there (any program of the same user) chose this device's
    /// key. Now nothing is taken in, at the first start or later.
    #[test]
    fn a_key_in_a_file_is_never_taken_in_and_stops_the_form() {
        let dir = Dir::new("vault-file-key");
        let planted = Keeping::in_dir(&dir.0).identity().unwrap();
        assert!(dir.key().exists());

        // First start of this form, nothing kept yet.
        let vault = Arc::new(Memory::default());
        for asked in [
            dir.in_vault(&vault).identity().map(Some),
            dir.in_vault(&vault).identity_if_any(),
        ] {
            let Err(Error::LeftInAFile(said)) = asked else {
                panic!("a key in a file was not refused");
            };
            assert!(said.contains("(identity.key)"), "{said}");
            // The folder is not said: the line goes into logs.
            assert!(!said.contains(dir.0.to_str().unwrap()), "{said}");
        }
        assert!(vault.get(Item::Key).unwrap().is_none(), "nothing moved in");
        assert!(dir.key().exists(), "and nothing removed");

        // The person removes the file: the device gets a key of its own.
        std::fs::remove_file(dir.key()).unwrap();
        let own = dir.in_vault(&vault).identity().unwrap();
        assert_ne!(own.public_key(), planted.public_key());
        assert!(!dir.key().exists());

        // A file that appears later is refused as well, and the kept key
        // stays what it was.
        std::fs::write(dir.key(), Identity::from_seed([9; 32]).seed_hex()).unwrap();
        assert!(matches!(
            dir.in_vault(&vault).identity(),
            Err(Error::LeftInAFile(_))
        ));
        std::fs::remove_file(dir.key()).unwrap();
        let still = dir.in_vault(&vault).identity().unwrap();
        assert_eq!(still.public_key(), own.public_key());
    }

    /// The kept key was removed (by whatever could) and a key file put in
    /// its place: the file's key does not become the device's.
    #[test]
    fn a_key_file_planted_after_the_kept_key_was_removed_is_not_taken_in() {
        let dir = Dir::new("vault-removed-key");
        let vault = Arc::new(Memory::default());
        let own = dir.in_vault(&vault).identity().unwrap();
        vault.items.lock().unwrap().remove(&Item::Key);
        let planted = Identity::from_seed([9; 32]);
        std::fs::write(dir.key(), planted.seed_hex()).unwrap();

        assert!(matches!(
            dir.in_vault(&vault).identity(),
            Err(Error::LeftInAFile(_))
        ));
        assert!(matches!(
            dir.in_vault(&vault).identity_if_any(),
            Err(Error::LeftInAFile(_))
        ));
        assert!(vault.get(Item::Key).unwrap().is_none());
        // Without the file the device starts anew: paired with nobody, and
        // with neither the removed key nor the planted one.
        std::fs::remove_file(dir.key()).unwrap();
        let anew = dir.in_vault(&vault).identity().unwrap();
        assert_ne!(anew.public_key(), planted.public_key());
        assert_ne!(anew.public_key(), own.public_key());
    }

    /// A link at the name is refused like a file, and never followed.
    #[cfg(unix)]
    #[test]
    fn a_link_where_the_key_file_would_be_is_refused_too() {
        let dir = Dir::new("vault-link-key");
        std::os::unix::fs::symlink(dir.0.join("nowhere"), dir.key()).unwrap();
        let vault = Arc::new(Memory::default());
        assert!(matches!(
            dir.in_vault(&vault).identity(),
            Err(Error::LeftInAFile(_))
        ));
    }

    #[test]
    fn a_vault_that_keeps_nothing_is_said_and_no_key_is_used() {
        let dir = Dir::new("vault-forgets");
        let vault = Arc::new(Memory {
            forgets: true,
            ..Memory::default()
        });
        assert!(matches!(
            dir.in_vault(&vault).identity(),
            Err(Error::Corrupt(_))
        ));
        assert!(!dir.key().exists(), "and no file is written instead");
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

    /// What the automatic move of an earlier draft did: whoever could put a
    /// list there (any program of the same user) decided which devices this
    /// one trusts. Now nothing is taken in, at the first start or later.
    #[test]
    fn a_list_in_a_file_is_never_taken_in_and_stops_the_form() {
        let dir = Dir::new("vault-file-list");
        let files = Keeping::in_dir(&dir.0);
        files.peers().unwrap().add(peer("stranger", 7)).unwrap();
        assert!(dir.list().exists());

        // First start of this form, nothing kept yet.
        let vault = Arc::new(Memory::default());
        let keeping = dir.in_vault(&vault);
        let Err(Error::LeftInAFile(said)) = keeping.peers() else {
            panic!("a list in a file was not refused");
        };
        assert!(said.contains("peers.json"), "{said}");
        assert!(
            vault.get(Item::Peers).unwrap().is_none(),
            "nothing moved in"
        );
        assert!(dir.list().exists(), "and nothing removed");

        // The person removes the file: the list is empty, and the device is
        // paired anew.
        std::fs::remove_file(dir.list()).unwrap();
        let mut store = keeping.peers().unwrap();
        assert!(store.peers().is_empty());
        store.add(peer("desk", 1)).unwrap();

        // A list that appears in the file later adds nobody: reading is
        // refused, and so is every change through a handle opened before.
        files.peers().unwrap().add(peer("stranger", 7)).unwrap();
        assert!(matches!(keeping.peers(), Err(Error::LeftInAFile(_))));
        assert!(matches!(
            store.add(peer("laptop", 2)),
            Err(Error::LeftInAFile(_))
        ));
        std::fs::remove_file(dir.list()).unwrap();
        let names: Vec<_> = keeping
            .peers()
            .unwrap()
            .peers()
            .iter()
            .map(|p| p.name.clone())
            .collect();
        assert_eq!(names, ["desk"]);
    }

    /// The kept list was removed (by whatever could) and a list put into
    /// the file: the devices in it are not trusted.
    #[test]
    fn a_list_planted_after_the_kept_list_was_removed_is_not_taken_in() {
        let dir = Dir::new("vault-removed-list");
        let vault = Arc::new(Memory::default());
        let keeping = dir.in_vault(&vault);
        keeping.peers().unwrap().add(peer("desk", 1)).unwrap();
        vault.items.lock().unwrap().remove(&Item::Peers);
        Keeping::in_dir(&dir.0)
            .peers()
            .unwrap()
            .add(peer("stranger", 7))
            .unwrap();

        assert!(matches!(keeping.peers(), Err(Error::LeftInAFile(_))));
        assert!(vault.get(Item::Peers).unwrap().is_none());
        // Without the file: nobody is trusted, rather than somebody's list.
        std::fs::remove_file(dir.list()).unwrap();
        assert!(keeping.peers().unwrap().peers().is_empty());
    }

    #[test]
    fn a_list_that_cannot_be_read_is_not_looked_at_either() {
        let dir = Dir::new("vault-list-damaged");
        std::fs::write(dir.list(), "{").unwrap();
        let vault = Arc::new(Memory::default());
        assert!(matches!(
            dir.in_vault(&vault).peers(),
            Err(Error::LeftInAFile(_))
        ));
        assert!(vault.get(Item::Peers).unwrap().is_none());
        assert!(dir.list().exists());
    }
}
