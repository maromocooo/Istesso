//! The host's end of a shell: the user's login shell on a new
//! pseudo-terminal, and the conversation that carries it.
//!
//! Four threads serve one shell:
//!
//! * the conversation's own thread waits for the shell to end, for the
//!   device to go, or for the device to lose its right (looked up again
//!   every second), and then ends everything;
//! * one passes what the shell prints to the device;
//! * one passes what the device sends (keys, a new size, "still here") to
//!   the shell;
//! * one waits for the shell's process to end, and collects it.
//!
//! Nothing that passes is written down: only how many bytes and frames, and
//! how the shell ended.

use std::net::TcpStream;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use istesso_link::shell::{ShellEnd, ShellFrame, MAX_DATA};
use istesso_link::{Channel, ChannelWriter, ShellRequest};

use super::sys::{self, Account, Ready};

/// How long the device may say nothing before the shell is ended. It says
/// "still here" every ten seconds ([`super::ALIVE_EVERY`]).
pub const SILENCE: Duration = Duration::from_secs(60);
/// How often the device's right to the shell is looked up again.
pub const RECHECK: Duration = Duration::from_secs(1);
/// How long after the shell ended what it printed last may take to come
/// through, before the end is reported anyway (a program it left running
/// in the background may hold the terminal open).
const DRAIN: Duration = Duration::from_millis(300);
/// How long a shell that was told to hang up has before it is killed.
const HANG_UP_GRACE: Duration = Duration::from_secs(2);
/// How often the threads that wait on the terminal look up whether they
/// should stop.
const TICK: Duration = Duration::from_millis(100);

/// The size of a terminal, kept within what a terminal can be.
fn clamp(size: u16) -> u16 {
    size.clamp(1, 1000)
}

/// How a host starts shells.
#[derive(Debug, Clone, Default)]
pub struct ShellSettings {
    /// The program to run, instead of the user's login shell, and not as a
    /// login shell. For tests, which must not run the login files of the
    /// person who runs them.
    pub program: Option<PathBuf>,
}

/// A shell that has started and has not been handed its conversation yet.
/// Dropping it hangs the shell up.
pub struct Shell {
    child: Option<Child>,
    master: Option<OwnedFd>,
    pid: u32,
    started: Instant,
}

/// What a shell's conversation came to, for the one line about it.
#[derive(Debug, Clone)]
pub struct Served {
    pub end: Ending,
    pub lasted: Duration,
    pub bytes_in: u64,
    pub frames_in: u64,
    pub bytes_out: u64,
    pub frames_out: u64,
    pub resizes: u64,
}

/// How a shell's conversation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// The shell ended, and the device was told how.
    Shell(ShellEnd),
    /// The device went away, said nothing for too long, or sent something
    /// that is not a shell frame.
    DeviceGone,
    /// The device lost the right to have a shell here.
    Revoked,
}

impl std::fmt::Display for Ending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ending::Shell(ShellEnd::Code(code)) => write!(f, "the shell exited with status {code}"),
            Ending::Shell(ShellEnd::Signal(signal)) => {
                write!(f, "the shell was ended by signal {signal}")
            }
            Ending::Shell(_) => f.write_str("the shell was ended"),
            Ending::DeviceGone => f.write_str("the device went away"),
            Ending::Revoked => f.write_str("the device lost its right to a shell here"),
        }
    }
}

/// `TERM` from what the device asked for, when it is a plausible name.
fn terminal_type(asked: Option<&str>) -> String {
    match asked {
        Some(term)
            if !term.is_empty()
                && term.len() <= 64
                && term
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-._+".contains(&b)) =>
        {
            term.to_string()
        }
        _ => "xterm-256color".to_string(),
    }
}

/// `LANG` from what the device asked for: that locale when it is a
/// plausible name and this machine has it, a UTF-8 locale this machine has
/// otherwise. Text is UTF-8 on both ends whatever the device says.
fn language(asked: Option<&str>, has: impl Fn(&str) -> bool) -> Option<String> {
    let plausible = |name: &str| {
        !name.is_empty()
            && name.len() <= 64
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._@".contains(&b))
    };
    if let Some(asked) = asked.filter(|name| plausible(name)) {
        if has(asked) {
            return Some(asked.to_string());
        }
    }
    ["C.UTF-8", "en_US.UTF-8"]
        .into_iter()
        .find(|name| has(name))
        .map(str::to_string)
}

