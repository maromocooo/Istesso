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
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
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
fn mount_point(base: &Path, device: &str, folder: &str) -> Result<PathBuf, String> {
    private_dir(base)?;
    let machine = base.join(folder_name(device));
    private_dir(&machine)?;
    let point = machine.join(folder);
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

/// Whether something other than the folder above it is mounted at `point`.
fn is_mounted(point: &Path) -> bool {
    match (
        std::fs::metadata(point),
        point.parent().map(std::fs::metadata),
    ) {
        (Ok(here), Some(Ok(above))) => here.dev() != above.dev(),
        _ => false,
    }
}

fn unmount(point: &Path) -> bool {
    let run = |force: bool| {
        let mut command = Command::new("/sbin/umount");
        if force {
            command.arg("-f");
        }
        command.arg(point).status().is_ok_and(|s| s.success())
    };
    !is_mounted(point) || run(false) || run(true)
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
            format!("vers=3,tcp,port={port},mountport={port},rdonly,nobrowse,nolocks,soft,actimeo=2");
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

        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("signal");
        let mut every = tokio::time::interval(Duration::from_secs(2));
        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => break,
                _ = term.recv() => break,
                _ = every.tick() => {
                    if !is_mounted(&point) {
                        eprintln!("the folder was unmounted from outside");
                        break;
                    }
                }
            }
        }
        // Unmount while the server still answers; a server that is gone would make unmount wait.
        let at = point.clone();
        let ok = tokio::task::spawn_blocking(move || unmount(&at))
            .await
            .unwrap_or(false);
        eprintln!(
            "{} {}",
            if ok { "unmounted" } else { "could not unmount" },
            point.display()
        );
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
