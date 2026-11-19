//! Research prototype (not part of the product): show a folder that lives on another machine,
//! read-only, at a fixed path on a Mac, with nothing added to macOS. See
//! docs/research/unified-mount.md and ADR 0007 (proposal "フォルダーを読むだけで見せる").
//!
//!   unified-mount-proto mount <device> <folder-id> [--address ADDR[:PORT]] [--base DIR]
//!       Asks the paired <device> (its name or the beginning of its id, as for `istesso
//!       peers`) for its folder <folder-id> over Istesso's control channel, serves it as a
//!       read-only NFSv3 server on 127.0.0.1, and mounts that with the built-in NFS client at
//!       <base>/<device name>/<folder-id>. <base> is ~/Istesso unless given; it and the device
//!       folder are made private (0700), and are refused when they are not. Unmounts on Ctrl-C
//!       or SIGTERM, and stops when the folder is unmounted from outside.
//!
//!       For tests only: --hold-before-mount-ms N waits N ms between opening the loopback port
//!       and mounting, and prints the port, so that another process can try to mount first.
//!
//! The device's identity and its list of paired devices are those of `istesso`
//! (`ISTESSO_CONFIG_DIR` points elsewhere).

mod nfs_side;

use istesso_agent::mount::client::Remote;
use istesso_agent::Paths;
use nfsserve::tcp::{NFSTcp, NFSTcpListener};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn usage() -> ExitCode {
    eprintln!("usage: unified-mount-proto mount <device> <folder-id> [--address ADDR[:PORT]] [--base DIR]");
    ExitCode::from(2)
}

struct Options {
    device: String,
    folder: String,
    address: Option<String>,
    base: Option<PathBuf>,
    hold: Option<Duration>,
}

fn parse(args: &[String]) -> Option<Options> {
    let (Some("mount"), Some(device), Some(folder)) = (
        args.first().map(String::as_str),
        args.get(1).cloned(),
        args.get(2).cloned(),
    ) else {
        return None;
    };
    let mut options = Options {
        device,
        folder,
        address: None,
        base: None,
        hold: None,
    };
    let mut rest = args[3..].iter();
    while let Some(flag) = rest.next() {
        let value = rest.next()?;
        match flag.as_str() {
            "--address" => options.address = Some(value.clone()),
            "--base" => options.base = Some(PathBuf::from(value)),
            "--hold-before-mount-ms" => {
                options.hold = Some(Duration::from_millis(value.parse().ok()?))
            }
            _ => return None,
        }
    }
    Some(options)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse(&args) {
        Some(options) if cfg!(target_os = "macos") => run_mount(options),
        Some(_) => {
            eprintln!("mount: this prototype mounts with the macOS NFS client only (on Linux, mounting NFS needs root; the Linux client in the research document is FUSE)");
            ExitCode::FAILURE
        }
        None => usage(),
    }
}

/// A name for the device's folder: the device's name, with anything but letters, digits, `.`,
/// `_` and `-` replaced.
fn folder_name(device: &str) -> String {
    let name: String = device
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '-'
            }
        })
        .take(64)
        .collect();
    match name.trim_matches('.') {
        "" => "device".to_string(),
        _ => name,
    }
}

/// Makes `dir` a folder only this user can enter, or checks that it is one.
fn private_dir(dir: &Path) -> Result<(), String> {
    if std::fs::symlink_metadata(dir).is_err() {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(dir)
            .map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    }
    istesso_agent::mount::private_folder(dir).map_err(|why| format!("{}: {why}", dir.display()))
}

