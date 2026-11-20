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

use super::sys::{self, Account};

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
/// How long what the shell printed last may take to leave before the frame
/// that says how it ended, and how long that frame may take. A device that
/// does not read is not waited for longer.
const LAST_WORDS: Duration = Duration::from_secs(1);
/// What the device typed that the shell has not taken yet, at most. Up to
/// this, the host goes on reading the device (and so hears it hang up, or
/// say "still here") while a program in the shell does not read. Beyond it
/// the host stops reading, and ends the shell when the shell has taken
/// nothing for [`SILENCE`].
pub const MAX_PENDING: usize = 1024 * 1024;

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
    /// For a shell whose device asked for forwards: `systemd-run`, to start
    /// it in a scope of its own (Linux), which tells its workspace apart
    /// ([`crate::workspace::watch`]). Without it, the shell's session does.
    pub scope_tool: Option<PathBuf>,
    /// For a shell whose device asked for forwards: what goes into its
    /// environment for requests to open a page (`BROWSER` and the agent's
    /// socket), and the folder put first on its `PATH`.
    pub workspace_env: Vec<(String, String)>,
    pub workspace_bin: Option<PathBuf>,
    /// Where the projects registered on this machine are looked up, for a
    /// device that names one instead of a folder (`istesso projects add`).
    /// Without it, only folders are taken.
    pub projects: Option<crate::Paths>,
}

impl ShellSettings {
    /// The folder of the project `name` registered on this machine, when
    /// projects are looked up at all.
    fn registered(&self, name: &str) -> Option<PathBuf> {
        self.projects
            .as_ref()
            .and_then(|paths| crate::projects::folder(paths, name))
    }
}

/// A shell that has started and has not been handed its conversation yet.
/// Dropping it hangs the shell up.
pub struct Shell {
    child: Option<Child>,
    master: Option<OwnedFd>,
    pid: u32,
    started: Instant,
    /// The scope it was started in, by its unit's name.
    scope: Option<String>,
    /// How long the device may say nothing ([`SILENCE`]).
    silence: Duration,
}

/// Where the workspace of a shell sends frames of its own on the shell's
/// conversation (the list of ports, requests to open a page), next to what
/// the shell prints.
#[derive(Clone)]
pub struct ShellOutlet {
    writer: Arc<Mutex<ChannelWriter>>,
    stop: Arc<AtomicBool>,
}

