//! The list of devices this machine trusts.
//!
//! A peer is trusted because its public key was pinned during pairing.
//! Removing it from the store is how access is revoked.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::identity::{fingerprint, write_private};
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
    /// Rights a newly paired device starts with. Installing software and
    /// attaching hardware have to be granted separately.
    pub fn defaults() -> Vec<Permission> {
        vec![Permission::View, Permission::Control]
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

/// Trusted peers, kept in a JSON file readable only by its owner.
pub struct PeerStore {
    path: PathBuf,
    peers: Vec<Peer>,
}

impl PeerStore {
    /// Opens the store at `path`. A missing file is an empty store.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let peers = match std::fs::read_to_string(&path) {
            Ok(text) => {
                let file: StoreFile = serde_json::from_str(&text).map_err(|err| {
                    Error::Corrupt(format!("peer store {}: {err}", path.display()))
                })?;
                if file.version != STORE_VERSION {
                    return Err(Error::Corrupt(format!(
                        "peer store {} has unsupported version {}",
                        path.display(),
                        file.version
                    )));
                }
                file.peers
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(err) => return Err(err.into()),
        };
        Ok(PeerStore { path, peers })
    }

    pub fn peers(&self) -> &[Peer] {
        &self.peers
    }

    /// Adds a peer, replacing an earlier record of the same key.
    pub fn add(&mut self, peer: Peer) -> Result<()> {
        self.peers.retain(|p| p.id != peer.id);
        self.peers.push(peer);
        self.save()
    }

    /// Removes the peer with this id or (unambiguous) name and returns it.
    pub fn remove(&mut self, selector: &str) -> Result<Option<Peer>> {
        let by_id = self.peers.iter().position(|p| p.id == selector);
        let index = match by_id {
            Some(index) => index,
            None => {
                let matches: Vec<usize> = self
                    .peers
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.name.eq_ignore_ascii_case(selector))
                    .map(|(i, _)| i)
                    .collect();
                match matches.as_slice() {
                    [] => return Ok(None),
                    [index] => *index,
                    _ => {
                        return Err(Error::InvalidName(format!(
                            "several devices are named '{selector}'; use the id instead"
                        )))
                    }
                }
            }
        };
        let removed = self.peers.remove(index);
        self.save()?;
        Ok(Some(removed))
    }

    /// The peer that owns `public_key`, if it is trusted.
    pub fn find_by_key(&self, public_key: &[u8; 32]) -> Option<&Peer> {
        let hex = to_hex(public_key);
        self.peers.iter().find(|p| p.public_key == hex)
    }

    pub fn is_trusted(&self, public_key: &[u8; 32]) -> bool {
        self.find_by_key(public_key).is_some()
    }

    fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = StoreFile {
            version: STORE_VERSION,
            peers: self.peers.clone(),
        };
        let json = serde_json::to_vec_pretty(&file)
            .map_err(|err| Error::Corrupt(format!("cannot encode peer store: {err}")))?;
        // Write to a sibling file first so a crash never leaves a half-written store.
        let tmp = self.path.with_extension("tmp");
        write_private(&tmp, &json)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("ud-pair-store-{tag}-{}", std::process::id()))
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
        assert_eq!(store.peers().len(), 2);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
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
