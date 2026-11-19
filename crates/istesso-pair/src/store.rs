//! The list of devices this machine trusts.
//!
//! A peer is trusted because its public key was pinned during pairing.
//! Removing it from the store is how access is revoked.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::identity::fingerprint;
use crate::keeping::{Item, Vault};
use crate::{to_hex, Error, Hello, Result};

/// What a paired device is allowed to do on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// See windows and listings.
    View,
    /// Send keyboard and pointer input.
    Control,
    /// Install or remove applications.
    InstallApps,
    /// Attach USB devices.
    Usb,
    /// Route audio.
    Audio,
}

impl Permission {
    /// Every permission, in the order they are stored and shown.
    pub const ALL: [Permission; 5] = [
        Permission::View,
        Permission::Control,
        Permission::InstallApps,
        Permission::Usb,
        Permission::Audio,
    ];

    /// Rights a newly paired device starts with. Installing software and
    /// attaching hardware have to be granted separately.
    pub fn defaults() -> Vec<Permission> {
        vec![Permission::View, Permission::Control]
    }

    /// The name used in the peer store and on the command line.
    pub fn name(self) -> &'static str {
        match self {
            Permission::View => "view",
            Permission::Control => "control",
            Permission::InstallApps => "install_apps",
            Permission::Usb => "usb",
            Permission::Audio => "audio",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
    /// Fingerprint of the public key; stable and safe to show.
    pub id: String,
    pub name: String,
    /// Hex-encoded Ed25519 public key.
    pub public_key: String,
    /// Seconds since the Unix epoch.
    pub paired_at: u64,
    pub permissions: Vec<Permission>,
}

impl Peer {
    /// Builds the record for a device that just completed pairing.
    pub fn from_hello(hello: &Hello) -> Self {
        Peer {
            id: fingerprint(&hello.public_key),
            name: hello.device_name.clone(),
            public_key: to_hex(&hello.public_key),
            paired_at: now_unix(),
            permissions: Permission::defaults(),
        }
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

const STORE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct StoreFile {
    version: u32,
    peers: Vec<Peer>,
}

/// Trusted peers, kept in a JSON file readable only by its owner, or, in a
/// build that keeps them elsewhere, as one item of that place
/// ([`crate::Keeping`], which is how a store is opened).
///
/// Several processes use the same list at once (the agent, `istesso pair`
/// waiting for a device, `istesso peers`). A handle's list is what was kept
/// when it was last read. Every change therefore takes the store's lock,
/// reads the list again, applies itself to what is there now and writes the
/// result back before the lock is released, so a handle opened earlier never
/// undoes a change another process made in between.
pub struct PeerStore {
    place: Place,
    peers: Vec<Peer>,
}

/// Where a store keeps its list.
pub(crate) enum Place {
    /// The JSON file at this path.
    File(PathBuf),
    /// One item of `vault`. `file` is where the file form kept the list: a
    /// list found there is moved in, and the store's lock stays next to it.
    Vault {
        vault: std::sync::Arc<dyn Vault>,
        file: PathBuf,
    },
}

impl Place {
    /// The file the store's lock is named after.
    fn file(&self) -> &Path {
        match self {
            Place::File(path) | Place::Vault { file: path, .. } => path,
        }
    }

    /// The list as it is kept now. `locked` says that the caller holds the
    /// store's lock.
    fn read(&self, locked: bool) -> Result<Vec<Peer>> {
        match self {
            Place::File(path) => read_peers(path),
            Place::Vault { vault, file } => {
                if let Some(kept) = vault.get(Item::Peers)? {
                    return parse_peers(&kept, &"the kept list of paired devices");
                }
                // Nothing kept yet. A list an earlier build left in its file
                // is moved in, by one process at a time.
                if !file.exists() {
                    return Ok(Vec::new());
                }
                let _lock = if locked {
                    None
                } else {
                    Some(StoreLock::take(file)?)
                };
                if let Some(kept) = vault.get(Item::Peers)? {
                    return parse_peers(&kept, &"the kept list of paired devices");
                }
                let peers = read_peers(file)?;
                let json = encode_peers(&peers)?;
                vault.set(Item::Peers, &json)?;
                // The file goes only once the list can be read back.
                if vault.get(Item::Peers)?.as_deref() != Some(json.as_slice()) {
                    return Err(Error::Corrupt(
                        "the list of paired devices did not arrive where it is kept; \
                         the file was left in place"
                            .into(),
                    ));
                }
                match std::fs::remove_file(file) {
                    Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.into()),
                    _ => Ok(peers),
                }
            }
        }
    }

    /// Keeps `peers`. Only called with the store's lock held.
    fn write(&self, peers: &[Peer]) -> Result<()> {
        match self {
            Place::File(path) => save(path, peers),
            Place::Vault { vault, .. } => vault.set(Item::Peers, &encode_peers(peers)?),
        }
    }
}

impl PeerStore {
    /// Opens the store in the file at `path`. A missing file is an empty
    /// store. For this crate's own tests: everything else opens a store
    /// through [`crate::Keeping::peers`].
    #[cfg(test)]
    pub(crate) fn open(path: impl Into<PathBuf>) -> Result<Self> {
        Self::open_in(Place::File(path.into()))
    }

