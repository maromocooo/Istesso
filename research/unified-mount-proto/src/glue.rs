//! The glue between the shell's cd switch (`scripts/dev/cd-switch.zsh`) and the mount: a
//! folder of another machine is mounted the first time a shell goes into it, and unmounted
//! when nothing on this Mac uses it any more. See ADR 0007 ("ish とマウント").
//!
//! `ensure` starts `mount` on its own (a session of its own, so that no terminal's signal
//! reaches it, its messages in a log) and waits until the folder is mounted and its contents
//! are answered. `release` is a plain unmount: the system refuses it while any program has a
//! file or a folder in the mount open, so nothing is pulled from under a program that reads.
//! Both take a lock per mount point, so that two shells that go into the same folder at the
//! same moment start one `mount`, not two.

use std::fs::{File, OpenOptions};
use std::io::Read as _;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

/// What a machine folder may be called, as said to the person.
pub const MACHINE_FOLDER_RULE: &str =
    "a machine folder is 1 to 64 of A-Z, a-z, 0-9, ., _ and -, not starting or ending with . and not starting with -";

/// How long `ensure` waits for a mount unless told otherwise. Longer than the viewer's own
/// limits for reaching the device (3 s) and for its answer (5 s), with room for `mount_nfs`.
const WAIT: Duration = Duration::from_secs(20);

/// How long `ensure` waits for the `mount` of an earlier mount at the same place (and its
/// watcher) to end before it says so and gives up: the `mount` looks every 2 s whether its
/// folder was unmounted, and then waits up to 5 s for its connection to close.
const EARLIER_WITHIN: Duration = Duration::from_secs(8);

/// How often `ensure` looks whether the folder is mounted yet.
const LOOK_EVERY: Duration = Duration::from_millis(20);

/// How often the `mount` that `ensure` starts tries to take the folder off, unless told
/// otherwise: a mount that nothing uses goes within this time, also when the shell that
/// went into it is gone (its window was closed, say).
const UNUSED: Duration = Duration::from_secs(30);

/// Whether `name` may be the folder below the base that a machine's folders are mounted in:
/// one plain folder name, with nothing that a shell, `mount_nfs` or a path would read as
/// something else.
pub fn valid_machine_folder(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        && !name.starts_with('.')
        && !name.ends_with('.')
        && !name.starts_with('-')
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnsureOptions {
    pub device: String,
    pub folder: String,
    pub base: PathBuf,
    pub machine: String,
    pub address: Option<String>,
    pub state: Option<PathBuf>,
    pub wait: Duration,
    pub unused: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseOptions {
    pub folder: String,
    pub base: PathBuf,
    pub machine: String,
    pub state: Option<PathBuf>,
}

/// The `--flag value` pairs after the words, each flag at most once and one of `allowed`.
fn flags<'a>(rest: &'a [String], allowed: &[&str]) -> Option<Vec<(&'a str, &'a str)>> {
    let mut seen: Vec<(&str, &str)> = Vec::new();
    let mut rest = rest.iter();
    while let Some(flag) = rest.next() {
        let value = rest.next()?;
        if !allowed.contains(&flag.as_str()) || seen.iter().any(|(f, _)| f == flag) {
            return None;
        }
        seen.push((flag, value));
    }
    Some(seen)
}

fn flag<'a>(flags: &[(&str, &'a str)], name: &str) -> Option<&'a str> {
    flags.iter().find(|(f, _)| *f == name).map(|(_, v)| *v)
}

/// `ensure <device> <folder-id> --base DIR --machine-folder NAME [--address A] [--state DIR]
/// [--wait-ms N]`, the words after `ensure`.
pub fn parse_ensure(args: &[String]) -> Option<EnsureOptions> {
    let (device, folder) = (args.first()?, args.get(1)?);
    if device.starts_with("--") || folder.starts_with("--") {
        return None;
    }
    let given = flags(
        &args[2..],
        &[
            "--base",
            "--machine-folder",
            "--address",
            "--state",
            "--wait-ms",
            "--unmount-when-unused",
        ],
    )?;
    Some(EnsureOptions {
        device: device.clone(),
        folder: folder.clone(),
        base: PathBuf::from(flag(&given, "--base")?),
        machine: flag(&given, "--machine-folder")?.to_string(),
        address: flag(&given, "--address").map(str::to_string),
        state: flag(&given, "--state").map(PathBuf::from),
        wait: match flag(&given, "--wait-ms") {
            Some(ms) => Duration::from_millis(ms.parse().ok().filter(|ms| *ms > 0)?),
            None => WAIT,
        },
        unused: match flag(&given, "--unmount-when-unused") {
            Some(secs) => Duration::from_secs(secs.parse().ok().filter(|secs| *secs > 0)?),
            None => UNUSED,
        },
    })
}

