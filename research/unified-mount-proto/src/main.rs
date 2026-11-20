//! Research prototype (not part of the product): show a folder that lives on another machine,
//! read-only, at a fixed path on a Mac, with nothing added to macOS. See
//! docs/research/unified-mount.md and ADR 0007 (proposal "フォルダーを読むだけで見せる").
//!
//!   unified-mount-proto mount <device> <folder-id> [--address ADDR[:PORT]] [--base DIR]
//!                             [--machine-folder NAME] [--unmount-when-unused SECS]
//!       Asks the paired <device> (its name or the beginning of its id, as for `istesso
//!       peers`) for its folder <folder-id> over Istesso's control channel, serves it as a
//!       read-only NFSv3 server on 127.0.0.1, and mounts that with the built-in NFS client at
//!       <base>/<device name>/<folder-id>. <base> is ~/Istesso unless given; it and the device
//!       folder are made private (0700), and are refused when they are not. Unmounts on Ctrl-C
//!       or SIGTERM, and stops when the folder is unmounted from outside. --machine-folder
//!       names the folder below <base> instead of the device's name (`ubuntu`, say, for the
//!       name the shell's cd switch knows the machine by). --unmount-when-unused tries a plain
//!       unmount every SECS seconds (refused while any program here has a file or a folder in
//!       the mount open), and ends once it went through.
//!
//!   unified-mount-proto ensure <device> <folder-id> --base DIR --machine-folder NAME
//!                              [--address ADDR[:PORT]] [--state DIR] [--wait-ms N]
//!                              [--unmount-when-unused SECS]
//!       The mount on first use, for the cd switch: when nothing is mounted at
//!       <base>/<NAME>/<folder-id>, starts `mount` for it on its own (a session of its own,
//!       its messages in a log in <state>), and waits until the folder is mounted, the
//!       `mount` ends, or N ms (20 s) have gone by. Prints the mount point when it is
//!       mounted; says why not, and ends with 1, when it is not. The `mount` it starts takes
//!       the folder off once nothing uses it (--unmount-when-unused SECS, 30 unless given).
//!
//!   unified-mount-proto release <folder-id> --base DIR --machine-folder NAME [--state DIR]
//!       Unmounts <base>/<NAME>/<folder-id> when nothing on this Mac uses it: a plain
//!       unmount, which the system refuses while any program has a file or a folder in it
//!       open. Ends with 0 when nothing is mounted there any more, 3 when it is in use (it
//!       stays mounted), 1 on any other failure. The `mount` that served it sees it gone and
//!       ends.
//!
//!       For tests only: --hold-before-mount-ms N waits N ms between opening the loopback port
//!       and mounting, and prints the port, so that another process can try to mount first.
//!
//!   unified-mount-proto watch <mount-point>     (started by `mount`, not by hand)
//!       Waits, in a session of its own, until the `mount` that started it is gone however
//!       it ended (its end of a pipe closes), and then takes off a mount it left.
//!
//! The device's identity and its list of paired devices are those of `istesso`
//! (`ISTESSO_CONFIG_DIR` points elsewhere).

/// One line on the standard error. A standard error that is gone (the terminal was closed)
/// does not end the program, as `eprintln!` would by panicking.
macro_rules! say {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

mod glue;
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
    say!("usage: unified-mount-proto mount <device> <folder-id> [--address ADDR[:PORT]] [--base DIR]");
    ExitCode::from(2)
}

