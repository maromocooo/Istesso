use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant};

use istesso_proto::ProjectKind;

const TIMEOUT: Duration = Duration::from_millis(200);

#[derive(Default)]
struct Scan {
    result: Mutex<Option<io::Result<Vec<ProjectKind>>>>,
    ready: Condvar,
}

// A blocked mount can outlive a manifest request. Later requests share its
// worker until the filesystem call returns.
static ACTIVE: LazyLock<Mutex<BTreeMap<PathBuf, Arc<Scan>>>> = LazyLock::new(Mutex::default);

pub(super) fn read(folder: PathBuf) -> Option<Vec<ProjectKind>> {
    read_with(folder, scan)
}

fn read_with(
    folder: PathBuf,
    inspect: impl FnOnce(&Path) -> io::Result<Vec<ProjectKind>> + Send + 'static,
) -> Option<Vec<ProjectKind>> {
    let pending = {
        let mut active = ACTIVE.lock().unwrap();
        if let Some(scan) = active.get(&folder) {
            Arc::clone(scan)
        } else {
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
            scan
        }
    };
    let (result, _) = pending
        .ready
        .wait_timeout_while(pending.result.lock().unwrap(), TIMEOUT, |value| {
            value.is_none()
        })
        .unwrap();
    result.as_ref()?.as_ref().ok().cloned()
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
}