/// `release <folder-id> --base DIR --machine-folder NAME [--state DIR]`, the words after
/// `release`.
pub fn parse_release(args: &[String]) -> Option<ReleaseOptions> {
    let folder = args.first()?;
    if folder.starts_with("--") {
        return None;
    }
    let given = flags(&args[1..], &["--base", "--machine-folder", "--state"])?;
    Some(ReleaseOptions {
        folder: folder.clone(),
        base: PathBuf::from(flag(&given, "--base")?),
        machine: flag(&given, "--machine-folder")?.to_string(),
        state: flag(&given, "--state").map(PathBuf::from),
    })
}

/// The name of a mount's files in the state folder: a short hash of the real path of the
/// base (8 hex digits), `-`, the machine folder, `=`, the folder id. `=` is in neither name,
/// so two mounts below one base never share their files; two bases share them only when
/// their hashes are the same.
fn state_name(root: &Path, machine: &str, folder: &str) -> String {
    // FNV-1a, 64 bits: the same in every build, unlike the standard hasher's.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in root.as_os_str().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!(
        "{:08x}-{machine}={folder}",
        (hash >> 32) as u32 ^ hash as u32
    )
}

/// The folder for the locks, the process numbers and the logs: the given one, or one in this
/// user's temporary folder (private on macOS, emptied when the Mac starts). Made private
/// (0700), and refused when it is not.
fn state_dir(given: Option<&Path>) -> Result<PathBuf, String> {
    let dir = match given {
        Some(dir) => dir.to_path_buf(),
        None => std::env::temp_dir().join("istesso-mounts"),
    };
    crate::private_dir(&dir)?;
    Ok(dir)
}

/// The mount point, when the machine's folder exists: its real path (as the mount table has
/// it) and the folder id. The mount point itself is not looked at: a mount whose server is
/// gone may not answer.
fn existing_point(base: &Path, machine: &str, folder: &str) -> Option<PathBuf> {
    std::fs::canonicalize(base.join(machine))
        .ok()
        .map(|machine| machine.join(folder))
}

/// A lock on one mount point, held while it lives.
struct Lock(#[allow(dead_code)] File);

fn lock(state: &Path, name: &str) -> Result<Lock, String> {
    let path = state.join(format!("{name}.lock"));
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    loop {
        // SAFETY: flock on a descriptor this function owns.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(Lock(file));
        }
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::Interrupted {
            return Err(format!("cannot lock {}: {err}", path.display()));
        }
    }
}

/// Whether process `pid` runs, and runs this program (a `mount` that has not ended yet, not
/// another program that was given the same number later).
fn runs_this_program(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 only asks whether the process exists.
    if unsafe { libc::kill(pid, 0) } != 0 {
        return false;
    }
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is as large as the call is told.
    let len = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if len <= 0 {
        return false;
    }
    let theirs = Path::new(std::ffi::OsStr::from_bytes(&buf[..len as usize]));
    std::env::current_exe()
        .ok()
        .and_then(|me| std::fs::canonicalize(me).ok())
        .is_some_and(|me| std::fs::canonicalize(theirs).is_ok_and(|them| them == me))
}

/// The process number in a pid file.
fn read_pid(pid_file: &Path) -> Option<i32> {
    std::fs::read_to_string(pid_file)
        .ok()
        .and_then(|text| text.trim().parse::<i32>().ok())
}

