//! What this device remembers about the others, and about itself.
//!
//! One file per paired device, in the cache directory. Nothing here is
//! trusted or needed for security: a snapshot only says where a device was
//! last reached and what it reported then, so that it can still be shown
//! (with a warning) while it is asleep. Whether a device is trusted is
//! decided by the peer store alone, and whoever answers at a remembered
//! address still has to pass the handshake.

use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use ud_proto::{AppEntry, HostManifest, MANIFEST_SCHEMA};

use crate::paths::Paths;

/// How many addresses are kept per device.
pub const MAX_ADDRESSES: usize = 8;

/// The last known state of a paired device.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HostSnapshot {
    pub id: String,
    pub name: String,
    /// `host:port` strings to try, the last one that worked first.
    #[serde(default)]
    pub addresses: Vec<String>,
    /// When the device last answered, in seconds since the Unix epoch.
    #[serde(default)]
    pub seen_at: Option<u64>,
    #[serde(default)]
    pub manifest: Option<HostManifest>,
    #[serde(default, deserialize_with = "ud_proto::known_only")]
    pub apps: Vec<AppEntry>,
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Reads the snapshot of device `id`. Missing or unreadable files count as
/// "nothing remembered".
pub fn load(paths: &Paths, id: &str) -> Option<HostSnapshot> {
    let path = paths.host_snapshot(id)?;
    let text = std::fs::read_to_string(path).ok()?;
    let snapshot: HostSnapshot = serde_json::from_str(&text).ok()?;
    (snapshot.id == id).then_some(snapshot)
}

pub fn store(paths: &Paths, snapshot: &HostSnapshot) -> io::Result<()> {
    let path = paths
        .host_snapshot(&snapshot.id)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a device id"))?;
    write_private(&path, &serde_json::to_vec_pretty(snapshot)?)
}

/// Forgets everything about device `id` (used when its pairing is removed).
pub fn forget(paths: &Paths, id: &str) {
    if let Some(path) = paths.host_snapshot(id) {
        let _ = std::fs::remove_file(path);
    }
}

/// Records an address at which device `id` might be reachable. A hint goes
/// behind the address that last worked; it becomes the first choice once a
/// connection to it succeeds.
pub fn suggest_address(paths: &Paths, id: &str, name: &str, address: &str) -> io::Result<()> {
    let mut snapshot = load(paths, id).unwrap_or_else(|| HostSnapshot {
        id: id.to_string(),
        name: name.to_string(),
        ..HostSnapshot::default()
    });
    if address.is_empty() || snapshot.addresses.iter().any(|a| a == address) {
        return Ok(());
    }
    let position = snapshot.addresses.len().min(1);
    snapshot.addresses.insert(position, address.to_string());
    snapshot.addresses.truncate(MAX_ADDRESSES);
    store(paths, &snapshot)
}

/// Moves `preferred` to the front of `addresses` (in the given order), drops
/// duplicates and keeps the list short.
pub fn promote(addresses: &mut Vec<String>, preferred: impl IntoIterator<Item = String>) {
    let mut merged: Vec<String> = Vec::new();
    for address in preferred.into_iter().chain(std::mem::take(addresses)) {
        if !address.is_empty() && !merged.contains(&address) {
            merged.push(address);
        }
    }
    merged.truncate(MAX_ADDRESSES);
    *addresses = merged;
}

#[derive(Serialize, Deserialize)]
struct LocalManifestFile {
    saved_at: u64,
    manifest: HostManifest,
}

/// This machine's manifest.
///
/// A full inspection starts a dozen programs and takes a second or two on
/// macOS, so its result is reused for up to `max_age`. The network adapters
/// are cheap to read and change when the machine moves, so they are always
/// read afresh.
pub fn local_manifest(paths: &Paths, max_age: Duration) -> HostManifest {
    let cached = std::fs::read_to_string(paths.local_manifest())
        .ok()
        .and_then(|text| serde_json::from_str::<LocalManifestFile>(&text).ok())
        .filter(|file| {
            file.manifest.schema == MANIFEST_SCHEMA
                && now_unix().saturating_sub(file.saved_at) <= max_age.as_secs()
        });
    match cached {
        Some(file) => {
            let mut manifest = file.manifest;
            manifest.power.interfaces = ud_doctor::interfaces();
            manifest
        }
        None => refresh_local_manifest(paths),
    }
}

/// Inspects this machine now and remembers the result.
pub fn refresh_local_manifest(paths: &Paths) -> HostManifest {
    let manifest = ud_doctor::detect();
    remember_local_manifest(paths, &manifest);
    manifest
}