    pub(crate) fn open_in(place: Place) -> Result<Self> {
        let peers = place.read(false)?;
        Ok(PeerStore { place, peers })
    }

    /// The peers as they were when the file was last read by this handle.
    pub fn peers(&self) -> &[Peer] {
        &self.peers
    }

    /// Adds a peer, replacing an earlier record of the same key.
    pub fn add(&mut self, peer: Peer) -> Result<()> {
        self.change(|peers| {
            peers.retain(|p| p.id != peer.id);
            peers.push(peer);
            Ok(((), true))
        })
    }

    /// Removes the peer with this id or (unambiguous) name and returns it.
    pub fn remove(&mut self, selector: &str) -> Result<Option<Peer>> {
        self.change(|peers| match position(peers, selector)? {
            Some(index) => Ok((Some(peers.remove(index)), true)),
            None => Ok((None, false)),
        })
    }

    /// One read-modify-write of the list under the store's lock. `apply`
    /// gets the list as it is kept now and says whether it changed it; only
    /// then is the list written. Afterwards this handle shows the list as
    /// it was left.
    fn change<T>(&mut self, apply: impl FnOnce(&mut Vec<Peer>) -> Result<(T, bool)>) -> Result<T> {
        if let Some(parent) = self.place.file().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _lock = StoreLock::take(self.place.file())?;
        let mut peers = self.place.read(true)?;
        let (result, changed) = apply(&mut peers)?;
        if changed {
            self.place.write(&peers)?;
        }
        self.peers = peers;
        Ok(result)
    }

    /// The peer with this id or (unambiguous) name.
    pub fn find(&self, selector: &str) -> Result<Option<&Peer>> {
        Ok(position(&self.peers, selector)?.map(|index| &self.peers[index]))
    }

    /// Grants (`allowed = true`) or withdraws one permission of the peer with
    /// this id or (unambiguous) name. Returns the updated record, or `None`
    /// when no peer matches.
    ///
    /// Nothing is sent to the device: only this machine's record of it
    /// changes. The change is honoured from the device's next connection on;
    /// a service that keeps a connection open for long has to read the store
    /// again while it runs.
    pub fn set_permission(
        &mut self,
        selector: &str,
        permission: Permission,
        allowed: bool,
    ) -> Result<Option<Peer>> {
        self.change(|peers| {
            let Some(index) = position(peers, selector)? else {
                return Ok((None, false));
            };
            let permissions = &mut peers[index].permissions;
            let had = permissions.contains(&permission);
            if allowed && !had {
                permissions.push(permission);
                permissions.sort_by_key(|p| Permission::ALL.iter().position(|known| known == p));
            } else if !allowed && had {
                permissions.retain(|p| *p != permission);
            }
            Ok((Some(peers[index].clone()), allowed != had))
        })
    }

