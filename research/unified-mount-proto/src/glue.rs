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

/// How long `ensure` waits for the `mount` of an earlier mount at the same place to end. It
/// looks every 2 s whether its folder was unmounted from outside.
const EARLIER_WITHIN: Duration = Duration::from_secs(4);

/// How often `ensure` looks whether the folder is mounted yet.
const LOOK_EVERY: Duration = Duration::from_millis(20);

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

/// The name of a mount's files in the state folder. `=` is in neither a machine folder's
/// name nor a folder id, so two mounts never share one.
fn state_name(machine: &str, folder: &str) -> String {
    format!("{machine}={folder}")
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

/// Waits, a short while, for the `mount` of an earlier mount at this place to end: it ends
/// within 2 s of its folder being unmounted from outside. A new mount at the same place
/// before that would be taken by it for its own.
fn wait_for_earlier(pid_file: &Path) {
    let Some(pid) = std::fs::read_to_string(pid_file)
        .ok()
        .and_then(|text| text.trim().parse::<i32>().ok())
    else {
        return;
    };
    let until = Instant::now() + EARLIER_WITHIN;
    while runs_this_program(pid) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// What a log of `mount` says so far.
#[derive(Debug, PartialEq, Eq)]
enum Logged {
    /// The folder is mounted and its contents are answered.
    Ready,
    /// It was mounted, and the connection to the device is lost since: every read fails.
    Lost,
    /// Not yet, and these are the reasons it gave for failing, if any.
    NotYet(Vec<String>),
}

fn read_log(text: &str) -> Logged {
    let mut reasons = Vec::new();
    let mut ready = false;
    for line in text.lines() {
        // `mount` says this once the gate is open, after mount_nfs succeeded.
        if line.starts_with("mounted ") {
            ready = true;
        } else if ready && line.starts_with("mount: lost the connection") {
            return Logged::Lost;
        } else if let Some(reason) = line.strip_prefix("mount: ") {
            reasons.push(reason.to_string());
        }
    }
    if ready {
        Logged::Ready
    } else {
        Logged::NotYet(reasons)
    }
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
    let name = state_name(&options.machine, &options.folder);
    let _lock = lock(&state, &name).map_err(failed)?;
    let mounted_at = || {
        existing_point(&options.base, &options.machine, &options.folder)
            .filter(|point| crate::is_mounted(point))
    };
    let pid_file = state.join(format!("{name}.pid"));
    let log_path = state.join(format!("{name}.log"));
    if let Some(point) = mounted_at() {
        if read_log(&log_text(&log_path)) != Logged::Lost {
            return Ok(point);
        }
        // Mounted, but every read fails since the connection was lost: mounted again, when
        // nothing uses it.
        match plain_unmount(&point) {
            Ok(()) => {}
            Err(e) if e.raw_os_error() == Some(libc::EBUSY) => {
                return Err(failed(
                    "the connection to the machine was lost, and the folder is still in use on this Mac; close what uses it and go in again",
                ))
            }
            Err(e) => return Err(failed(format!("cannot unmount {}: {e}", point.display()))),
        }
    }
    wait_for_earlier(&pid_file);

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
        .arg(&options.machine);
    if let Some(address) = &options.address {
        command.arg("--address").arg(address);
    }
    command
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
    loop {
        let logged = read_log(&log_text(&log_path));
        if logged == Logged::Ready {
            if let Some(point) = mounted_at() {
                // The `mount` goes on by itself; it is not waited for.
                return Ok(point);
            }
        }
        let lost = logged == Logged::Lost;
        match child.try_wait() {
            Ok(Some(status)) => {
                let _ = std::fs::remove_file(&pid_file);
                let Logged::NotYet(mut reasons) = read_log(&log_text(&log_path)) else {
                    return Err(failed(format!("the mount ended at once ({status})")));
                };
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
            if lost {
                return Err(failed("the connection to the machine was lost at once"));
            }
            let mut reasons = match read_log(&log_text(&log_path)) {
                Logged::NotYet(reasons) => reasons,
                Logged::Ready | Logged::Lost => Vec::new(),
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
    let _lock = lock(&state, &state_name(&options.machine, &options.folder))?;
    let Some(point) = existing_point(&options.base, &options.machine, &options.folder) else {
        return Ok(Released::NotMounted);
    };
    if !crate::is_mounted(&point) {
        return Ok(Released::NotMounted);
    }
    match plain_unmount(&point) {
        Ok(()) => Ok(Released::Unmounted),
        Err(e) if e.raw_os_error() == Some(libc::EBUSY) => Ok(Released::Busy),
        Err(e) => Err(format!("cannot unmount {}: {e}", point.display())),
    }
}

/// unmount(2) without force: refused (EBUSY) while anything in the mount is open.
fn plain_unmount(point: &Path) -> std::io::Result<()> {
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
            }
        );
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
        // A folder id has no `=` and no `.`; a machine folder has no `=`.
        assert_ne!(state_name("a-b", "c"), state_name("a", "b-c"));
        assert_eq!(state_name("ubuntu", "proj"), "ubuntu=proj");
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
        assert_eq!(read_log(""), Logged::NotYet(vec![]));
        assert_eq!(read_log("watcher: process 12\n"), Logged::NotYet(vec![]));
        assert_eq!(
            read_log("watcher: process 12\nmounted proj of host read-only at /x after 0.1 s\n"),
            Logged::Ready
        );
        assert_eq!(
            read_log("mount: cannot reach host: no answer\nrefused: 0 later\n"),
            Logged::NotYet(vec!["cannot reach host: no answer".into()])
        );
        // A line that only contains the words is not the line.
        assert_eq!(read_log("test: not mounted yet\n"), Logged::NotYet(vec![]));
        // Lost after it was mounted: every read fails from then on.
        assert_eq!(
            read_log("mounted p of h read-only at /x after 0.1 s\nmount: lost the connection to h (closed); reads fail from now on, until this is stopped\n"),
            Logged::Lost
        );
        // The same words before the mount are only a reason.
        assert_eq!(
            read_log("mount: lost the connection to h\n"),
            Logged::NotYet(vec!["lost the connection to h".into()])
        );
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
        let began = Instant::now();
        wait_for_earlier(&pid_file);
        assert!(began.elapsed() < Duration::from_secs(1));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