/// Remembers a manifest that was just produced by a full inspection.
pub fn remember_local_manifest(paths: &Paths, manifest: &HostManifest) {
    let file = LocalManifestFile {
        saved_at: now_unix(),
        manifest: manifest.clone(),
    };
    if let Ok(json) = serde_json::to_vec(&file) {
        // A cache that cannot be written only costs time on the next call.
        let _ = write_private(&paths.local_manifest(), &json);
    }
}

/// Writes a file only its owner can read, replacing any earlier version in
/// one step.
fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(parent)?;
    }
    // The agent and the command line tool may write at the same time; each
    // process uses its own temporary file.
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options
        .open(&tmp)
        .and_then(|mut file| file.write_all(contents))
        .and_then(|_| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    const ID: &str = "1a2b-3c4d-5e6f-7a8b";

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn snapshots_survive_a_round_trip() {
        let dir = TempDir::new("cache-roundtrip");
        let paths = dir.paths();
        assert_eq!(load(&paths, ID), None);

        let snapshot = HostSnapshot {
            id: ID.to_string(),
            name: "studio".to_string(),
            addresses: list(&["192.168.1.30:47801"]),
            seen_at: Some(1_800_000_000),
            manifest: None,
            apps: Vec::new(),
        };
        store(&paths, &snapshot).unwrap();
        assert_eq!(load(&paths, ID), Some(snapshot));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file = paths.host_snapshot(ID).unwrap();
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        forget(&paths, ID);
        assert_eq!(load(&paths, ID), None);
    }

    #[test]
    fn a_file_for_another_device_or_a_damaged_file_is_ignored() {
        let dir = TempDir::new("cache-damaged");
        let paths = dir.paths();
        let file = paths.host_snapshot(ID).unwrap();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();

        std::fs::write(&file, "{ not json").unwrap();
        assert_eq!(load(&paths, ID), None);

        std::fs::write(&file, r#"{"id":"ffff-ffff-ffff-ffff","name":"other"}"#).unwrap();
        assert_eq!(load(&paths, ID), None);

        // Fields added by later versions are fine, missing ones get defaults.
        std::fs::write(
            &file,
            format!(r#"{{"id":"{ID}","name":"studio","colour":"blue"}}"#),
        )
        .unwrap();
        let snapshot = load(&paths, ID).unwrap();
        assert_eq!(snapshot.name, "studio");
        assert!(snapshot.addresses.is_empty());
        assert!(store(
            &paths,
            &HostSnapshot {
                id: "../escape".to_string(),
                ..HostSnapshot::default()
            }
        )
        .is_err());
    }

    #[test]
    fn working_addresses_come_first_and_the_list_stays_short() {
        let mut addresses = list(&["a:1", "b:1", "c:1"]);
        promote(&mut addresses, list(&["c:1", "d:1", ""]));
        assert_eq!(addresses, list(&["c:1", "d:1", "a:1", "b:1"]));

        let mut many: Vec<String> = (0..20).map(|i| format!("h{i}:1")).collect();
        promote(&mut many, list(&["new:1"]));
        assert_eq!(many.len(), MAX_ADDRESSES);
        assert_eq!(many[0], "new:1");
    }

    #[test]
    fn hints_do_not_displace_the_address_that_worked() {
        let dir = TempDir::new("cache-hints");
        let paths = dir.paths();
        suggest_address(&paths, ID, "studio", "192.168.1.30:47801").unwrap();
        suggest_address(&paths, ID, "studio", "192.168.1.31:47801").unwrap();
        suggest_address(&paths, ID, "studio", "192.168.1.32:47801").unwrap();
        suggest_address(&paths, ID, "studio", "192.168.1.31:47801").unwrap();
        let snapshot = load(&paths, ID).unwrap();
        assert_eq!(snapshot.name, "studio");
        assert_eq!(
            snapshot.addresses,
            list(&[
                "192.168.1.30:47801",
                "192.168.1.32:47801",
                "192.168.1.31:47801"
            ])
        );
    }

    #[test]
    fn the_local_manifest_is_reused_while_fresh() {
        let dir = TempDir::new("cache-local");
        let paths = dir.paths();

        // Plant a recognisable manifest as if a full inspection had just run.
        let mut planted = ud_doctor::detect();
        planted.hardware.cpu_model = "planted".to_string();
        remember_local_manifest(&paths, &planted);

        let reused = local_manifest(&paths, Duration::from_secs(600));
        assert_eq!(reused.hardware.cpu_model, "planted");
        // The adapters always reflect the present.
        assert_eq!(reused.power.interfaces, ud_doctor::interfaces());

        // Too old: inspected again and remembered.
        std::thread::sleep(Duration::from_millis(1100));
        let fresh = local_manifest(&paths, Duration::from_secs(0));
        assert_ne!(fresh.hardware.cpu_model, "planted");
        let again = local_manifest(&paths, Duration::from_secs(600));
        assert_ne!(again.hardware.cpu_model, "planted");
    }
}