struct Options {
    device: String,
    folder: String,
    address: Option<String>,
    base: Option<PathBuf>,
    /// The folder below the base, instead of one named after the device.
    machine: Option<String>,
    /// Every so often, a plain unmount: the mount goes once nothing on this Mac uses it.
    unused: Option<Duration>,
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
        machine: None,
        unused: None,
        hold: None,
    };
    let mut rest = args[3..].iter();
    while let Some(flag) = rest.next() {
        let value = rest.next()?;
        match flag.as_str() {
            "--address" => options.address = Some(value.clone()),
            "--base" => options.base = Some(PathBuf::from(value)),
            "--machine-folder" => options.machine = Some(value.clone()),
            "--unmount-when-unused" => {
                options.unused = Some(Duration::from_secs(
                    value.parse().ok().filter(|secs| *secs > 0)?,
                ))
            }
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
    if let (Some("watch"), Some(point), 2) =
        (args.first().map(String::as_str), args.get(1), args.len())
    {
        return run_watch(Path::new(point));
    }
    match args.first().map(String::as_str) {
        Some("ensure") => {
            return match glue::parse_ensure(&args[1..]) {
                Some(options) if cfg!(target_os = "macos") => glue::run_ensure(options),
                Some(_) => {
                    say!("ensure: this prototype mounts with the macOS NFS client only");
                    ExitCode::FAILURE
                }
                None => {
                    say!("usage: unified-mount-proto ensure <device> <folder-id> --base DIR --machine-folder NAME [--address ADDR[:PORT]] [--state DIR] [--wait-ms N] [--unmount-when-unused SECS]");
                    ExitCode::from(2)
                }
            }
        }
        Some("release") => {
            return match glue::parse_release(&args[1..]) {
                Some(options) => glue::run_release(options),
                None => {
                    say!("usage: unified-mount-proto release <folder-id> --base DIR --machine-folder NAME [--state DIR]");
                    ExitCode::from(2)
                }
            }
        }
        _ => {}
    }
    match parse(&args) {
        Some(options) if cfg!(target_os = "macos") => run_mount(options),
        Some(_) => {
            say!("mount: this prototype mounts with the macOS NFS client only (on Linux, mounting NFS needs root; the Linux client in the research document is FUSE)");
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

/// The empty folder to mount at: <base>/<machine>/<folder-id>, with the two above it private.
/// `machine` is a checked folder name ([`glue::valid_machine_folder`]). The path is the real
/// one (no links in it), as the mount table has it. A mount left there by an earlier run that
/// was killed is taken off first, by a plain unmount: one that a program here still uses may
/// be another run's that this one cannot tell apart, and is not forced off (the person can,
/// with `umount -f`). The mount point itself is not looked at before that (a mount whose
/// server is gone may not answer).
fn mount_point(base: &Path, machine: &str, folder: &str) -> Result<(PathBuf, bool), String> {
    private_dir(base)?;
    let machine = base.join(machine);
    private_dir(&machine)?;
    let machine = std::fs::canonicalize(&machine).map_err(|e| e.to_string())?;
    let point = machine.join(folder);
    if is_mounted(&point) {
        say!(
            "mount: a mount was left at {}; taking it off",
            point.display()
        );
        match glue::plain_unmount(&point) {
            Ok(()) => {}
            Err(e) if e.raw_os_error() == Some(libc::EBUSY) => {
                return Err(format!(
                    "a mount is at {} already and a program on this Mac uses it; close what uses it (or, if it is a mount left behind that no longer answers, take it off with `umount -f`) and try again",
                    point.display()
                ));
            }
            Err(_) if !is_mounted(&point) => {}
            Err(e) => {
                return Err(format!(
                    "{} is still mounted and could not be unmounted: {e}",
                    point.display()
                ));
            }
        }
    }
    let made = std::fs::symlink_metadata(&point).is_err();
    if made {
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
    Ok((point, made))
}

/// Takes away the mount point this run made, when nothing was mounted there: an empty folder
/// would look like an empty project.
fn forget_point(point: &Path, made: bool) {
    if made && !is_mounted(point) {
        let _ = std::fs::remove_dir(point);
    }
}

/// A mount's identity in the kernel's table (`f_fsid`). Another mount at the same place,
/// made later, has another one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fsid(pub [i32; 2]);

impl std::fmt::Display for Fsid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.0[0], self.0[1])
    }
}

impl std::str::FromStr for Fsid {
    type Err = ();
    fn from_str(text: &str) -> Result<Self, ()> {
        let mut parts = text.split(' ');
        let (Some(a), Some(b), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(());
        };
        Ok(Fsid([
            a.parse().map_err(|_| ())?,
            b.parse().map_err(|_| ())?,
        ]))
    }
}

/// The mount this process made (the mount program), or watches over (the watcher), once it
/// is known. With it, only that mount is ever taken off: a later mount at the same place is
/// another program's.
static OWN: std::sync::OnceLock<Fsid> = std::sync::OnceLock::new();

/// Whether this process's own mount is still in the table at `point`; without one known,
/// whether anything is. In doubt (the table cannot be read): yes.
fn own_is_mounted(point: &Path) -> bool {
    match (OWN.get(), mounted_fsids(point)) {
        (_, Err(())) => true,
        (Some(own), Ok(found)) => found.contains(own),
        (None, Ok(found)) => !found.is_empty(),
    }
}

/// Whether what an unmount at `point` would take off is this process's own mount (the
/// topmost one there). Without one known, or in doubt: yes.
fn own_is_on_top(point: &Path) -> bool {
    match (OWN.get(), mounted_fsids(point)) {
        (Some(own), Ok(found)) => found.last() == Some(own),
        _ => true,
    }
}

/// Whether the kernel's mount table has something mounted at `point` (a real path). Read
/// without asking any file server (`MNT_NOWAIT`), so a mount whose server is gone, or
/// answers every question with an error, is still seen. In doubt (the table cannot be
/// read): mounted, so that the program does not leave a mount behind.
pub(crate) fn is_mounted(point: &Path) -> bool {
    !matches!(mounted_fsids(point), Ok(found) if found.is_empty())
}

/// The mounts the kernel's table has at `point` (a real path), the topmost last; read as
/// for [`is_mounted`]. `Err` when the table cannot be read.
pub(crate) fn mounted_fsids(point: &Path) -> Result<Vec<Fsid>, ()> {
    const _: () = assert!(std::mem::size_of::<libc::fsid_t>() == 8);
    let want = point.as_os_str().as_bytes();
    for _ in 0..3 {
        // SAFETY: with a null buffer, getfsstat only counts.
        let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
        if count < 0 {
            return Err(());
        }
        let room = count as usize + 8;
        // SAFETY: statfs is plain data; all zeroes is a valid value.
        let mut table: Vec<libc::statfs> = vec![unsafe { std::mem::zeroed() }; room];
        let bytes = (room * std::mem::size_of::<libc::statfs>()) as libc::c_int;
        // SAFETY: the buffer holds `room` entries of `bytes` bytes in all.
        let filled = unsafe { libc::getfsstat(table.as_mut_ptr(), bytes, libc::MNT_NOWAIT) };
        if filled < 0 {
            return Err(());
        }
        if filled as usize >= room {
            // The table grew while it was read: read it again.
            continue;
        }
        return Ok(table[..filled as usize]
            .iter()
            .filter(|entry| {
                // SAFETY: f_mntonname is a NUL-terminated string inside the entry.
                unsafe { std::ffi::CStr::from_ptr(entry.f_mntonname.as_ptr()) }.to_bytes() == want
            })
            .map(|entry| {
                // SAFETY: fsid_t is two i32 (checked above); its field is private in libc.
                Fsid(unsafe { std::ptr::read((&entry.f_fsid as *const libc::fsid_t).cast()) })
            })
            .collect());
    }
    Err(())
}

/// Takes the mount at `point` off, by force when it is busy: this process's own mount once
/// it is known ([`OWN`]), and never one made later at the same place by another program.
/// True when the own mount (or, before one is known, any mount) is not there any more.
fn unmount(point: &Path) -> bool {
    for force in [false, true] {
        if !own_is_mounted(point) {
            return true;
        }
        if !own_is_on_top(point) {
            // Another mount lies on top of this one; taking it off would take off another
            // program's. Tried again later.
            return false;
        }
        let mut command = Command::new("/sbin/umount");
        if force {
            command.arg("-f");
        }
        let _ = command.arg(point).status();
    }
    !own_is_mounted(point)
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
            say!("unmounted {}", point.display());
            return;
        }
        if tries.is_multiple_of(10) {
            say!(
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
        say!("mount: a folder id is 1 to 32 of a-z, 0-9 and -, not starting with -");
        return ExitCode::from(2);
    }
    if let Some(machine) = &options.machine {
        if !glue::valid_machine_folder(machine) {
            say!("mount: {}", glue::MACHINE_FOLDER_RULE);
            return ExitCode::from(2);
        }
    }
    let began = Instant::now();
    // For the next `ensure`, which waits for this program to end before it mounts here again.
    say!("process: {}", std::process::id());
    let paths = Paths::from_env();
    // The place first, from the list of paired devices, without asking the device: a mount
    // left there by a run that was killed is taken off even when the device cannot be
    // reached now.
    let device = match istesso_pair::PeerStore::open(paths.peers())
        .map_err(|e| e.to_string())
        .and_then(|peers| {
            peers
                .find(&options.device)
                .map_err(|e| e.to_string())?
                .map(|peer| peer.name.clone())
                .ok_or_else(|| format!("no paired device matches '{}'", options.device))
        }) {
        Ok(name) => name,
        Err(why) => {
            say!("mount: {why}");
            return ExitCode::FAILURE;
        }
    };
    let base = match options.base {
        Some(base) => base,
        None => match std::env::var_os("HOME") {
            Some(home) => PathBuf::from(home).join("Istesso"),
            None => {
                say!("mount: no home folder");
                return ExitCode::FAILURE;
            }
        },
    };
    let machine = options
        .machine
        .clone()
        .unwrap_or_else(|| folder_name(&device));
    let (point, made) = match mount_point(&base, &machine, &options.folder) {
        Ok(found) => found,
        Err(why) => {
            say!("mount: {why}");
            return ExitCode::FAILURE;
        }
    };
    let device_asked = options.device.clone();
    let (remote, opened) = match Remote::open(
        &paths,
        &options.device,
        options.address.as_deref(),
        &options.folder,
        move |why| {
            say!("mount: lost the connection to {device_asked} ({why}); reads fail from now on, until this is stopped")
        },
    ) {
        Ok(opened) => opened,
        Err(why) => {
            say!("mount: {why}");
            forget_point(&point, made);
            return ExitCode::FAILURE;
        }
    };
    let open_after = began.elapsed();

    // The watcher: a process of its own that takes the mount off when this one is gone,
    // however it ends (SIGKILL included), so that the port is never free while the folder
    // is mounted for longer than it takes to unmount.
    let mut watcher = match start_watcher(&point) {
        Ok(watcher) => watcher,
        Err(why) => {
            say!("mount: cannot start the watcher: {why}");
            forget_point(&point, made);
            return ExitCode::FAILURE;
        }
    };
    say!("watcher: process {}", watcher.id());
    // What the watcher is told: the own mount once it is made, or that there is none.
    let mut to_watcher = watcher.stdin.take();

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
            say!("mount: {e}");
            tell_watcher(&mut to_watcher, "none");
            forget_point(&point, made);
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
                say!("mount: cannot listen on 127.0.0.1: {e}");
                tell_watcher(&mut to_watcher, "none");
                forget_point(&point, made);
                return ExitCode::FAILURE;
            }
        };
        let port = listener.get_listen_port();
        tokio::spawn(async move {
            let _ = listener.handle_forever().await;
        });
        // The signals are taken from here on, before anything is mounted: one that comes
        // while mount_nfs runs is answered once it is done, by unmounting.
        use tokio::signal::unix::{signal, SignalKind};
        let mut signals = [
            SignalKind::interrupt(),
            SignalKind::terminate(),
            SignalKind::hangup(),
            SignalKind::quit(),
        ]
        .map(|kind| signal(kind).expect("signal"));
        if let Some(hold) = options.hold {
            say!("test: serving on 127.0.0.1:{port}; mounting in {} ms", hold.as_millis());
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
                say!(
                    "mount: mount_nfs failed ({other:?}); {} later mount request(s) were refused; stopping",
                    gate.refused_mounts.load(Ordering::Relaxed)
                );
                if is_mounted(&point) {
                    unmount_until_gone(&point).await;
                } else {
                    tell_watcher(&mut to_watcher, "none");
                }
                forget_point(&point, made);
                return ExitCode::FAILURE;
            }
        }
        // The own mount, by its identity in the table: the topmost one at the place, which
        // mount_nfs has just made.
        if let Some(own) = mounted_fsids(&point).ok().and_then(|found| found.last().copied()) {
            let _ = OWN.set(own);
            say!("fsid: {own}");
            tell_watcher(&mut to_watcher, &format!("fsid {own}"));
        }
        gate.open();
        say!(
            "mounted {} of {} read-only at {} after {:.3} s (the folder was open after {:.3} s)",
            options.folder,
            opened.device,
            point.display(),
            began.elapsed().as_secs_f64(),
            open_after.as_secs_f64(),
        );

        let [interrupt, terminate, hangup, quit] = &mut signals;
        let mut every = tokio::time::interval(Duration::from_secs(2));
        let mut tried = Instant::now();
        loop {
            tokio::select! {
                _ = interrupt.recv() => break,
                _ = terminate.recv() => break,
                _ = hangup.recv() => break,
                _ = quit.recv() => break,
                _ = every.tick() => {
                    let at = point.clone();
                    let mounted = tokio::task::spawn_blocking(move || own_is_mounted(&at))
                        .await
                        .unwrap_or(true);
                    if !mounted {
                        say!("the folder was unmounted from outside");
                        break;
                    }
                    // A plain unmount, which the system refuses while any program here has a
                    // file or a folder in the mount open; only of the own mount.
                    if options.unused.is_some_and(|unused| tried.elapsed() >= unused) {
                        tried = Instant::now();
                        let at = point.clone();
                        let gone = tokio::task::spawn_blocking(move || {
                            own_is_on_top(&at) && glue::plain_unmount(&at).is_ok()
                        })
                        .await
                        .unwrap_or(false);
                        if gone {
                            say!("nothing on this Mac used the folder; unmounted it");
                            break;
                        }
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
    say!(
        "refused: {} later mount request(s), {} unknown handle(s), {} question(s) before the mount",
        gate.refused_mounts.load(Ordering::Relaxed),
        gate.refused_handles.load(Ordering::Relaxed),
        gate.refused_early.load(Ordering::Relaxed),
    );
    drop(rt);
    // The mount is off: the watcher sees its pipe close, finds the own mount gone, and ends.
    drop(to_watcher);
    let _ = watcher.wait();
    code
}

/// One line to the watcher: `fsid A B`, the own mount, or `none`, nothing of this program's
/// is mounted.
fn tell_watcher(pipe: &mut Option<std::process::ChildStdin>, line: &str) {
    use std::io::Write as _;
    if let Some(pipe) = pipe.as_mut() {
        let _ = writeln!(pipe, "{line}");
    }
}

/// What the watcher was told, from all the lines it read: the own mount, `Some(None)` for
/// "nothing of mine is mounted", `None` when it was told nothing (the program was killed
/// before it knew).
fn told(text: &str) -> Option<Option<Fsid>> {
    let mut told = None;
    for line in text.lines() {
        if line == "none" {
            told = Some(None);
        } else if let Some(own) = line.strip_prefix("fsid ").and_then(|f| f.parse().ok()) {
            told = Some(Some(own));
        }
    }
    told
}

/// Starts `watch` for `point` in a session of its own (the terminal's signals do not reach
/// it), with a pipe whose writing end only this process holds.
fn start_watcher(point: &Path) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    let mut command = Command::new(std::env::current_exe()?);
    // In `/`: never in the folder that will be mounted, which would keep the mount in use.
    command
        .arg("watch")
        .arg(point)
        .current_dir("/")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null());
    // SAFETY: setsid is safe between fork and exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command.spawn()
}

/// The watcher: waits until the pipe from the program that started it closes (it ended, was
/// killed or crashed), then takes off a mount it left at `point`, trying for half a minute.
fn run_watch(point: &Path) -> ExitCode {
    use std::io::Read as _;
    // This process only cleans up after the other one: no signal of a terminal or a script
    // ends it early.
    for signal in [libc::SIGINT, libc::SIGHUP, libc::SIGTERM, libc::SIGQUIT] {
        // SAFETY: setting a signal to be ignored is always safe.
        unsafe { libc::signal(signal, libc::SIG_IGN) };
    }
    let mut stdin = std::io::stdin();
    let mut heard = Vec::new();
    let mut buf = [0u8; 256];
    loop {
        match stdin.read(&mut buf) {
            Ok(0) => break,
            // A few short lines; more is not kept.
            Ok(n) if heard.len() < 4096 => heard.extend_from_slice(&buf[..n]),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    match told(&String::from_utf8_lossy(&heard)) {
        // Nothing of the program's is mounted: a mount at the place is another's.
        Some(None) => return ExitCode::SUCCESS,
        Some(Some(own)) => {
            let _ = OWN.set(own);
        }
        // Killed before it knew: whatever is at the place is taken off, as before.
        None => {}
    }
    if !own_is_mounted(point) {
        return ExitCode::SUCCESS;
    }
    say!(
        "watch: the mount program is gone and left {} mounted; unmounting",
        point.display()
    );
    for _ in 0..30 {
        if unmount(point) {
            say!("watch: unmounted {}", point.display());
            return ExitCode::SUCCESS;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    say!("watch: could not unmount {}", point.display());
    ExitCode::FAILURE
}
