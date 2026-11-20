//! A folder of this machine shown read-only on another, over the control
//! channel instead of `ssh` (proposed in ADR 0007, "フォルダーを読むだけで見せる").
//!
//! * [`export`] answers questions about one folder and never leaves it: every
//!   entry is opened one name at a time from the folder's own descriptor,
//!   without following links. It began as the research prototype's
//!   `export.rs` (branch `research-unified-mount`).
//! * [`rules`] decides which entries are not shown: the files Git ignores,
//!   and `.git`, unless the owner says so for that folder.
//! * [`host`] is what the agent does once a device may see a folder.
//! * [`client`] is the device's end: one question at a time, and "still
//!   here" in between. The loopback NFS server that the operating system
//!   mounts is outside the product (`research/unified-mount-proto`).
//!
//! Off by default: the host has to name the folders (by an id; the device
//! never sends a path) and the devices ([`MountPolicy`]), and the devices
//! must still be paired and allowed to control the host, looked up again
//! every second.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use istesso_link::mount::valid_folder_id;
use istesso_pair::{Peer, PeerStore};

pub mod client;
pub mod export;
pub mod host;
pub mod rules;

/// How often the device says "still here".
pub const ALIVE_EVERY: Duration = Duration::from_secs(10);
/// Folders shown at the same time to one device, and on this machine.
pub const MAX_PER_DEVICE: usize = 8;
pub const MAX_ON_HOST: usize = 16;
/// The most ids one conversation hands out.
pub const MAX_IDS: usize = 100_000;

/// One folder the owner named.
#[derive(Debug, Clone)]
pub struct Folder {
    pub id: String,
    pub path: PathBuf,
    /// Show what Git ignores, and `.git`, too.
    pub show_ignored: bool,
}

/// Which folders this machine shows, and to which devices. Empty, the
/// default: none, to nobody.
#[derive(Debug, Clone, Default)]
pub struct MountPolicy {
    folders: Vec<Folder>,
    /// Each device by its id, key and name, as they were when the agent
    /// started.
    devices: Vec<(String, String, String)>,
}

impl MountPolicy {
    /// Reads `ID=FOLDER` pairs, devices by their id only (as `istesso peers`
    /// shows it; a name can be given to another device by whoever pairs
    /// it), and the ids of folders that show what Git ignores. A folder
    /// that is not there, a device that is not paired or an id given twice
    /// is an error: the person meant something else and should hear it.
    pub fn resolve(
        peers: &PeerStore,
        folders: &[String],
        devices: &[String],
        show_ignored: &[String],
    ) -> Result<Self, String> {
        let show: HashSet<&str> = show_ignored.iter().map(String::as_str).collect();
        let mut named = Vec::new();
        for spec in folders {
            let (id, path) = spec
                .split_once('=')
                .ok_or_else(|| format!("--mount-folder {spec}: expected ID=FOLDER"))?;
            if !valid_folder_id(id) {
                return Err(format!(
                    "--mount-folder {id}: an id is 1 to 32 of a-z, 0-9 and -, not starting with -"
                ));
            }
            let path = PathBuf::from(path);
            if !path.is_absolute() || !path.is_dir() {
                return Err(format!(
                    "--mount-folder {id}: not an absolute path to a folder"
                ));
            }
            if named.iter().any(|folder: &Folder| folder.id == id) {
                return Err(format!("--mount-folder {id}: given twice"));
            }
            named.push(Folder {
                id: id.to_string(),
                path,
                show_ignored: show.contains(id),
            });
        }
        for id in &show {
            if !named.iter().any(|folder| folder.id == *id) {
                return Err(format!("--mount-show-ignored {id}: no folder has that id"));
            }
        }
        let mut allowed = Vec::new();
        for id in devices {
            let Some(peer) = peers.peers().iter().find(|peer| peer.id == *id) else {
                let by_name = peers.find(id).ok().flatten();
                return Err(match by_name {
                    Some(peer) => format!(
                        "--allow-mount takes the id of a paired device, not its name: {} has the id {}",
                        peer.name, peer.id
                    ),
                    None => format!("--allow-mount: no paired device has the id '{id}'"),
                });
            };
            allowed.push((peer.id.clone(), peer.public_key.clone(), peer.name.clone()));
        }
        Ok(MountPolicy {
            folders: named,
            devices: allowed,
        })
    }

    /// Whether any device may see any folder.
    pub fn is_on(&self) -> bool {
        !self.folders.is_empty() && !self.devices.is_empty()
    }

    /// Whether this device was named, with the key it had then.
    pub fn names(&self, peer: &Peer) -> bool {
        self.devices
            .iter()
            .any(|(id, key, _)| *id == peer.id && *key == peer.public_key)
    }

    /// The devices that may see the folders, as "name (id)", for the person
    /// who starts the agent to check.
    pub fn devices(&self) -> Vec<String> {
        self.devices
            .iter()
            .map(|(id, _, name)| format!("{name} ({id})"))
            .collect()
    }

    /// The folder with this id, when the owner named one.
    pub fn folder(&self, id: &str) -> Option<&Folder> {
        self.folders.iter().find(|folder| folder.id == id)
    }

    pub fn folder_count(&self) -> usize {
        self.folders.len()
    }

    pub fn device_count(&self) -> usize {
        self.devices.len()
    }
}

/// Where the user's global Git ignore file is, as Git would look for it
/// (`core.excludesFile`, or `~/.config/git/ignore`). It may not be there
/// (no rules) or not be readable (every entry hidden): [`rules::Rules::new`]
/// tells.
pub fn global_ignore_file() -> Option<PathBuf> {
    ignore::gitignore::gitconfig_excludes_path()
}

/// Whether `path` is a folder only its owner can enter or read: what the
/// parent of a mount point has to be.
pub fn private_folder(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_dir() {
        return Err("is not a folder (or is a link)".into());
    }
    // SAFETY: getuid cannot fail.
    if meta.uid() != unsafe { libc::getuid() } {
        return Err("belongs to another user".into());
    }
    if meta.mode() & 0o077 != 0 {
        return Err(format!(
            "can be entered by others (mode {:o}; it must be 700)",
            meta.mode() & 0o777
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