/// The folder the shell starts in: the project the device named, or home.
fn start_folder(project: Option<&str>, home: &Path) -> Result<PathBuf, String> {
    let folder = match project {
        None => return Ok(home.to_path_buf()),
        Some(path) if path.contains('\0') => return Err("that is not a folder name".into()),
        Some("~") => home.to_path_buf(),
        Some(path) if path.starts_with("~/") => home.join(&path[2..]),
        Some(path) if Path::new(path).is_absolute() => PathBuf::from(path),
        Some(_) => {
            return Err(
                "the project folder must be an absolute path on the host or start with ~/".into(),
            )
        }
    };
    if folder.is_dir() {
        Ok(folder)
    } else {
        Err("there is no such folder on the host".into())
    }
}

/// The environment of a new shell: made afresh, as a login does, and not
/// taken over from the agent.
fn environment(
    account: &Account,
    shell: &Path,
    request: &ShellRequest,
    inherited: impl Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    let path = if cfg!(target_os = "macos") {
        "/usr/bin:/bin:/usr/sbin:/sbin"
    } else {
        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
    };
    let mut env = vec![
        ("HOME".to_string(), account.home.display().to_string()),
        ("USER".to_string(), account.name.clone()),
        ("LOGNAME".to_string(), account.name.clone()),
        ("SHELL".to_string(), shell.display().to_string()),
        ("PATH".to_string(), path.to_string()),
        ("TERM".to_string(), terminal_type(request.term.as_deref())),
    ];
    if let Some(lang) = language(request.locale.as_deref(), sys::has_locale) {
        env.push(("LANG".to_string(), lang));
    }
    // Folders of this user's session that a login would set as well.
    let session = if cfg!(target_os = "macos") {
        "TMPDIR"
    } else {
        "XDG_RUNTIME_DIR"
    };
    if let Some(value) = inherited(session).filter(|value| Path::new(value).is_absolute()) {
        env.push((session.to_string(), value));
    }
    env
}