/// The empty folder to mount at: <base>/<device>/<folder-id>, with the two above it private.
/// The path is the real one (no links in it), as the mount table has it. A mount left there
/// by an earlier run that was killed is taken off first; the mount point itself is not
/// looked at before that (a mount whose server is gone may not answer).
fn mount_point(base: &Path, device: &str, folder: &str) -> Result<PathBuf, String> {
    private_dir(base)?;
    let machine = base.join(folder_name(device));
    private_dir(&machine)?;
    let machine = std::fs::canonicalize(&machine).map_err(|e| e.to_string())?;
    let point = machine.join(folder);
    if is_mounted(&point) {
        eprintln!(
            "mount: a mount was left at {}; taking it off",
            point.display()
        );
        if !unmount(&point) {
            return Err(format!(
                "{} is still mounted and could not be unmounted",
                point.display()
            ));
        }
    }
    if std::fs::symlink_metadata(&point).is_err() {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&point)
            .map_err(|e| format!("cannot make {}: {e}", point.display()))?;
    }
    let meta = std::fs::symlink_metadata(&point).map_err(|e| e.to_string())?;
    if !meta.is_dir() {
        return Err(format!("{} is not a folder", point.display()));
    }
    if std::fs::read_dir(&point)
        .map_err(|e| e.to_string())?
        .next()
        .is_some()
    {
        return Err(format!(
            "{} is not empty (something is mounted there already?)",
            point.display()
        ));
    }
    Ok(point)
}

/// Whether the kernel's mount table has something mounted at `point` (a real path). Read
/// without asking any file server (`MNT_NOWAIT`), so a mount whose server is gone, or
/// answers every question with an error, is still seen. In doubt (the table cannot be
/// read): mounted, so that the program does not leave a mount behind.
fn is_mounted(point: &Path) -> bool {
    let want = point.as_os_str().as_bytes();
    for _ in 0..3 {
        // SAFETY: with a null buffer, getfsstat only counts.
        let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
        if count < 0 {
            return true;
        }
        let room = count as usize + 8;
        // SAFETY: statfs is plain data; all zeroes is a valid value.
        let mut table: Vec<libc::statfs> = vec![unsafe { std::mem::zeroed() }; room];
        let bytes = (room * std::mem::size_of::<libc::statfs>()) as libc::c_int;
        // SAFETY: the buffer holds `room` entries of `bytes` bytes in all.
        let filled = unsafe { libc::getfsstat(table.as_mut_ptr(), bytes, libc::MNT_NOWAIT) };
        if filled < 0 {
            return true;
        }
        if filled as usize >= room {
            // The table grew while it was read: read it again.
            continue;
        }
        return table[..filled as usize].iter().any(|entry| {
            // SAFETY: f_mntonname is a NUL-terminated string inside the entry.
            unsafe { std::ffi::CStr::from_ptr(entry.f_mntonname.as_ptr()) }.to_bytes() == want
        });
    }
    true
}

/// Takes the mount at `point` off, by force when it is busy. True when nothing is mounted
/// there any more.
fn unmount(point: &Path) -> bool {
    for force in [false, true] {
        if !is_mounted(point) {
            return true;
        }
        let mut command = Command::new("/sbin/umount");
        if force {
            command.arg("-f");
        }
        let _ = command.arg(point).status();
    }
    !is_mounted(point)
}

