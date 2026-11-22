//! The cross-OS application catalogue.
//!
//! Each host lists what is installed on it ([`scan_local`]); [`merge`] turns
//! the lists of all hosts into one entry per application, so the user sees a
//! single icon no matter where the app lives. Apps whose hosts are all
//! stopped stay in the catalogue with a badge instead of disappearing.

pub mod linux;
pub mod macos;

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

use istesso_proto::{AppEntry, AppInstance, Badge, HostApps, HostState, OsFamily, UnifiedApp};

/// Maximum size of a desktop entry or bundle Info.plist, in bytes.
const MAX_ENTRY_BYTES: u64 = 1024 * 1024;

/// Reads a regular file, following links and skipping FIFOs, sockets, devices and directories.
/// Checking the path before opening it avoids waiting for a FIFO writer. The scanned folders
/// belong to the user or root. A same-user swap to a FIFO between the check and open is
/// outside this guard. We recheck the open handle and bound the read to cap memory use
/// even if the file grows.
fn read_small_file(path: &Path, limit: u64) -> Option<Vec<u8>> {
    if !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= limit).then_some(bytes)
}

/// Lists the applications installed on this machine.
pub fn scan_local() -> Vec<AppEntry> {
    match OsFamily::current() {
        OsFamily::Linux => linux::scan(&linux::default_dirs()),
        OsFamily::Macos => macos::scan(&macos::default_dirs()),
        _ => Vec::new(),
    }
}

/// Names that different platforms use for the same application.
const ALIASES: &[(&str, &str)] = &[
    ("firefox web browser", "firefox"),
    ("code", "visual studio code"),
    ("code oss", "visual studio code"),
    ("chromium web browser", "chromium"),
    ("google chrome stable", "google chrome"),
];

/// Reduces an application name to the key used to recognise the same app on
/// different hosts: lower case, punctuation removed, known aliases resolved.
pub fn match_key(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    let normalised = cleaned
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    ALIASES
        .iter()
        .find(|(alias, _)| *alias == normalised)
        .map(|(_, canonical)| canonical.to_string())
        .unwrap_or(normalised)
}

/// Merges the app lists of several hosts into one catalogue.
///
/// A host that has the same application installed twice (a package and a
/// snap, say) contributes one instance: the first one it listed, which is the
/// one its own desktop would show first.
pub fn merge(hosts: &[HostApps]) -> Vec<UnifiedApp> {
    // key -> (instances, per-instance host facts needed to pick a host)
    let mut groups: BTreeMap<String, Vec<(AppInstance, f32, bool)>> = BTreeMap::new();
    let mut listed: BTreeSet<(String, String)> = BTreeSet::new();
    for host_apps in hosts {
        let host = &host_apps.host;
        for app in &host_apps.apps {
            let key = match_key(&app.name);
            if key.is_empty() {
                continue;
            }
            if !listed.insert((key.clone(), host.id.clone())) {
                continue;
            }
            let instance = AppInstance {
                host_id: host.id.clone(),
                host_name: host.name.clone(),
                host_state: host.state,
                local: host.local,
                app: app.clone(),
            };
            groups
                .entry(key)
                .or_default()
                .push((instance, host.load, host.wake_possible));
        }
    }

    let mut unified: Vec<UnifiedApp> = groups
        .into_iter()
        .map(|(key, mut entries)| {
            // Stable order: the local machine first, then by host name.
            entries.sort_by(|(a, _, _), (b, _, _)| {
                b.local
                    .cmp(&a.local)
                    .then_with(|| a.host_name.cmp(&b.host_name))
            });

            let preferred_host = entries
                .iter()
                .filter(|(i, _, _)| i.host_state == HostState::Awake)
                .min_by(|(a, load_a, _), (b, load_b, _)| {
                    // Local beats remote (no streaming at all); among remote
                    // hosts the least loaded one wins.
                    b.local
                        .cmp(&a.local)
                        .then_with(|| load_a.total_cmp(load_b))
                        .then_with(|| a.host_name.cmp(&b.host_name))
                })
                .map(|(i, _, _)| i.host_id.clone());

            let badge = preferred_host.is_none().then(|| Badge::HostStopped {
                wake_possible: entries.iter().any(|(_, _, wake)| *wake),
            });

            let name = entries[0].0.app.name.clone();
            UnifiedApp {
                key,
                name,
                instances: entries.into_iter().map(|(i, _, _)| i).collect(),
                preferred_host,
                badge,
            }
        })
        .collect();

    unified.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.key.cmp(&b.key))
    });
    unified
}

#[cfg(test)]
mod tests {
    use super::*;
    use istesso_proto::{HostRef, Launch};

    #[cfg(unix)]
    pub(super) fn make_fifo(path: &Path) {
        assert!(std::process::Command::new("mkfifo")
            .arg(path)
            .status()
            .unwrap()
            .success());
    }

