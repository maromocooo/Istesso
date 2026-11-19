//! Research prototype (not part of the product): show a folder that lives on another machine,
//! read-only, at a fixed path on a Mac, with nothing added to macOS. See
//! docs/research/unified-mount.md.
//!
//!   unified-mount-proto export <folder>
//!       Answers requests about <folder> on standard input and output (the owning machine).
//!   unified-mount-proto mount <mount-point> -- <command> [args...]
//!       Runs <command> (which must run `export` on the owning machine, for example through
//!       ssh), serves a read-only NFSv3 server on 127.0.0.1 backed by it, mounts it with the
//!       built-in NFS client, and unmounts on Ctrl-C or SIGTERM.
//!   unified-mount-proto serve -- <command> [args...]
//!       Same, without mounting: prints the port and serves until stopped.

mod export;
mod nfs_side;
mod wire;

use nfsserve::tcp::{NFSTcp, NFSTcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

fn usage() -> ExitCode {
    eprintln!("usage: unified-mount-proto export <folder>");
    eprintln!("       unified-mount-proto mount <mount-point> -- <command> [args...]");
    eprintln!("       unified-mount-proto serve -- <command> [args...]");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("export") if args.len() == 2 => run_export(Path::new(&args[1])),
        Some("mount") if args.len() >= 4 && args[2] == "--" => {
            run_mount(Some(PathBuf::from(&args[1])), &args[3..])
        }
        Some("serve") if args.len() >= 3 && args[1] == "--" => run_mount(None, &args[2..]),
        _ => usage(),
    }
}

fn run_export(folder: &Path) -> ExitCode {
    let mut ex = match export::Export::open(folder) {
        Ok(ex) => ex,
        Err(e) => {
            eprintln!("export: cannot open the folder: {e}");
            return ExitCode::FAILURE;
        }
    };
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    match export::serve(&mut ex, &mut stdin.lock(), &mut stdout.lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("export: {e}");
            ExitCode::FAILURE
        }
    }
}

fn check_mount_point(p: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(p).map_err(|e| format!("mount point: {e}"))?;
    if !meta.is_dir() {
        return Err("mount point must be a folder (not a link)".into());
    }
    if std::fs::read_dir(p)
        .map_err(|e| format!("mount point: {e}"))?
        .next()
        .is_some()
    {
        return Err("mount point must be empty".into());
    }
    Ok(())
}

fn run_mount(mount_point: Option<PathBuf>, command: &[String]) -> ExitCode {
    if let Some(mp) = &mount_point {
        if let Err(e) = check_mount_point(mp) {
            eprintln!("mount: {e}");
            return ExitCode::FAILURE;
        }
        if !cfg!(target_os = "macos") {
            eprintln!("mount: this prototype mounts with the macOS NFS client only (on Linux, mounting NFS needs root; the Linux client in the research document is FUSE)");
            return ExitCode::FAILURE;
        }
    }
    let mut child = match Command::new(&command[0])
        .args(&command[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("mount: cannot start the export command: {e}");
            return ExitCode::FAILURE;
        }
    };
    let remote = match nfs_side::Remote::from_child(&mut child) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("mount: {e}");
            let _ = child.kill();
            let _ = child.wait();
            return ExitCode::FAILURE;
        }
    };
    // Ask once before mounting, so that a missing owner shows up here and not as a hung mount.
    match remote.call(&wire::Request::Getattr {
        id: export::ROOT_ID,
    }) {
        Ok((wire::Response::Attr { .. }, _)) => {}
        other => {
            eprintln!("mount: the owning side did not answer: {other:?}");
            let _ = child.kill();
            let _ = child.wait();
            return ExitCode::FAILURE;
        }
    }
    let fs = nfs_side::RemoteFs::new(remote);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("runtime");
    let code = rt.block_on(async move {
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
        eprintln!("serving read-only NFSv3 on 127.0.0.1:{port}");
        if let Some(mp) = &mount_point {
            let opts = format!(
                "vers=3,tcp,port={port},mountport={port},rdonly,nobrowse,nolocks,soft,actimeo=2"
            );
            let mp2 = mp.clone();
            let status = tokio::task::spawn_blocking(move || {
                Command::new("/sbin/mount_nfs")
                    .args(["-o", &opts, "127.0.0.1:/"])
                    .arg(&mp2)
                    .status()
            })
            .await;
            match status {
                Ok(Ok(s)) if s.success() => eprintln!("mounted at {}", mp.display()),
                other => {
                    eprintln!("mount: mount_nfs failed: {other:?}");
                    return ExitCode::FAILURE;
                }
            }
        }
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("signal");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
        if let Some(mp) = &mount_point {
            // Unmount while the server still answers; a hung server would make unmount wait.
            let mp2 = mp.clone();
            let ok = tokio::task::spawn_blocking(move || {
                Command::new("/sbin/umount")
                    .arg(&mp2)
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false)
                    || Command::new("/sbin/umount")
                        .arg("-f")
                        .arg(&mp2)
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false)
            })
            .await
            .unwrap_or(false);
            eprintln!(
                "{} {}",
                if ok { "unmounted" } else { "could not unmount" },
                mp.display()
            );
        }
        ExitCode::SUCCESS
    });
    // Dropping the runtime drops the server and with it the export command's input, which
    // ends that command. Give it a moment, then stop it.
    drop(rt);
    for _ in 0..20 {
        if let Ok(Some(_)) = child.try_wait() {
            return code;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    code
}
