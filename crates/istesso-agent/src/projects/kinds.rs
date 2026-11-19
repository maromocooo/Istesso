use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant};

use istesso_proto::ProjectKind;

pub(super) const TIMEOUT: Duration = Duration::from_millis(200);

#[derive(Default)]
pub(super) struct Scan {
    result: Mutex<Option<io::Result<Vec<ProjectKind>>>>,
    ready: Condvar,
}

// A blocked mount can outlive a manifest request. Later requests share its
// worker until the filesystem call returns.
static ACTIVE: LazyLock<Mutex<BTreeMap<PathBuf, Arc<Scan>>>> = LazyLock::new(Mutex::default);

/// Starts looking at `folder` on a worker of its own. All folders of one
/// listing are started before any is waited for, so that several blocked
/// mounts cost one wait, not one each.
pub(super) fn start(folder: PathBuf) -> Option<Arc<Scan>> {
    start_with(folder, scan)
}

fn start_with(
    folder: PathBuf,
    inspect: impl FnOnce(&Path) -> io::Result<Vec<ProjectKind>> + Send + 'static,
) -> Option<Arc<Scan>> {
    let mut active = ACTIVE.lock().unwrap();
    if let Some(scan) = active.get(&folder) {
        return Some(Arc::clone(scan));
    }
    let scan = Arc::new(Scan::default());
    let worker = Arc::clone(&scan);
    let key = folder.clone();
    std::thread::Builder::new()
        .spawn(move || {
            let result = inspect(&folder);
            let mut active = ACTIVE.lock().unwrap();
            active.remove(&folder);
            *worker.result.lock().unwrap() = Some(result);
            worker.ready.notify_all();
        })
        .ok()?;
    active.insert(key, Arc::clone(&scan));
    Some(scan)
}

/// What the scan found, if it is done by `deadline`.
pub(super) fn wait(scan: &Scan, deadline: Instant) -> Option<Vec<ProjectKind>> {
    let left = deadline.saturating_duration_since(Instant::now());
    let (result, _) = scan
        .ready
        .wait_timeout_while(scan.result.lock().unwrap(), left, |value| value.is_none())
        .unwrap();
    result.as_ref()?.as_ref().ok().cloned()
}

#[cfg(test)]
fn read_with(
    folder: PathBuf,
    inspect: impl FnOnce(&Path) -> io::Result<Vec<ProjectKind>> + Send + 'static,
) -> Option<Vec<ProjectKind>> {
    let scan = start_with(folder, inspect)?;
    wait(&scan, Instant::now() + TIMEOUT)
}

fn scan(folder: &Path) -> io::Result<Vec<ProjectKind>> {
    let started = Instant::now();
    let mut kinds = BTreeSet::new();
    for entry in fs::read_dir(folder)? {
        if started.elapsed() >= TIMEOUT {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let entry = entry?;
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let kind = match name {
            "Cargo.toml" if entry.file_type()?.is_file() => ProjectKind::Rust,
            "package.json" if entry.file_type()?.is_file() => ProjectKind::Node,
            "Package.swift" if entry.file_type()?.is_file() => ProjectKind::Apple,
            _ if name.ends_with(".xcodeproj") && entry.file_type()?.is_dir() => ProjectKind::Apple,
            _ if name.ends_with(".blend") && entry.file_type()?.is_file() => ProjectKind::Blender,
            _ => continue,
        };
        kinds.insert(kind);
    }
    if kinds.is_empty() {
        kinds.insert(ProjectKind::Unknown);
    }
    Ok(kinds.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn a_stalled_folder_times_out_and_reuses_the_pending_scan() {
        let folder = PathBuf::from("synthetic-stalled-folder");
        let (release, blocked) = mpsc::channel();
        assert!(read_with(folder.clone(), move |_| {
            blocked.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(vec![ProjectKind::Rust])
        })
        .is_none());
        let (started, extra_scan) = mpsc::channel();
        assert!(read_with(folder.clone(), move |_| {
            started.send(()).unwrap();
            Ok(vec![])
        })
        .is_none());
        assert!(extra_scan.try_recv().is_err());
        release.send(()).unwrap();
        // A caller racing with release may share the original result.
        let rescan = || read_with(folder.clone(), |_| Ok(vec![ProjectKind::Apple]));
        assert!(rescan().is_some());
        assert_eq!(rescan(), Some(vec![ProjectKind::Apple]));
    }

    #[test]
    fn several_stalled_folders_share_one_wait() {
        let mut releases = Vec::new();
        let mut scans = Vec::new();
        for i in 0..3 {
            let (release, blocked) = mpsc::channel::<()>();
            let folder = PathBuf::from(format!("synthetic-stalled-folder-{i}"));
            scans.push(
                start_with(folder, move |_| {
                    let _ = blocked.recv_timeout(Duration::from_secs(5));
                    Ok(vec![ProjectKind::Rust])
                })
                .unwrap(),
            );
            releases.push(release);
        }
        let started = Instant::now();
        let deadline = started + TIMEOUT;
        for scan in &scans {
            assert!(wait(scan, deadline).is_none());
        }
        assert!(started.elapsed() < TIMEOUT * 2);
        drop(releases);
    }
}