impl Shell {
    /// Starts the user's login shell on a new pseudo-terminal, as the user
    /// this process runs as. The error says why, in words fit for the
    /// device, and never quotes what the device sent.
    pub fn start(request: &ShellRequest, settings: &ShellSettings) -> Result<Shell, String> {
        let account = sys::account()?;
        let folder = start_folder(request.project.as_deref(), &account.home)?;
        let (program, argv0) = match &settings.program {
            Some(program) => (program.clone(), program_name(program)),
            None => (
                account.shell.clone(),
                format!("-{}", program_name(&account.shell)),
            ),
        };
        let env = environment(&account, &program, request, |name| std::env::var(name).ok());

        let mut command = Command::new(&program);
        command
            .arg0(argv0)
            .env_clear()
            .envs(env)
            .current_dir(&folder);

        let started = Instant::now();
        let (child, master) = {
            let _starting = sys::STARTING.lock().unwrap_or_else(|e| e.into_inner());
            let (master, slave) = sys::open_pty(clamp(request.cols), clamp(request.rows))
                .map_err(|e| format!("cannot open a terminal: {e}"))?;
            let copy = |fd: &OwnedFd| fd.try_clone().map(Stdio::from);
            let (stdin, stdout, stderr) = (copy(&slave), copy(&slave), copy(&slave));
            let (stdin, stdout, stderr) = match (stdin, stdout, stderr) {
                (Ok(a), Ok(b), Ok(c)) => (a, b, c),
                _ => return Err("cannot open a terminal".into()),
            };
            command.stdin(stdin).stdout(stdout).stderr(stderr);
            // SAFETY: runs in the child between fork and exec, and calls only
            // `signal`, `setsid` and `ioctl`, which are safe there. The standard
            // input is the terminal by then.
            unsafe {
                command.pre_exec(|| {
                    // Every signal does what it does by default, as after a login:
                    // an agent started in the background of a script has Control-C
                    // and Control-\ ignored, and a shell keeps for its programs
                    // what was ignored when it started. (The mask of blocked
                    // signals is cleared by the standard library already.)
                    for signal in 1..65 {
                        if signal != libc::SIGKILL && signal != libc::SIGSTOP {
                            libc::signal(signal, libc::SIG_DFL);
                        }
                    }
                    // A session of its own, with the terminal as its controlling
                    // terminal: Control-C and Control-Z reach the program in the
                    // foreground, and a hang-up reaches the shell.
                    if libc::setsid() < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let child = command
                .spawn()
                .map_err(|e| format!("cannot start the shell: {e}"))?;
            // The terminal's other end now belongs to the shell alone: once the
            // shell and what it started have closed it, reading this end ends.
            drop(slave);
            drop(command);
            (child, master)
        };
        sys::set_nonblocking(&master).map_err(|e| format!("cannot use the terminal: {e}"))?;
        Ok(Shell {
            pid: child.id(),
            child: Some(child),
            master: Some(master),
            started,
        })
    }

    /// The shell's process number.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Carries the shell over `channel` until it ends, the device goes, or
    /// `still_allowed` says no (asked every [`RECHECK`]).
    pub fn serve(
        mut self,
        channel: Channel<TcpStream>,
        mut still_allowed: impl FnMut() -> bool,
    ) -> Served {
        let counts = Arc::new(Counts::default());
        let (Some(master), Some(child)) = (self.master.take(), self.child.take()) else {
            return counts.served(Ending::DeviceGone, self.started);
        };
        let pid = self.pid;
        let hang_up = |master: OwnedFd, gone: &mpsc::Receiver<Event>| {
            // The shell and what runs in its foreground are told to hang up, as when a
            // terminal is closed; whatever is still there after a moment is killed.
            sys::signal_group(pid, libc::SIGHUP);
            drop(master);
            let deadline = Instant::now() + HANG_UP_GRACE;
            while let Some(left) = deadline.checked_duration_since(Instant::now()) {
                if let Ok(Event::Exited(_)) = gone.recv_timeout(left) {
                    return;
                }
            }
            sys::signal_group(pid, libc::SIGKILL);
        };

        let stream = channel.stream();
        let _ = stream
            .set_read_timeout(Some(SILENCE))
            .and_then(|_| stream.set_write_timeout(Some(SILENCE)));
        let (events_in, events) = mpsc::channel::<Event>();
        let Ok((reader, writer)) = channel.split() else {
            child_exit_watch(child, events_in);
            hang_up(master, &events);
            return counts.served(Ending::DeviceGone, self.started);
        };
        let writer = Arc::new(Mutex::new(writer));
        let stop = Arc::new(AtomicBool::new(false));

        let printing = master.try_clone().ok().map(|fd| {
            let (writer, stop, counts, events) = (
                writer.clone(),
                stop.clone(),
                counts.clone(),
                events_in.clone(),
            );
            std::thread::spawn(move || pass_output(fd, writer, stop, counts, events))
        });
        let typing = master.try_clone().ok().map(|fd| {
            let (writer, stop, counts, events) = (
                writer.clone(),
                stop.clone(),
                counts.clone(),
                events_in.clone(),
            );
            std::thread::spawn(move || pass_input(reader, fd, writer, stop, counts, events))
        });
        child_exit_watch(child, events_in);

        let mut exited: Option<(Instant, ShellEnd)> = None;
        let mut printed_all = printing.is_none();
        let mut checked = Instant::now();
        let ending = loop {
            match events.recv_timeout(TICK) {
                Ok(Event::PrintedAll) => printed_all = true,
                Ok(Event::Exited(end)) => exited = Some((Instant::now(), end)),
                Ok(Event::DeviceGone) => break Ending::DeviceGone,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break Ending::DeviceGone,
            }
            if let Some((at, end)) = exited {
                if printed_all || at.elapsed() >= DRAIN {
                    break Ending::Shell(end);
                }
            }
            if printing.is_none() || typing.is_none() {
                break Ending::DeviceGone;
            }
            if checked.elapsed() >= RECHECK {
                checked = Instant::now();
                if !still_allowed() {
                    break Ending::Revoked;
                }
            }
        };

        // Nothing more goes to the terminal or comes from it.
        stop.store(true, Ordering::SeqCst);
        if let Some(printing) = printing {
            let _ = printing.join();
        }
        let last = match ending {
            Ending::Shell(end) => Some(ShellFrame::Exit(end)),
            Ending::Revoked => Some(ShellFrame::Exit(ShellEnd::Revoked)),
            Ending::DeviceGone => None,
        };
        {
            let mut writer = writer.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(last) = last {
                let _ = writer.send(&last.encode());
            }
            writer.shut_down();
        }
        if let Some(typing) = typing {
            let _ = typing.join();
        }
        if !matches!(ending, Ending::Shell(_)) {
            hang_up(master, &events);
        }
        counts.served(ending, self.started)
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        // A shell that was never served (the device went before it was told that
        // the shell runs) is hung up and collected.
        if let Some(mut child) = self.child.take() {
            sys::signal_group(self.pid, libc::SIGHUP);
            drop(self.master.take());
            std::thread::spawn(move || {
                let deadline = Instant::now() + HANG_UP_GRACE;
                while Instant::now() < deadline {
                    if let Ok(Some(_)) = child.try_wait() {
                        return;
                    }
                    std::thread::sleep(TICK);
                }
                let _ = child.kill();
                let _ = child.wait();
            });
        }
    }
}

fn program_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "sh".to_string())
}

enum Event {
    /// The terminal has nothing more to print: everything holding it is gone.
    PrintedAll,
    /// The shell's process ended.
    Exited(ShellEnd),
    /// The conversation is over from the device's side.
    DeviceGone,
}

#[derive(Default)]
struct Counts {
    bytes_in: AtomicU64,
    frames_in: AtomicU64,
    bytes_out: AtomicU64,
    frames_out: AtomicU64,
    resizes: AtomicU64,
}

impl Counts {
    fn served(&self, end: Ending, started: Instant) -> Served {
        Served {
            end,
            lasted: started.elapsed(),
            bytes_in: self.bytes_in.load(Ordering::Relaxed),
            frames_in: self.frames_in.load(Ordering::Relaxed),
            bytes_out: self.bytes_out.load(Ordering::Relaxed),
            frames_out: self.frames_out.load(Ordering::Relaxed),
            resizes: self.resizes.load(Ordering::Relaxed),
        }
    }
}

/// Waits for the shell's process to end, and says how.
fn child_exit_watch(mut child: Child, events: mpsc::Sender<Event>) {
    std::thread::spawn(move || {
        let end = match child.wait() {
            Ok(status) => match (status.code(), status.signal()) {
                (Some(code), _) => ShellEnd::Code(code),
                (None, Some(signal)) => ShellEnd::Signal(signal),
                (None, None) => ShellEnd::Host,
            },
            Err(_) => ShellEnd::Host,
        };
        let _ = events.send(Event::Exited(end));
    });
}

/// What the shell prints, to the device.
fn pass_output(
    master: OwnedFd,
    writer: Arc<Mutex<ChannelWriter>>,
    stop: Arc<AtomicBool>,
    counts: Arc<Counts>,
    events: mpsc::Sender<Event>,
) {
    let mut buffer = vec![0u8; MAX_DATA];
    while !stop.load(Ordering::SeqCst) {
        match sys::wait_for(master.as_raw_fd(), Ready::Read, TICK) {
            Ok(false) => continue,
            Ok(true) => {}
            Err(_) => break,
        }
        match sys::read(master.as_raw_fd(), &mut buffer) {
            // Linux says EIO, macOS may say nothing, once no process holds the
            // terminal's other end any more.
            Ok(0) => break,
            Ok(n) => {
                let frame = ShellFrame::data(&buffer[..n]);
                let sent = writer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .send(&frame);
                if sent.is_err() {
                    let _ = events.send(Event::DeviceGone);
                    return;
                }
                counts.bytes_out.fetch_add(n as u64, Ordering::Relaxed);
                counts.frames_out.fetch_add(1, Ordering::Relaxed);
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(_) => break,
        }
    }
    let _ = events.send(Event::PrintedAll);
}

/// What the device sends, to the shell.
fn pass_input(
    mut reader: istesso_link::ChannelReader,
    master: OwnedFd,
    writer: Arc<Mutex<ChannelWriter>>,
    stop: Arc<AtomicBool>,
    counts: Arc<Counts>,
    events: mpsc::Sender<Event>,
) {
    while let Ok(message) = reader.recv() {
        let Ok(frame) = ShellFrame::decode(&message) else {
            break;
        };
        match frame {
            ShellFrame::Data(bytes) => {
                counts
                    .bytes_in
                    .fetch_add(bytes.len() as u64, Ordering::Relaxed);
                counts.frames_in.fetch_add(1, Ordering::Relaxed);
                write_to_terminal(&master, &bytes, &stop);
            }
            ShellFrame::Resize { cols, rows } => {
                counts.resizes.fetch_add(1, Ordering::Relaxed);
                let _ = sys::set_size(master.as_raw_fd(), clamp(cols), clamp(rows));
            }
            ShellFrame::Alive => {
                let answered = writer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .send(&ShellFrame::Alive.encode());
                if answered.is_err() {
                    break;
                }
            }
            // Only the host ends a shell; frames of a later version are skipped.
            ShellFrame::Exit(_) | ShellFrame::Unknown(_) => {}
        }
        if stop.load(Ordering::SeqCst) {
            return;
        }
    }
    let _ = events.send(Event::DeviceGone);
}

/// Writes all of `bytes` to the terminal, waiting while the shell does not
/// read, until told to stop. A terminal that is gone takes nothing.
fn write_to_terminal(master: &OwnedFd, mut bytes: &[u8], stop: &AtomicBool) {
    while !bytes.is_empty() && !stop.load(Ordering::SeqCst) {
        match sys::write(master.as_raw_fd(), bytes) {
            Ok(n) => bytes = &bytes[n..],
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                let _ = sys::wait_for(master.as_raw_fd(), Ready::Write, TICK);
            }
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_terminal_type_is_a_plain_name_or_the_default() {
        assert_eq!(terminal_type(Some("xterm-ghostty")), "xterm-ghostty");
        assert_eq!(terminal_type(Some("rio")), "rio");
        assert_eq!(terminal_type(None), "xterm-256color");
        for odd in ["", "a b", "x;rm -rf", "../../x", "x\u{0}", &"a".repeat(65)] {
            assert_eq!(terminal_type(Some(odd)), "xterm-256color", "{odd:?}");
        }
    }

    #[test]
    fn the_language_is_one_this_machine_has() {
        let has = |name: &str| ["ja_JP.UTF-8", "C.UTF-8"].contains(&name);
        assert_eq!(
            language(Some("ja_JP.UTF-8"), has).as_deref(),
            Some("ja_JP.UTF-8")
        );
        // macOS terminals may say just "UTF-8", which is no locale on Linux.
        assert_eq!(language(Some("UTF-8"), has).as_deref(), Some("C.UTF-8"));
        assert_eq!(language(None, has).as_deref(), Some("C.UTF-8"));
        assert_eq!(
            language(Some("ja_JP.UTF-8;export X=1"), has).as_deref(),
            Some("C.UTF-8")
        );
        assert_eq!(language(Some("fr_FR.UTF-8"), |_| false), None);
    }

    #[test]
    fn the_shell_starts_in_the_project_or_at_home() {
        let home = std::env::temp_dir();
        assert_eq!(start_folder(None, &home).unwrap(), home);
        assert_eq!(start_folder(Some("~"), &home).unwrap(), home);
        assert_eq!(start_folder(Some("/"), &home).unwrap(), PathBuf::from("/"));
        assert!(start_folder(Some("relative/folder"), &home).is_err());
        assert!(start_folder(Some("/no/such/folder/here"), &home).is_err());
        assert!(start_folder(Some("~/no-such-folder-here-either"), &home).is_err());
        assert!(start_folder(Some("/tmp\u{0}x"), &home).is_err());
    }

    #[test]
    fn the_environment_is_made_afresh() {
        let account = Account {
            name: "someone".into(),
            home: PathBuf::from("/var/empty/a-test-user"),
            shell: PathBuf::from("/bin/zsh"),
        };
        let request = ShellRequest {
            cols: 80,
            rows: 24,
            term: Some("xterm-ghostty".into()),
            locale: None,
            project: None,
        };
        let inherited = |name: &str| match name {
            "TMPDIR" | "XDG_RUNTIME_DIR" => Some("/run/user/1000".to_string()),
            _ => Some("from the agent".to_string()),
        };
        let env = environment(&account, Path::new("/bin/zsh"), &request, inherited);
        let get = |name: &str| {
            env.iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(get("HOME"), Some("/var/empty/a-test-user"));
        assert_eq!(get("USER"), Some("someone"));
        assert_eq!(get("SHELL"), Some("/bin/zsh"));
        assert_eq!(get("TERM"), Some("xterm-ghostty"));
        // Nothing else of the agent's comes along.
        assert!(env.iter().all(|(_, value)| value != "from the agent"));
        assert!(get("ISTESSO_CONFIG_DIR").is_none());
    }
}