    /// The peer that owns `public_key`, if it is trusted.
    pub fn find_by_key(&self, public_key: &[u8; 32]) -> Option<&Peer> {
        let hex = to_hex(public_key);
        self.peers.iter().find(|p| p.public_key == hex)
    }

    pub fn is_trusted(&self, public_key: &[u8; 32]) -> bool {
        self.find_by_key(public_key).is_some()
    }
}

/// The peer with this id or (unambiguous) name in `peers`.
fn position(peers: &[Peer], selector: &str) -> Result<Option<usize>> {
    if let Some(index) = peers.iter().position(|p| p.id == selector) {
        return Ok(Some(index));
    }
    let matches: Vec<usize> = peers
        .iter()
        .enumerate()
        .filter(|(_, p)| p.name.eq_ignore_ascii_case(selector))
        .map(|(i, _)| i)
        .collect();
    match matches.as_slice() {
        [] => Ok(None),
        [index] => Ok(Some(*index)),
        _ => Err(Error::InvalidName(format!(
            "several devices are named '{selector}'; use the id instead"
        ))),
    }
}

fn read_peers(path: &Path) -> Result<Vec<Peer>> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_peers(
            text.as_bytes(),
            &format_args!("peer store {}", path.display()),
        ),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(err.into()),
    }
}

/// The list in `text`, which is the store as it is kept. `origin` names
/// where it came from, for the error.
fn parse_peers(text: &[u8], origin: &dyn std::fmt::Display) -> Result<Vec<Peer>> {
    let file: StoreFile =
        serde_json::from_slice(text).map_err(|err| Error::Corrupt(format!("{origin}: {err}")))?;
    if file.version != STORE_VERSION {
        return Err(Error::Corrupt(format!(
            "{origin} has unsupported version {}",
            file.version
        )));
    }
    Ok(file.peers)
}

/// The store as it is kept, in a file or elsewhere.
fn encode_peers(peers: &[Peer]) -> Result<Vec<u8>> {
    let file = StoreFile {
        version: STORE_VERSION,
        peers: peers.to_vec(),
    };
    serde_json::to_vec_pretty(&file)
        .map_err(|err| Error::Corrupt(format!("cannot encode peer store: {err}")))
}

/// `path` with `suffix` appended to its file name: `peers.json` becomes
/// `peers.json.lock`.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// Writes the store. Only called with the store's lock held.
fn save(path: &Path, peers: &[Peer]) -> Result<()> {
    let json = encode_peers(peers)?;
    // Write a new sibling file first and move it over the store, so a crash
    // never leaves a half-written store. The name is this process's and this
    // write's own, and the file is created new, so it is private from its
    // first byte whatever an earlier run left behind.
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let tmp = sibling(
        path,
        &format!(
            ".{}.{}.tmp",
            std::process::id(),
            WRITES.fetch_add(1, Ordering::Relaxed)
        ),
    );
    let written = write_new_private(&tmp, &json).and_then(|()| std::fs::rename(&tmp, path));
    if let Err(err) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(err.into());
    }
    // Make the rename itself durable.
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

/// Creates `path`, which must not exist yet, readable only by its owner.
fn write_new_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    // The name is unique to this write; a file under it can only be left
    // over from a process that had the same number and was killed.
    match std::fs::remove_file(path) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err),
        _ => {}
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

/// An exclusive advisory lock on the store, held while the value lives.
///
/// The lock is on a separate file next to the store (`peers.json.lock`),
/// because the store itself is replaced on every write. It is taken with
/// `flock`, which belongs to the open file, so two handles in one process
/// exclude each other as two processes do, and the system releases it when
/// a process dies. The lock file is left in place; removing it would let two
/// writers lock different files.
pub(crate) struct StoreLock {
    _file: std::fs::File,
}