impl ShellOutlet {
    /// Sends one frame; false once the conversation is over.
    pub fn send(&self, frame: &ShellFrame) -> bool {
        if self.stop.load(Ordering::SeqCst) {
            return false;
        }
        self.writer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .send(&frame.encode())
            .is_ok()
    }
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

/// The folder the shell starts in: the folder the device named, the folder
/// of the project it named, or home. A project's name never looks like a
/// path (it starts with a letter or digit and has no `/`), and a relative
/// path was never taken, so the two cannot be mistaken for each other.
/// `registered` gives the folder of a project registered on this machine.
fn start_folder(
    project: Option<&str>,
    home: &Path,
    registered: impl Fn(&str) -> Option<PathBuf>,
) -> Result<PathBuf, String> {
    let folder = match project {
        None => return Ok(home.to_path_buf()),
        Some(path) if path.contains('\0') => return Err("that is not a folder name".into()),
        Some("~") => home.to_path_buf(),
        Some(path) if path.starts_with("~/") => home.join(&path[2..]),
        Some(path) if Path::new(path).is_absolute() => PathBuf::from(path),
        Some(name) if crate::projects::validate_name(name).is_ok() => {
            let Some(folder) = registered(name) else {
                return Err("no project of that name is registered on the host".into());
            };
            if !folder.is_dir() {
                return Err("the folder of that project is not on the host any more".into());
            }
            return Ok(folder);
        }
        Some(_) => {
            return Err(
                "the project folder must be an absolute path on the host or start with ~/, \
                 or be the name of a project registered there"
                    .into(),
            )
        }
    };
    if folder.is_dir() {
        Ok(folder)
    } else {
        Err("there is no such folder on the host".into())
    }
}

/// The folder below which the host's main editor may open files of the
/// shell's workspace (ADR 0007, "メインのエディターで開く頼み"): the project
/// the device named, by its real place. None without a project, and none
/// when it is the root or the home folder itself. A project named by its
/// name is found as the shell found it ([`ShellSettings::registered`]).
pub(crate) fn project_folder(project: Option<&str>, settings: &ShellSettings) -> Option<PathBuf> {
    let project = project?;
    let account = sys::account().ok()?;
    let folder = start_folder(Some(project), &account.home, |name| {
        settings.registered(name)
    })
    .ok()?;
    crate::workspace::editor::workspace_folder(&folder, &account.home)
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
        sys::refuse_root(sys::is_root())?;
        let account = sys::account()?;
        let folder = start_folder(request.project.as_deref(), &account.home, |name| {
            settings.registered(name)
        })?;
        let (program, argv0) = match &settings.program {
            Some(program) => (program.clone(), program_name(program)),
            None => (
                account.shell.clone(),
                format!("-{}", program_name(&account.shell)),
            ),
        };
        let mut env = environment(&account, &program, request, |name| std::env::var(name).ok());
        if request.forward {
            workspace_environment(&mut env, settings);
        }

        // A shell whose ports are forwarded starts in a scope of its own,
        // when there is a way to make one: `systemd-run` registers the scope
        // and then becomes the shell, in the same process. It cannot give the
        // shell a name that starts with `-`, so a login shell is asked for
        // with `-l` instead.
        let scope = match (&settings.scope_tool, request.forward) {
            (Some(_), true) => Some(format!(
                "istesso-shell-{}",
                &crate::workspace::random_id()
                    .map_err(|e| format!("cannot name the shell's scope: {e}"))?[..16]
            )),
            _ => None,
        };
        let mut command = match (&settings.scope_tool, &scope) {
            (Some(tool), Some(unit)) => {
                let mut command = Command::new(tool);
                command
                    .args(["--user", "--scope", "--quiet", "--collect"])
                    .arg(format!("--unit={unit}"))
                    .arg("--")
                    .arg(&program);
                if settings.program.is_none() {
                    command.arg("-l");
                }
                command
            }
            _ => {
                let mut command = Command::new(&program);
                command.arg0(argv0);
                command
            }
        };
        command.env_clear().envs(env).current_dir(&folder);

        let below = sys::descriptors_to_look_at();
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
            // `fcntl`, `close`, `signal`, `setsid` and `ioctl`, which are safe
            // there. The standard input is the terminal by then.
            unsafe {
                command.pre_exec(move || {
                    // Nothing of the agent's goes along: a descriptor some
                    // library opened without close-on-exec is closed here.
                    sys::close_inherited(below);
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
            scope,
            silence: SILENCE,
        })
    }

    /// Changes how long the device may say nothing, from [`SILENCE`]. For
    /// tests, which cannot wait a minute.
    pub fn set_silence(&mut self, silence: Duration) {
        self.silence = silence;
    }

    /// The shell's process number.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// The scope the shell was started in, by its unit's name.
    pub fn scope(&self) -> Option<&str> {
        self.scope.as_deref()
    }

    /// Carries the shell over `channel` until it ends, the device goes, or
    /// `still_allowed` says no (asked every [`RECHECK`]).
    pub fn serve(self, channel: Channel<TcpStream>, still_allowed: impl FnMut() -> bool) -> Served {
        self.serve_with(channel, still_allowed, |_| {})
    }

    /// As [`Shell::serve`], and hands `attach` the conversation's outlet once
    /// the shell is carried, for the shell's workspace to send on.
    pub fn serve_with(
        mut self,
        channel: Channel<TcpStream>,
        mut still_allowed: impl FnMut() -> bool,
        attach: impl FnOnce(ShellOutlet),
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
            .set_read_timeout(Some(self.silence))
            .and_then(|_| stream.set_write_timeout(Some(self.silence)));
        let (events_in, events) = mpsc::channel::<Event>();
        // A handle on the connection that ends it from here, whatever the other
        // threads are waiting for.
        let socket = stream.try_clone();
        let (Ok(socket), Ok((mut reader, writer))) = (socket, channel.split()) else {
            child_exit_watch(child, events_in);
            hang_up(master, &events);
            return counts.served(Ending::DeviceGone, self.started);
        };
        // A frame from the device is never larger than this; a larger one is
        // refused before it is taken in.
        reader.set_max_message(1 + istesso_link::shell::MAX_DATA_ACCEPTED);
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
            let silence = self.silence;
            std::thread::spawn(move || {
                pass_input(reader, fd, writer, stop, counts, events, silence)
            })
        });
        child_exit_watch(child, events_in);
        attach(ShellOutlet {
            writer: writer.clone(),
            stop: stop.clone(),
        });

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
        // The device is told how the shell ended, after what the shell printed
        // last, when that can be done soon. A device that does not read is not
        // waited for: its connection is shut, which also ends a write that is
        // waiting for it.
        let last = match ending {
            Ending::Shell(end) => Some(ShellFrame::Exit(end)),
            Ending::Revoked => Some(ShellFrame::Exit(ShellEnd::Revoked)),
            Ending::DeviceGone => None,
        };
        if let Some(last) = last {
            // After the shell ended, its last output is worth a moment. After the
            // right was taken away, nothing is: only what the thread that prints
            // needs to notice that it should stop.
            let patience = match ending {
                Ending::Shell(_) => LAST_WORDS,
                _ => 2 * TICK,
            };
            let printed = printing
                .as_ref()
                .is_none_or(|printing| finished_within(printing, patience));
            if printed {
                if let Some(mut writer) = locked_within(&writer, LAST_WORDS) {
                    let _ = socket.set_write_timeout(Some(LAST_WORDS));
                    let _ = writer.send(&last.encode());
                }
            }
        }
        let _ = socket.shutdown(std::net::Shutdown::Both);
        if let Some(printing) = printing {
            let _ = printing.join();
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

/// Adds what a shell of a workspace gets: the variables for requests to open
/// a page, and the folder with `istesso-open` (and `xdg-open`) first on its
/// `PATH`.
fn workspace_environment(env: &mut Vec<(String, String)>, settings: &ShellSettings) {
    for (name, value) in &settings.workspace_env {
        env.retain(|(known, _)| known != name);
        env.push((name.clone(), value.clone()));
    }
    if let Some(bin) = &settings.workspace_bin {
        if let Some((_, path)) = env.iter_mut().find(|(name, _)| name == "PATH") {
            *path = format!("{}:{path}", bin.display());
        }
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

/// Waits until `thread` has finished, for `limit` at most.
fn finished_within<T>(thread: &std::thread::JoinHandle<T>, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while !thread.is_finished() {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    true
}

/// The writer, when it is free within `limit`: a thread that is waiting in a
/// write to a device that does not read holds it.
fn locked_within(
    writer: &Mutex<ChannelWriter>,
    limit: Duration,
) -> Option<std::sync::MutexGuard<'_, ChannelWriter>> {
    let deadline = Instant::now() + limit;
    loop {
        match writer.try_lock() {
            Ok(guard) => return Some(guard),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => return Some(poisoned.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(std::sync::TryLockError::WouldBlock) => return None,
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
        match sys::wait(Some(master.as_raw_fd()), None, TICK) {
            Ok((false, _)) => continue,
            Ok((true, _)) => {}
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
///
/// The device is read whenever something arrives, also while a program in
/// the shell does not read its input: what was typed waits here
/// ([`MAX_PENDING`] at most) and goes to the terminal as the program takes
/// it. So a device that hangs up, or says nothing for [`SILENCE`], is heard
/// even then. When more than that waits, the device is no longer read, and
/// the shell is ended once it has taken nothing for [`SILENCE`].
fn pass_input(
    mut reader: istesso_link::ChannelReader,
    master: OwnedFd,
    writer: Arc<Mutex<ChannelWriter>>,
    stop: Arc<AtomicBool>,
    counts: Arc<Counts>,
    events: mpsc::Sender<Event>,
    silence: Duration,
) {
    let socket = reader.stream().as_raw_fd();
    let terminal = master.as_raw_fd();
    let mut pending: Vec<u8> = Vec::new();
    let mut patience = Patience::new(silence, Instant::now());
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let full = pending.len() >= MAX_PENDING;
        if patience.run_out(full, Instant::now()) {
            break;
        }
        let (readable, writable) = match sys::wait(
            (!full).then_some(socket),
            (!pending.is_empty()).then_some(terminal),
            TICK,
        ) {
            Ok(ready) => ready,
            Err(_) => break,
        };
        if writable {
            to_terminal(terminal, &mut pending, &mut patience);
        }
        if !readable {
            continue;
        }
        // A frame that has begun to arrive is whole within the connection's
        // timeout, or the connection is over.
        let Ok(message) = reader.recv() else { break };
        patience.heard(Instant::now());
        let Ok(frame) = ShellFrame::decode(&message) else {
            break;
        };
        match frame {
            ShellFrame::Data(bytes) => {
                counts
                    .bytes_in
                    .fetch_add(bytes.len() as u64, Ordering::Relaxed);
                counts.frames_in.fetch_add(1, Ordering::Relaxed);
                if pending.is_empty() {
                    patience.taken(Instant::now());
                }
                pending.extend_from_slice(&bytes);
                // At once, not after the next wait: what is typed is not held up.
                to_terminal(terminal, &mut pending, &mut patience);
            }
            ShellFrame::Resize { cols, rows } => {
                counts.resizes.fetch_add(1, Ordering::Relaxed);
                let _ = sys::set_size(terminal, clamp(cols), clamp(rows));
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
            // Only the host ends a shell, lists ports, asks to open pages and opens
            // windows; frames of a later version are skipped.
            ShellFrame::Exit(_)
            | ShellFrame::Ports(_)
            | ShellFrame::OpenUrl(_)
            | ShellFrame::OpenWindow(_)
            | ShellFrame::Unknown(_) => {}
        }
    }
    let _ = events.send(Event::DeviceGone);
}

/// When the host stops waiting for a device whose typing waits for the shell.
///
/// While less than [`MAX_PENDING`] waits, the device is read, and it is gone
/// when it has said nothing for the silence. While more waits, it is not read
/// and cannot be heard: then the shell is given the silence to take
/// something, and when it does not, the conversation ends. Leaving that
/// state starts the device's silence afresh: it could not be heard while it
/// was not read.
struct Patience {
    silence: Duration,
    heard: Instant,
    taken: Instant,
    was_full: bool,
}

impl Patience {
    fn new(silence: Duration, now: Instant) -> Self {
        Patience {
            silence,
            heard: now,
            taken: now,
            was_full: false,
        }
    }

    /// The device said something.
    fn heard(&mut self, now: Instant) {
        self.heard = now;
    }

    /// The shell took some of what waits (or nothing waited until now).
    fn taken(&mut self, now: Instant) {
        self.taken = now;
    }

    /// Whether to stop waiting, with `full` saying whether more than
    /// [`MAX_PENDING`] waits now.
    fn run_out(&mut self, full: bool, now: Instant) -> bool {
        if self.was_full && !full {
            self.heard = now;
        }
        self.was_full = full;
        let since = if full { self.taken } else { self.heard };
        now.saturating_duration_since(since) >= self.silence
    }
}

/// Gives the terminal as much of `pending` as it takes now, without waiting.
/// A terminal that is gone takes nothing more, and what waits is dropped.
fn to_terminal(terminal: std::os::fd::RawFd, pending: &mut Vec<u8>, patience: &mut Patience) {
    while !pending.is_empty() {
        match sys::write(terminal, pending) {
            Ok(0) => return,
            Ok(n) => {
                pending.drain(..n);
                patience.taken(Instant::now());
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return,
            Err(_) => {
                pending.clear();
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The second review's case: more than is kept waits for a program that
    /// reads slowly, for longer than the silence, while the device is there.
    #[test]
    fn a_device_that_could_not_be_heard_while_much_waited_is_not_taken_for_gone() {
        let silence = Duration::from_secs(60);
        let t = Instant::now();
        let at = |seconds: u64| t + Duration::from_secs(seconds);
        let mut patience = Patience::new(silence, t);
        patience.heard(at(1));
        // Full from 2 s on; the program takes a little every 10 s.
        for second in (2..=130).step_by(10) {
            patience.taken(at(second));
            assert!(!patience.run_out(true, at(second)), "{second}");
        }
        // Below the limit again after 130 s: the device is read again, and its
        // silence counts from now.
        assert!(!patience.run_out(false, at(131)));
        assert!(!patience.run_out(false, at(190)));
        // It says nothing for the whole silence after that: gone.
        assert!(patience.run_out(false, at(191)));
    }

    #[test]
    fn a_shell_that_takes_nothing_while_much_waits_is_given_up_after_the_silence() {
        let silence = Duration::from_secs(60);
        let t = Instant::now();
        let at = |seconds: u64| t + Duration::from_secs(seconds);
        let mut patience = Patience::new(silence, t);
        patience.taken(at(1));
        assert!(!patience.run_out(true, at(60)));
        assert!(patience.run_out(true, at(61)));
        // A device that is read and heard keeps the conversation.
        let mut patience = Patience::new(silence, t);
        for second in (0..200).step_by(10) {
            patience.heard(at(second));
            assert!(!patience.run_out(false, at(second + 5)));
        }
    }

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
        let none = |_: &str| None;
        assert_eq!(start_folder(None, &home, none).unwrap(), home);
        assert_eq!(start_folder(Some("~"), &home, none).unwrap(), home);
        assert_eq!(
            start_folder(Some("/"), &home, none).unwrap(),
            PathBuf::from("/")
        );
        assert!(start_folder(Some("relative/folder"), &home, none).is_err());
        assert!(start_folder(Some("/no/such/folder/here"), &home, none).is_err());
        assert!(start_folder(Some("~/no-such-folder-here-either"), &home, none).is_err());
        assert!(start_folder(Some("/tmp\u{0}x"), &home, none).is_err());
    }

    #[test]
    fn a_project_is_named_by_a_word_and_never_by_a_path() {
        let home = std::env::temp_dir();
        let asked = std::cell::RefCell::new(Vec::new());
        let registered = |name: &str| {
            asked.borrow_mut().push(name.to_string());
            match name {
                "demo" => Some(home.clone()),
                "gone" => Some(PathBuf::from("/no/such/folder/here")),
                _ => None,
            }
        };
        assert_eq!(start_folder(Some("demo"), &home, registered).unwrap(), home);
        let unknown = start_folder(Some("other"), &home, registered).unwrap_err();
        assert!(unknown.contains("no project of that name"), "{unknown}");
        assert!(!unknown.contains("other"), "the name sent is not quoted");
        assert!(start_folder(Some("gone"), &home, registered).is_err());
        // What is not a project name is not looked up: a path with a slash,
        // a hidden folder's name, `..`, an option.
        for path in ["work/demo", ".demo", "..", "-demo", "two words"] {
            assert!(start_folder(Some(path), &home, registered).is_err());
        }
        assert_eq!(*asked.borrow(), ["demo", "other", "gone"]);
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
            forward: false,
            windows: false,
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