    #[cfg(unix)]
    pub(super) fn with_timeout<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || sender.send(work()).unwrap());
        receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("file read or scan did not finish")
    }

    #[test]
    fn reads_files_up_to_the_size_limit() {
        let root =
            std::env::temp_dir().join(format!("istesso-catalog-limit-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("entry");
        assert!(read_small_file(&path, MAX_ENTRY_BYTES).is_none());
        let mut bytes = vec![b'x'; MAX_ENTRY_BYTES as usize];
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(
            read_small_file(&path, MAX_ENTRY_BYTES).as_deref(),
            Some(bytes.as_slice())
        );
        bytes.push(b'x');
        std::fs::write(&path, &bytes).unwrap();
        assert!(read_small_file(&path, MAX_ENTRY_BYTES).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn skips_a_fifo_without_waiting_for_a_writer() {
        let root =
            std::env::temp_dir().join(format!("istesso-catalog-fifo-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let pipe = root.join("pipe");
        make_fifo(&pipe);
        assert!(with_timeout(move || read_small_file(&pipe, MAX_ENTRY_BYTES)).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    fn host(id: &str, state: HostState, local: bool, load: f32, wake: bool) -> HostRef {
        HostRef {
            id: id.to_string(),
            name: id.to_string(),
            state,
            local,
            wake_possible: wake,
            load,
        }
    }

    fn linux_app(name: &str) -> AppEntry {
        let id = format!("{}.desktop", name.to_lowercase().replace(' ', "-"));
        AppEntry {
            id: id.clone(),
            name: name.to_string(),
            platform: OsFamily::Linux,
            version: None,
            launch: Launch::DesktopEntry {
                desktop_id: id,
                exec: name.to_lowercase(),
            },
            icon: None,
            categories: Vec::new(),
        }
    }

    fn mac_app(name: &str) -> AppEntry {
        let bundle_id = format!("com.example.{}", name.to_lowercase().replace(' ', ""));
        AppEntry {
            id: bundle_id.clone(),
            name: name.to_string(),
            platform: OsFamily::Macos,
            version: None,
            launch: Launch::AppBundle {
                bundle_id,
                path: format!("/Applications/{name}.app"),
            },
            icon: None,
            categories: Vec::new(),
        }
    }

    #[test]
    fn match_key_normalises_names() {
        assert_eq!(match_key("Google Chrome"), "google chrome");
        assert_eq!(match_key("  Visual   Studio-Code "), "visual studio code");
        assert_eq!(match_key("Firefox Web Browser"), "firefox");
        assert_eq!(match_key("Code"), "visual studio code");
        assert_eq!(match_key("ターミナル"), "ターミナル");
        assert_eq!(match_key("!!!"), "");
    }

    #[test]
    fn same_app_on_two_hosts_becomes_one_icon() {
        let hosts = [
            HostApps {
                host: host("ubuntu", HostState::Awake, false, 0.1, true),
                apps: vec![linux_app("Firefox Web Browser"), linux_app("Blender")],
            },
            HostApps {
                host: host("air", HostState::Awake, true, 0.5, false),
                apps: vec![mac_app("Firefox"), mac_app("Dia")],
            },
        ];
        let merged = merge(&hosts);
        let names: Vec<&str> = merged.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Blender", "Dia", "Firefox"]);

        let firefox = &merged[2];
        assert_eq!(firefox.instances.len(), 2);
        // The local copy is listed first and wins even though the other host
        // is less loaded: local apps need no streaming.
        assert!(firefox.instances[0].local);
        assert_eq!(firefox.preferred_host.as_deref(), Some("air"));
        assert_eq!(firefox.badge, None);
    }

    #[test]
    fn remote_only_app_picks_the_least_loaded_awake_host() {
        let hosts = [
            HostApps {
                host: host("busy", HostState::Awake, false, 0.9, true),
                apps: vec![linux_app("Blender")],
            },
            HostApps {
                host: host("idle", HostState::Awake, false, 0.1, true),
                apps: vec![linux_app("Blender")],
            },
            HostApps {
                host: host("asleep", HostState::Asleep, false, 0.0, true),
                apps: vec![linux_app("Blender")],
            },
        ];
        let merged = merge(&hosts);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].preferred_host.as_deref(), Some("idle"));
        assert_eq!(merged[0].badge, None);
    }

    #[test]
    fn app_on_stopped_hosts_keeps_its_icon_with_a_badge() {
        let hosts = [
            HostApps {
                host: host("studio", HostState::Asleep, false, 0.0, true),
                apps: vec![mac_app("Xcode")],
            },
            HostApps {
                host: host("air", HostState::Unknown, false, 0.0, false),
                apps: vec![mac_app("Xcode")],
            },
        ];
        let merged = merge(&hosts);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].preferred_host, None);
        assert_eq!(
            merged[0].badge,
            Some(Badge::HostStopped {
                wake_possible: true
            })
        );
    }

    #[test]
    fn stopped_host_without_a_wake_method_is_reported() {
        let hosts = [HostApps {
            host: host("air", HostState::Asleep, false, 0.0, false),
            apps: vec![mac_app("Dia")],
        }];
        let merged = merge(&hosts);
        assert_eq!(
            merged[0].badge,
            Some(Badge::HostStopped {
                wake_possible: false
            })
        );
    }

    #[test]
    fn empty_input_gives_an_empty_catalogue() {
        assert!(merge(&[]).is_empty());
    }
}