impl StoreLock {
    pub(crate) fn take(store: &Path) -> std::io::Result<Self> {
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(sibling(store, ".lock"))?;
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            loop {
                // SAFETY: `flock` only reads the descriptor, which `file`
                // keeps open for the duration of the call.
                let status = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
                if status == 0 {
                    break;
                }
                let err = std::io::Error::last_os_error();
                if err.kind() != std::io::ErrorKind::Interrupted {
                    return Err(err);
                }
            }
        }
        // Elsewhere there is no lock: the hosts and the command line run on
        // macOS and Linux. Closing the file releases the lock.
        Ok(StoreLock { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("istesso-pair-store-{tag}-{}", std::process::id()))
            .join("peers.json")
    }

    fn hello(name: &str, seed: u8) -> Hello {
        Hello {
            device_name: name.to_string(),
            public_key: crate::Identity::from_seed([seed; 32]).public_key(),
        }
    }

    #[test]
    fn peers_survive_reopening() {
        let path = temp_store("reopen");
        let studio = hello("Mac Studio", 1);
        {
            let mut store = PeerStore::open(&path).unwrap();
            assert!(store.peers().is_empty());
            store.add(Peer::from_hello(&studio)).unwrap();
        }
        let store = PeerStore::open(&path).unwrap();
        assert_eq!(store.peers().len(), 1);
        assert_eq!(store.peers()[0].name, "Mac Studio");
        assert_eq!(store.peers()[0].permissions, Permission::defaults());
        assert!(store.is_trusted(&studio.public_key));
        assert!(!store.is_trusted(&hello("other", 2).public_key));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn pairing_again_replaces_the_old_record() {
        let path = temp_store("replace");
        let mut store = PeerStore::open(&path).unwrap();
        store.add(Peer::from_hello(&hello("old name", 1))).unwrap();
        store.add(Peer::from_hello(&hello("new name", 1))).unwrap();
        assert_eq!(store.peers().len(), 1);
        assert_eq!(store.peers()[0].name, "new name");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn removing_a_peer_revokes_it() {
        let path = temp_store("remove");
        let mut store = PeerStore::open(&path).unwrap();
        let air = hello("MacBook Air", 1);
        let ubuntu = hello("ubuntu", 2);
        store.add(Peer::from_hello(&air)).unwrap();
        store.add(Peer::from_hello(&ubuntu)).unwrap();

        let removed = store.remove("macbook air").unwrap().unwrap();
        assert_eq!(removed.name, "MacBook Air");
        assert!(!store.is_trusted(&air.public_key));

        let id = fingerprint(&ubuntu.public_key);
        assert!(store.remove(&id).unwrap().is_some());
        assert!(store.remove("nobody").unwrap().is_none());
        assert!(PeerStore::open(&path).unwrap().peers().is_empty());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn ambiguous_names_need_the_id() {
        let path = temp_store("ambiguous");
        let mut store = PeerStore::open(&path).unwrap();
        store.add(Peer::from_hello(&hello("mac", 1))).unwrap();
        store.add(Peer::from_hello(&hello("mac", 2))).unwrap();
        assert!(matches!(store.remove("mac"), Err(Error::InvalidName(_))));
        assert!(matches!(store.find("mac"), Err(Error::InvalidName(_))));
        assert_eq!(store.peers().len(), 2);
        // The id always works.
        let id = store.peers()[1].id.clone();
        assert_eq!(store.find(&id).unwrap().unwrap().id, id);
        assert!(store.find("nobody").unwrap().is_none());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn permissions_are_granted_and_withdrawn_one_at_a_time() {
        let path = temp_store("permissions");
        let air = hello("MacBook Air", 1);
        let id = fingerprint(&air.public_key);
        {
            let mut store = PeerStore::open(&path).unwrap();
            store.add(Peer::from_hello(&air)).unwrap();
            store.add(Peer::from_hello(&hello("ubuntu", 2))).unwrap();

            // A new device may look and control, nothing else.
            assert_eq!(
                store.find(&id).unwrap().unwrap().permissions,
                Permission::defaults()
            );

            // Granted by name; the others keep what they had.
            let peer = store
                .set_permission("macbook air", Permission::Audio, true)
                .unwrap()
                .unwrap();
            assert_eq!(
                peer.permissions,
                vec![Permission::View, Permission::Control, Permission::Audio]
            );
            assert_eq!(
                store.find("ubuntu").unwrap().unwrap().permissions,
                Permission::defaults()
            );

            // Granting twice changes nothing, and the order does not depend
            // on the order of the grants.
            store.set_permission(&id, Permission::Audio, true).unwrap();
            let peer = store
                .set_permission(&id, Permission::InstallApps, true)
                .unwrap()
                .unwrap();
            assert_eq!(
                peer.permissions,
                vec![
                    Permission::View,
                    Permission::Control,
                    Permission::InstallApps,
                    Permission::Audio
                ]
            );
        }

        // The grant is on disk, not only in memory.
        let mut store = PeerStore::open(&path).unwrap();
        assert!(store
            .find(&id)
            .unwrap()
            .unwrap()
            .permissions
            .contains(&Permission::Audio));

        // Withdrawn by id; withdrawing what was never granted is not an error.
        let peer = store
            .set_permission(&id, Permission::Audio, false)
            .unwrap()
            .unwrap();
        assert!(!peer.permissions.contains(&Permission::Audio));
        let peer = store
            .set_permission(&id, Permission::Usb, false)
            .unwrap()
            .unwrap();
        assert_eq!(
            peer.permissions,
            vec![
                Permission::View,
                Permission::Control,
                Permission::InstallApps
            ]
        );
        let store = PeerStore::open(&path).unwrap();
        assert!(!store
            .find(&id)
            .unwrap()
            .unwrap()
            .permissions
            .contains(&Permission::Audio));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn permissions_of_unknown_or_ambiguous_devices_are_not_touched() {
        let path = temp_store("permissions-selector");
        let mut store = PeerStore::open(&path).unwrap();
        store.add(Peer::from_hello(&hello("mac", 1))).unwrap();
        store.add(Peer::from_hello(&hello("mac", 2))).unwrap();
        assert!(store
            .set_permission("nobody", Permission::Audio, true)
            .unwrap()
            .is_none());
        assert!(matches!(
            store.set_permission("mac", Permission::Audio, true),
            Err(Error::InvalidName(_))
        ));
        assert!(store
            .peers()
            .iter()
            .all(|peer| peer.permissions == Permission::defaults()));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    fn permissions_on_disk(path: &Path, selector: &str) -> Option<Vec<Permission>> {
        PeerStore::open(path)
            .unwrap()
            .find(selector)
            .unwrap()
            .map(|peer| peer.permissions.clone())
    }

    #[test]
    fn a_stale_handle_does_not_undo_another_change() {
        let path = temp_store("stale");
        let air = hello("MacBook Air", 1);
        let ubuntu = hello("ubuntu", 2);
        {
            let mut store = PeerStore::open(&path).unwrap();
            store.add(Peer::from_hello(&air)).unwrap();
            store.add(Peer::from_hello(&ubuntu)).unwrap();
        }

        // Two handles opened at the same time, as two commands would.
        let mut stale = PeerStore::open(&path).unwrap();
        let mut other = PeerStore::open(&path).unwrap();
        other
            .set_permission("macbook air", Permission::Audio, true)
            .unwrap();

        // Each kind of change made through the stale handle keeps the other's.
        stale.add(Peer::from_hello(&hello("studio", 3))).unwrap();
        assert_eq!(
            permissions_on_disk(&path, "macbook air").unwrap(),
            vec![Permission::View, Permission::Control, Permission::Audio]
        );
        assert!(stale
            .find("macbook air")
            .unwrap()
            .unwrap()
            .permissions
            .contains(&Permission::Audio));

        other.remove("studio").unwrap().unwrap();
        stale
            .set_permission("ubuntu", Permission::Control, false)
            .unwrap()
            .unwrap();
        assert!(permissions_on_disk(&path, "studio").is_none());
        assert_eq!(
            permissions_on_disk(&path, "macbook air").unwrap(),
            vec![Permission::View, Permission::Control, Permission::Audio]
        );

        other
            .set_permission("ubuntu", Permission::Usb, true)
            .unwrap()
            .unwrap();
        stale.remove("macbook air").unwrap().unwrap();
        assert_eq!(
            permissions_on_disk(&path, "ubuntu").unwrap(),
            vec![Permission::View, Permission::Usb]
        );
        assert!(permissions_on_disk(&path, "macbook air").is_none());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_device_paired_while_rights_change_keeps_those_changes() {
        // `istesso pair host` opens the store, then waits minutes for the
        // other device. Meanwhile the person withdraws rights and removes a
        // device with `istesso peers`.
        let path = temp_store("pairing");
        let air = hello("MacBook Air", 1);
        let old = hello("old phone", 2);
        {
            let mut store = PeerStore::open(&path).unwrap();
            store.add(Peer::from_hello(&air)).unwrap();
            store.add(Peer::from_hello(&old)).unwrap();
            store
                .set_permission("macbook air", Permission::Audio, true)
                .unwrap();
        }
        let mut waiting = PeerStore::open(&path).unwrap();

        let mut peers = PeerStore::open(&path).unwrap();
        peers
            .set_permission("macbook air", Permission::View, false)
            .unwrap();
        let mut peers = PeerStore::open(&path).unwrap();
        peers
            .set_permission("macbook air", Permission::Audio, false)
            .unwrap();
        let mut peers = PeerStore::open(&path).unwrap();
        peers.remove("old phone").unwrap().unwrap();

        waiting.add(Peer::from_hello(&hello("ubuntu", 3))).unwrap();

        let store = PeerStore::open(&path).unwrap();
        let names: Vec<&str> = store.peers().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["MacBook Air", "ubuntu"]);
        assert_eq!(
            permissions_on_disk(&path, "macbook air").unwrap(),
            vec![Permission::Control]
        );
        assert!(!store.is_trusted(&old.public_key));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn writers_at_the_same_time_all_keep_their_change() {
        let path = temp_store("concurrent");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let writers: Vec<_> = (0..16u8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let mut store = PeerStore::open(&path).unwrap();
                    let name = format!("device {i}");
                    store.add(Peer::from_hello(&hello(&name, i + 1))).unwrap();
                    store
                        .set_permission(&name, Permission::Audio, true)
                        .unwrap();
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        let store = PeerStore::open(&path).unwrap();
        assert_eq!(store.peers().len(), 16);
        assert!(store
            .peers()
            .iter()
            .all(|peer| peer.permissions.contains(&Permission::Audio)));
        // Only the store and its lock are left; no temporary files.
        let mut left: Vec<String> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["peers.json", "peers.json.lock"]);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_store_is_private_even_over_a_readable_file() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_store("mode");
        let dir = path.parent().unwrap();
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(&path, r#"{"version": 1, "peers": []}"#).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        // A temporary file left behind by an older version, readable by all.
        let leftover = path.with_extension("tmp");
        std::fs::write(&leftover, "{}").unwrap();
        std::fs::set_permissions(&leftover, std::fs::Permissions::from_mode(0o644)).unwrap();

        let mut store = PeerStore::open(&path).unwrap();
        store.add(Peer::from_hello(&hello("mac", 1))).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&path.with_file_name("peers.json.lock")), 0o600);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn permission_names_match_the_stored_form() {
        for permission in Permission::ALL {
            let json = serde_json::to_string(&permission).unwrap();
            assert_eq!(json, format!("\"{}\"", permission.name()));
        }
    }

    #[test]
    fn damaged_store_is_reported() {
        let path = temp_store("damaged");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        assert!(matches!(PeerStore::open(&path), Err(Error::Corrupt(_))));
        std::fs::write(&path, r#"{"version": 99, "peers": []}"#).unwrap();
        assert!(matches!(PeerStore::open(&path), Err(Error::Corrupt(_))));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