/// Unmounts, trying again every second for as long as it takes: the program never ends
/// while the folder is mounted, so that its loopback port cannot be taken by another
/// process that the kernel would then ask for this folder's files.
async fn unmount_until_gone(point: &Path) {
    let mut tries = 0u64;
    loop {
        let at = point.to_path_buf();
        if tokio::task::spawn_blocking(move || unmount(&at))
            .await
            .unwrap_or(false)
        {
            eprintln!("unmounted {}", point.display());
            return;
        }
        if tries.is_multiple_of(10) {
            eprintln!(
                "could not unmount {} yet; trying again every second (the program stays until it is)",
                point.display()
            );
        }
        tries += 1;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn run_mount(options: Options) -> ExitCode {
    // The id becomes a folder name here: check it before anything is asked.
    if !istesso_link::mount::valid_folder_id(&options.folder) {
        eprintln!("mount: a folder id is 1 to 32 of a-z, 0-9 and -, not starting with -");
        return ExitCode::from(2);
    }
    let began = Instant::now();
    let device_asked = options.device.clone();
    let (remote, opened) = match Remote::open(
        &Paths::from_env(),
        &options.device,
        options.address.as_deref(),
        &options.folder,
        move |why| {
            eprintln!("mount: lost the connection to {device_asked} ({why}); reads fail from now on, until this is stopped")
        },
    ) {
        Ok(opened) => opened,
        Err(why) => {
            eprintln!("mount: {why}");
            return ExitCode::FAILURE;
        }
    };
    let open_after = began.elapsed();
    let base = match options.base {
        Some(base) => base,
        None => match std::env::var_os("HOME") {
            Some(home) => PathBuf::from(home).join("Istesso"),
            None => {
                eprintln!("mount: no home folder");
                return ExitCode::FAILURE;
            }
        },
    };
    let point = match mount_point(&base, &opened.device, &options.folder) {
        Ok(point) => point,
        Err(why) => {
            eprintln!("mount: {why}");
            return ExitCode::FAILURE;
        }
    };

    // A panic must not leave the mount behind: take it off while the server still answers
    // (its threads run on while the panicking one unwinds). A panic in one question's task
    // takes the mount off too; the program then sees it gone and ends.
    let hook_point = point.clone();
    let earlier = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        earlier(info);
        let _ = unmount(&hook_point);
    }));

    let remote = Arc::new(remote);
    let gate = Arc::new(nfs_side::Gate::default());
    let fs = match nfs_side::RemoteFs::new(remote.clone(), gate.clone()) {
        Ok(fs) => fs,
        Err(e) => {
            eprintln!("mount: {e}");
            return ExitCode::FAILURE;
        }
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("runtime");
    let code = rt.block_on(async {
        let listener = match NFSTcpListener::bind("127.0.0.1:0", fs).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("mount: cannot listen on 127.0.0.1: {e}");
                return ExitCode::FAILURE;
            }
        };
        let port = listener.get_listen_port();
        tokio::spawn(async move {
            let _ = listener.handle_forever().await;
        });
        if let Some(hold) = options.hold {
            eprintln!("test: serving on 127.0.0.1:{port}; mounting in {} ms", hold.as_millis());
            tokio::time::sleep(hold).await;
        }
        let opts =
            format!("vers=3,tcp,port={port},mountport={port},rdonly,nobrowse,nolocks,soft,actimeo=2,rsize=1048576");
        let at = point.clone();
        let status = tokio::task::spawn_blocking(move || {
            Command::new("/sbin/mount_nfs")
                .args(["-o", &opts, "127.0.0.1:/"])
                .arg(&at)
                .status()
        })
        .await;
        match status {
            Ok(Ok(s)) if s.success() => {}
            other => {
                // Another process may have taken the only MNT. Nothing but the folder's
                // attributes was answered to anyone; stop before anything else is.
                eprintln!(
                    "mount: mount_nfs failed ({other:?}); {} later mount request(s) were refused; stopping",
                    gate.refused_mounts.load(Ordering::Relaxed)
                );
                if is_mounted(&point) {
                    unmount_until_gone(&point).await;
                }
                return ExitCode::FAILURE;
            }
        }
        gate.open();
        eprintln!(
            "mounted {} of {} read-only at {} after {:.3} s (the folder was open after {:.3} s)",
            options.folder,
            opened.device,
            point.display(),
            began.elapsed().as_secs_f64(),
            open_after.as_secs_f64(),
        );

        use tokio::signal::unix::{signal, SignalKind};
        let mut signals = [
            SignalKind::interrupt(),
            SignalKind::terminate(),
            SignalKind::hangup(),
            SignalKind::quit(),
        ]
        .map(|kind| signal(kind).expect("signal"));
        let [interrupt, terminate, hangup, quit] = &mut signals;
        let mut every = tokio::time::interval(Duration::from_secs(2));
        loop {
            tokio::select! {
                _ = interrupt.recv() => break,
                _ = terminate.recv() => break,
                _ = hangup.recv() => break,
                _ = quit.recv() => break,
                _ = every.tick() => {
                    let at = point.clone();
                    let mounted = tokio::task::spawn_blocking(move || is_mounted(&at))
                        .await
                        .unwrap_or(true);
                    if !mounted {
                        eprintln!("the folder was unmounted from outside");
                        break;
                    }
                }
            }
        }
        // Unmount while the server still answers; the port stays this program's until the
        // mount is gone.
        unmount_until_gone(&point).await;
        ExitCode::SUCCESS
    });
    remote.close();
    eprintln!(
        "refused: {} later mount request(s), {} unknown handle(s), {} question(s) before the mount",
        gate.refused_mounts.load(Ordering::Relaxed),
        gate.refused_handles.load(Ordering::Relaxed),
        gate.refused_early.load(Ordering::Relaxed),
    );
    drop(rt);
    code
}