/// Waits, a short while, for these processes (an earlier `mount` at this place and its
/// watcher) to end. False when one of them still runs this program after that: a new mount
/// at the same place must not start then.
fn wait_for_earlier(pids: &[i32], within: Duration) -> bool {
    let until = Instant::now() + within;
    loop {
        if !pids.iter().any(|pid| runs_this_program(*pid)) {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// What a log of `mount` says so far.
#[derive(Debug, Default, PartialEq, Eq)]
struct Record {
    /// The folder is mounted and its contents are answered.
    ready: bool,
    /// After that, the connection to the device was lost: every read fails.
    lost: bool,
    /// The mount's identity in the kernel's table.
    fsid: Option<crate::Fsid>,
    /// The process numbers of the `mount` and of its watcher.
    process: Option<i32>,
    watcher: Option<i32>,
    /// The reasons it gave for failing before it was mounted.
    reasons: Vec<String>,
}

fn read_log(text: &str) -> Record {
    let mut record = Record::default();
    for line in text.lines() {
        // `mount` says this once the gate is open, after mount_nfs succeeded.
        if line.starts_with("mounted ") {
            record.ready = true;
        } else if let Some(fsid) = line.strip_prefix("fsid: ") {
            record.fsid = fsid.parse().ok();
        } else if let Some(pid) = line.strip_prefix("watcher: process ") {
            record.watcher = pid.parse().ok();
        } else if let Some(pid) = line.strip_prefix("process: ") {
            record.process = pid.parse().ok();
        } else if record.ready && line.starts_with("mount: lost the connection") {
            record.lost = true;
        } else if let Some(reason) = line.strip_prefix("mount: ") {
            if !record.ready {
                record.reasons.push(reason.to_string());
            }
        }
    }
    record
}

fn log_text(path: &Path) -> String {
    let mut text = String::new();
    if let Ok(file) = File::open(path) {
        // The log of one run is short; more is not read.
        let _ = file.take(64 * 1024).read_to_string(&mut text);
    }
    text
}

pub fn run_ensure(options: EnsureOptions) -> ExitCode {
    match ensure(&options) {
        Ok(point) => {
            println!("{}", point.display());
            ExitCode::SUCCESS
        }
        Err(Refused::Usage(why)) => {
            say!("ensure: {why}");
            ExitCode::from(2)
        }
        Err(Refused::Failed(reasons)) => {
            for reason in reasons {
                say!(
                    "cannot show {}/{} here: {reason}",
                    options.machine,
                    options.folder
                );
            }
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug)]
enum Refused {
    Usage(String),
    Failed(Vec<String>),
}

fn failed(why: impl Into<String>) -> Refused {
    Refused::Failed(vec![why.into()])
}

fn ensure(options: &EnsureOptions) -> Result<PathBuf, Refused> {
    if !istesso_link::mount::valid_folder_id(&options.folder) {
        return Err(Refused::Usage(
            "a folder id is 1 to 32 of a-z, 0-9 and -, not starting with -".into(),
        ));
    }
    if !valid_machine_folder(&options.machine) {
        return Err(Refused::Usage(MACHINE_FOLDER_RULE.into()));
    }
    let state = state_dir(options.state.as_deref()).map_err(failed)?;
    // The base first: the names of the state files come from its real path.
    crate::private_dir(&options.base).map_err(failed)?;
    let root = std::fs::canonicalize(&options.base)
        .map_err(|e| failed(format!("{}: {e}", options.base.display())))?;
    let name = state_name(&root, &options.machine, &options.folder);
    let _lock = lock(&state, &name).map_err(failed)?;
    let mounted_at = || {
        existing_point(&options.base, &options.machine, &options.folder)
            .filter(|point| crate::is_mounted(point))
    };
    let pid_file = state.join(format!("{name}.pid"));
    let log_path = state.join(format!("{name}.log"));

    // The mount of this flow that is there, when its `mount` still runs: used as it is, or,
    // when its connection is lost, taken off (when nothing uses it) and made again. Only
    // that mount, known by its identity in the table: never another one at the same place.
    let record = read_log(&log_text(&log_path));
    let earlier = read_pid(&pid_file);
    let running = earlier.filter(|pid| runs_this_program(*pid));
    if earlier.is_some() && running.is_none() {
        // That `mount` is gone: its number may be given to another program.
        let _ = std::fs::remove_file(&pid_file);
    }
    if let (Some(pid), Some(point)) = (
        running,
        existing_point(&options.base, &options.machine, &options.folder),
    ) {
        let found = crate::mounted_fsids(&point);
        match record.fsid {
            Some(own) if found.as_ref().map_or(true, |found| found.contains(&own)) => {
                if !record.lost {
                    return Ok(point);
                }
                if !matches!(&found, Ok(found) if found.last() == Some(&own)) {
                    return Err(failed(
                        "the connection to the machine was lost, and another mount lies on this one; take that off first",
                    ));
                }
                match plain_unmount(&point) {
                    Ok(()) => {
                        // It ends at once instead of at its next look.
                        // SAFETY: a signal to the process this flow started, by its number,
                        // which runs this program.
                        unsafe { libc::kill(pid, libc::SIGTERM) };
                    }
                    Err(e) if e.raw_os_error() == Some(libc::EBUSY) => {
                        return Err(failed(
                            "the connection to the machine was lost, and the folder is still in use on this Mac; close what uses it and go in again",
                        ))
                    }
                    Err(e) => {
                        return Err(failed(format!("cannot unmount {}: {e}", point.display())))
                    }
                }
            }
            // Mounted and answering, from a `mount` that has not said which mount is its
            // own.
            None if record.ready && !record.lost && crate::is_mounted(&point) => return Ok(point),
            _ => {}
        }
    }
    // No mount of this flow is there to be used. An earlier `mount` at this place, or its
    // watcher, that still runs would take a new mount here for its own: it has to end first.
    let pids: Vec<i32> = running
        .into_iter()
        .chain(record.process)
        .chain(record.watcher)
        .collect();
    if !wait_for_earlier(&pids, EARLIER_WITHIN) {
        return Err(failed(
            "an earlier mount of this folder is still ending; try again in a moment",
        ));
    }
    let _ = std::fs::remove_file(&pid_file);

    // A new log for this run: an earlier `mount` that has not ended yet writes on in the old
    // one, not in this one.
    let _ = std::fs::remove_file(&log_path);
    let log = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&log_path)
        .map_err(|e| failed(format!("cannot write {}: {e}", log_path.display())))?;
    let exe = std::env::current_exe().map_err(|e| failed(e.to_string()))?;
    let mut command = Command::new(exe);
    command
        .arg("mount")
        .arg(&options.device)
        .arg(&options.folder)
        .arg("--base")
        .arg(&options.base)
        .arg("--machine-folder")
        .arg(&options.machine)
        .arg("--unmount-when-unused")
        .arg(options.unused.as_secs().to_string());
    if let Some(address) = &options.address {
        command.arg("--address").arg(address);
    }
    // Not in the folder of whoever asked: a shell that went into the empty folder where the
    // mount will be would make the program stand in its own mount, and the mount would
    // always be in use.
    command
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is safe between fork and exec.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = command
        .spawn()
        .map_err(|e| failed(format!("cannot start the mount: {e}")))?;
    let pid = child.id() as i32;
    let _ = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&pid_file)
        .and_then(|mut file| std::io::Write::write_all(&mut file, format!("{pid}\n").as_bytes()));

    let until = Instant::now() + options.wait;
    // A run that ends without a mount leaves no files but its lock.
    let forget = || {
        let _ = std::fs::remove_file(&pid_file);
        let _ = std::fs::remove_file(&log_path);
    };
    loop {
        let logged = read_log(&log_text(&log_path));
        if logged.ready && !logged.lost {
            if let Some(point) = mounted_at() {
                // The `mount` goes on by itself; it is not waited for.
                return Ok(point);
            }
        }
        let lost = logged.lost;
        match child.try_wait() {
            Ok(Some(status)) => {
                let logged = read_log(&log_text(&log_path));
                forget();
                if logged.ready {
                    return Err(failed(format!("the mount ended at once ({status})")));
                }
                let mut reasons = logged.reasons;
                if reasons.is_empty() {
                    reasons.push(format!("the mount ended ({status})"));
                }
                return Err(Refused::Failed(reasons));
            }
            Ok(None) => {}
            Err(e) => return Err(failed(format!("cannot follow the mount: {e}"))),
        }
        if lost || Instant::now() >= until {
            // Its own handling of SIGTERM takes off whatever it mounted before it ends.
            // SAFETY: a signal to the process this function started, by its number.
            unsafe { libc::kill(pid, libc::SIGTERM) };
            let stop = Instant::now() + Duration::from_secs(5);
            while matches!(child.try_wait(), Ok(None)) && Instant::now() < stop {
                std::thread::sleep(Duration::from_millis(50));
            }
            let logged = read_log(&log_text(&log_path));
            // The pid file stays while it runs: the next `ensure` waits for it.
            if matches!(child.try_wait(), Ok(Some(_))) {
                forget();
            }
            if lost {
                return Err(failed("the connection to the machine was lost at once"));
            }
            let mut reasons = if logged.ready {
                Vec::new()
            } else {
                logged.reasons
            };
            reasons.push(format!(
                "not mounted after {:.0} s; stopped trying",
                options.wait.as_secs_f64()
            ));
            return Err(Refused::Failed(reasons));
        }
        std::thread::sleep(LOOK_EVERY);
    }
}

/// What `release` found or did.
#[derive(Debug, PartialEq, Eq)]
pub enum Released {
    /// Nothing was mounted there.
    NotMounted,
    /// It was mounted, and is not any more.
    Unmounted,
    /// A program on this Mac uses it: it stays mounted.
    Busy,
}

pub fn run_release(options: ReleaseOptions) -> ExitCode {
    match release(&options) {
        Ok(Released::NotMounted | Released::Unmounted) => ExitCode::SUCCESS,
        Ok(Released::Busy) => {
            say!(
                "{}/{} is in use on this Mac; it stays mounted",
                options.machine,
                options.folder
            );
            ExitCode::from(3)
        }
        Err(why) => {
            say!("release: {why}");
            ExitCode::FAILURE
        }
    }
}

pub fn release(options: &ReleaseOptions) -> Result<Released, String> {
    if !istesso_link::mount::valid_folder_id(&options.folder) {
        return Err("a folder id is 1 to 32 of a-z, 0-9 and -, not starting with -".into());
    }
    if !valid_machine_folder(&options.machine) {
        return Err(MACHINE_FOLDER_RULE.into());
    }
    let state = state_dir(options.state.as_deref())?;
    let Ok(root) = std::fs::canonicalize(&options.base) else {
        return Ok(Released::NotMounted);
    };
    let name = state_name(&root, &options.machine, &options.folder);
    let _lock = lock(&state, &name)?;
    let Some(point) = existing_point(&options.base, &options.machine, &options.folder) else {
        return Ok(Released::NotMounted);
    };
    let on_top = match crate::mounted_fsids(&point) {
        Ok(found) if found.is_empty() => return Ok(Released::NotMounted),
        Ok(found) => found.last().copied(),
        Err(()) => None,
    };
    match plain_unmount(&point) {
        Ok(()) => {
            // The `mount` of the mount just taken off ends at once instead of at its next
            // look, when it is this flow's and still runs.
            let record = read_log(&log_text(&state.join(format!("{name}.log"))));
            let pid_file = state.join(format!("{name}.pid"));
            if let (Some(own), Some(pid)) = (record.fsid, read_pid(&pid_file)) {
                if on_top == Some(own) && runs_this_program(pid) {
                    // SAFETY: a signal to the process this flow started, by its number, which
                    // runs this program; its mount is gone, so it only ends.
                    unsafe { libc::kill(pid, libc::SIGTERM) };
                    // Its number is in the log too, for an `ensure` that comes before it ended.
                    let _ = std::fs::remove_file(&pid_file);
                }
            }
            Ok(Released::Unmounted)
        }
        Err(e) if e.raw_os_error() == Some(libc::EBUSY) => Ok(Released::Busy),
        Err(e) => Err(format!("cannot unmount {}: {e}", point.display())),
    }
}

/// unmount(2) without force: refused (EBUSY) while anything in the mount is open.
pub(crate) fn plain_unmount(point: &Path) -> std::io::Result<()> {
    let path = std::ffi::CString::new(point.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    // SAFETY: a NUL-terminated path; flags 0 is a plain unmount.
    if unsafe { libc::unmount(path.as_ptr(), 0) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split(' ').map(str::to_string).collect()
    }

    /// A folder of this test's own below the temporary folder, removed by the test.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ish-mount-glue-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).unwrap();
        dir
    }

    #[test]
    fn the_watcher_takes_the_last_thing_it_was_told() {
        use crate::{told, Fsid};
        assert_eq!(told(""), None);
        assert_eq!(told("none\n"), Some(None));
        assert_eq!(told("fsid 12 -7\n"), Some(Some(Fsid([12, -7]))));
        assert_eq!(told("fsid 12 -7\nnone\n"), Some(None));
        // A line it does not know is passed over.
        assert_eq!(told("fsid twelve\nfsid 1 2 3\n"), None);
        assert_eq!("3 4".parse::<Fsid>(), Ok(Fsid([3, 4])));
        assert_eq!(Fsid([3, 4]).to_string(), "3 4");
    }

    #[test]
    fn the_mount_table_has_the_root_and_not_a_plain_folder() {
        let dir = scratch("table");
        assert_eq!(crate::mounted_fsids(&dir), Ok(vec![]));
        assert_eq!(crate::mounted_fsids(Path::new("/")).map(|f| f.len()), Ok(1));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn machine_folders_are_one_plain_name() {
        for good in ["ubuntu", "Mac-mini", "host_2", "a", "a.b", &"x".repeat(64)] {
            assert!(valid_machine_folder(good), "{good}");
        }
        for bad in [
            "",
            ".",
            "..",
            ".hidden",
            "trailing.",
            "-flag",
            "a/b",
            "a b",
            "a=b",
            "naïve",
            "a\0b",
            "~",
            "$HOME",
            &"x".repeat(65),
        ] {
            assert!(!valid_machine_folder(bad), "{bad:?}");
        }
    }

    #[test]
    fn ensure_takes_its_words_and_flags_once_each() {
        let parsed = parse_ensure(&words(
            "proto-host proj --base /b/Istesso --machine-folder ubuntu --address 192.0.2.1:26910 --state /s --wait-ms 1500",
        ))
        .unwrap();
        assert_eq!(
            parsed,
            EnsureOptions {
                device: "proto-host".into(),
                folder: "proj".into(),
                base: "/b/Istesso".into(),
                machine: "ubuntu".into(),
                address: Some("192.0.2.1:26910".into()), // check-private: ok (a documentation address, RFC 5737)
                state: Some("/s".into()),
                wait: Duration::from_millis(1500),
                unused: UNUSED,
            }
        );
        let often = parse_ensure(&words(
            "d f --base b --machine-folder m --unmount-when-unused 2",
        ))
        .unwrap();
        assert_eq!(often.unused, Duration::from_secs(2));
        let plain = parse_ensure(&words("d f --machine-folder m --base b")).unwrap();
        assert_eq!(plain.wait, WAIT);
        assert_eq!(plain.address, None);
        for bad in [
            "",
            "d",
            "d f",
            "d f --base b",
            "d f --machine-folder m",
            "d f --base b --machine-folder m --base c",
            "d f --base b --machine-folder m --hold-before-mount-ms 1",
            "d f --base b --machine-folder m --wait-ms",
            "d f --base b --machine-folder m --wait-ms 0",
            "d f --base b --machine-folder m --wait-ms soon",
            "d f --base b --machine-folder m --unmount-when-unused 0",
            "d f --base b --machine-folder m --unmount-when-unused",
            "--base b f --machine-folder m",
        ] {
            assert_eq!(parse_ensure(&words(bad)), None, "{bad}");
        }
    }

    #[test]
    fn release_takes_the_folder_and_where_it_is() {
        assert_eq!(
            parse_release(&words("proj --base /b --machine-folder ubuntu")).unwrap(),
            ReleaseOptions {
                folder: "proj".into(),
                base: "/b".into(),
                machine: "ubuntu".into(),
                state: None,
            }
        );
        for bad in [
            "",
            "proj",
            "proj --base /b",
            "--base /b --machine-folder m",
            "proj --base /b --machine-folder m --address a",
        ] {
            assert_eq!(parse_release(&words(bad)), None, "{bad}");
        }
    }

    #[test]
    fn two_mounts_never_share_their_files() {
        let root = Path::new("/Volumes/one/Istesso");
        // A folder id has no `=`; a machine folder has no `=`.
        assert_ne!(state_name(root, "a-b", "c"), state_name(root, "a", "b-c"));
        let name = state_name(root, "ubuntu", "proj");
        assert!(name.ends_with("-ubuntu=proj"), "{name}");
        assert_eq!(name.len(), 8 + "-ubuntu=proj".len());
        // The same in every run, and another one for another root.
        assert_eq!(name, state_name(root, "ubuntu", "proj"));
        assert_ne!(
            name,
            state_name(Path::new("/Volumes/test/Istesso"), "ubuntu", "proj")
        );
    }

    #[test]
    fn a_mount_point_exists_only_below_a_machine_folder_that_exists() {
        let dir = scratch("point");
        let base = dir.join("Istesso");
        assert_eq!(existing_point(&base, "ubuntu", "proj"), None);
        std::fs::create_dir_all(base.join("ubuntu")).unwrap();
        let point = existing_point(&base, "ubuntu", "proj").unwrap();
        // The real path, as the mount table has it (the temporary folder is behind a link
        // on macOS).
        assert_eq!(
            point,
            std::fs::canonicalize(base.join("ubuntu"))
                .unwrap()
                .join("proj")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_log_says_ready_only_once_the_contents_are_answered() {
        assert_eq!(read_log(""), Record::default());
        let started = read_log("process: 11\nwatcher: process 12\n");
        assert!(!started.ready);
        assert_eq!(started.process, Some(11));
        assert_eq!(started.watcher, Some(12));
        assert!(started.reasons.is_empty());
        let mounted = read_log(
            "watcher: process 12\nfsid: 50331660 30\nmounted proj of host read-only at /x after 0.1 s\n",
        );
        assert!(mounted.ready && !mounted.lost);
        assert_eq!(mounted.fsid, Some(crate::Fsid([50331660, 30])));
        assert_eq!(
            read_log("mount: cannot reach host: no answer\nrefused: 0 later\n").reasons,
            vec!["cannot reach host: no answer".to_string()]
        );
        // A line that only contains the words is not the line.
        assert_eq!(read_log("test: not mounted yet\n"), Record::default());
        // Lost after it was mounted: every read fails from then on.
        let lost = read_log("mounted p of h read-only at /x after 0.1 s\nmount: lost the connection to h (closed); reads fail from now on, until this is stopped\n");
        assert!(lost.ready && lost.lost);
        assert!(lost.reasons.is_empty());
        // The same words before the mount are only a reason.
        let before = read_log("mount: lost the connection to h\n");
        assert!(!before.lost);
        assert_eq!(before.reasons, vec!["lost the connection to h".to_string()]);
    }

    #[test]
    fn release_of_a_folder_that_is_not_mounted_does_nothing() {
        let dir = scratch("release");
        let base = dir.join("Istesso");
        let state = dir.join("state");
        let options = ReleaseOptions {
            folder: "proj".into(),
            base: base.clone(),
            machine: "ubuntu".into(),
            state: Some(state.clone()),
        };
        assert_eq!(release(&options), Ok(Released::NotMounted));
        std::fs::create_dir_all(base.join("ubuntu/proj")).unwrap();
        assert_eq!(release(&options), Ok(Released::NotMounted));
        // The folder is still there, and so is what is in it.
        std::fs::write(base.join("ubuntu/proj/kept"), b"x").unwrap();
        assert_eq!(release(&options), Ok(Released::NotMounted));
        assert!(base.join("ubuntu/proj/kept").exists());
        // Bad names are refused before anything is looked at.
        let bad = ReleaseOptions {
            folder: "../proj".into(),
            ..options.clone()
        };
        assert!(release(&bad).is_err());
        let bad = ReleaseOptions {
            machine: "..".into(),
            ..options
        };
        assert!(release(&bad).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_plain_unmount_of_a_folder_that_is_no_mount_is_not_taken_for_busy() {
        let dir = scratch("plain");
        let err = plain_unmount(&dir).unwrap_err();
        assert_ne!(err.raw_os_error(), Some(libc::EBUSY));
        assert!(!crate::is_mounted(&dir));
        assert!(crate::is_mounted(Path::new("/")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_state_folder_must_be_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("state");
        let state = dir.join("state");
        assert_eq!(state_dir(Some(&state)).unwrap(), state);
        assert_eq!(
            std::fs::metadata(&state).unwrap().permissions().mode() & 0o777,
            0o700
        );
        std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(state_dir(Some(&state)).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_earlier_mount_is_waited_for_only_while_it_runs_this_program() {
        assert!(!runs_this_program(0));
        assert!(!runs_this_program(-1));
        // This test runs the test program, not the prototype; launchd is no mount.
        assert!(runs_this_program(std::process::id() as i32));
        assert!(!runs_this_program(1));
        let dir = scratch("earlier");
        let pid_file = dir.join("x.pid");
        std::fs::write(&pid_file, "1\n").unwrap();
        assert_eq!(read_pid(&pid_file), Some(1));
        assert_eq!(read_pid(&dir.join("none.pid")), None);
        let began = Instant::now();
        assert!(wait_for_earlier(&[1, 0], Duration::from_secs(5)));
        assert!(began.elapsed() < Duration::from_secs(1));
        // One that runs this program is waited for, and then it is said so.
        let began = Instant::now();
        assert!(!wait_for_earlier(
            &[std::process::id() as i32],
            Duration::from_millis(200)
        ));
        assert!(began.elapsed() >= Duration::from_millis(200));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
