//! The device's end of a shell: the program a terminal application runs
//! instead of a local shell (`istesso shell <device>`).
//!
//! The terminal is put into raw mode and every byte typed goes to the other
//! machine as it is; every byte the shell there prints comes back and is
//! written out as it is. Nothing is interpreted here, so any terminal
//! application works, and so does any program inside the shell.

use std::io::Write;
use std::os::fd::{FromRawFd, RawFd};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use istesso_link::shell::{ShellEnd, ShellFrame, MAX_DATA};
use istesso_link::{ChannelWriter, Error as LinkError, ShellAnswer, ShellRequest};

use super::sys;
use crate::survey::connect_peer_or_say_why;
use crate::Paths;

const STDIN: RawFd = 0;
const STDOUT: RawFd = 1;

/// How long finding the device and its handshake may take.
const CONNECT_WITHIN: Duration = Duration::from_secs(3);
/// How long the device's answer to the request for a shell may take: it
/// starts the shell first.
const ANSWER_WITHIN: Duration = Duration::from_secs(10);
/// How long the host may say nothing before the connection is taken for
/// lost. It answers "still here", which is said every
/// [`super::ALIVE_EVERY`].
const SILENCE: Duration = Duration::from_secs(30);

/// What a shell is asked for.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The device, as for `istesso peers`: its name or the beginning of its
    /// id.
    pub device: String,
    /// Where to reach it, when not where it was last seen.
    pub address: Option<String>,
    /// Where on the device to start, as [`ShellRequest::project`] takes it:
    /// a folder there, or the name of a project registered there.
    /// Remembered for the device once the shell runs there; without one,
    /// the shell starts where it started last time.
    pub project: Option<String>,
    /// Whether and how the ports of the shell's workspace are forwarded to
    /// this machine, and its requests to open a page taken
    /// ([`crate::workspace::device`]).
    pub forward: crate::workspace::device::Settings,
    /// Start at home, and forget the remembered folder.
    pub home: bool,
}

/// The folder a shell on the device `id` was last opened in, if one is
/// remembered.
pub fn remembered_project(paths: &Paths, id: &str) -> Option<String> {
    let file = paths.last_project(id)?;
    let text = std::fs::read_to_string(file).ok()?;
    let project = text.trim_end_matches('\n');
    (!project.is_empty() && !project.contains('\n')).then(|| project.to_string())
}

/// Remembers `project` as the folder for the device `id`, or forgets the
/// folder (`None`). Readable by its owner only; a failure to remember is not
/// worth a word.
pub fn remember_project(paths: &Paths, id: &str, project: Option<&str>) {
    let Some(file) = paths.last_project(id) else {
        return;
    };
    match project {
        None => {
            let _ = std::fs::remove_file(file);
        }
        Some(project) => {
            use std::io::Write as _;
            use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
            if let Some(dir) = file.parent() {
                let _ = std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(dir);
            }
            let written = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&file)
                .and_then(|mut out| out.write_all(format!("{project}\n").as_bytes()));
            if written.is_err() {
                let _ = std::fs::remove_file(file);
            }
        }
    }
}

/// Opens a shell on another device in this terminal and returns when it is
/// over, with the status this program should end with: the shell's own, or
/// 255 when there was no shell or the connection was lost (as `ssh` does).
/// What went wrong is said on the standard error, in one line.
pub fn run(paths: &Paths, options: &Options) -> u8 {
    let say = |line: &str| {
        let _ = writeln!(std::io::stderr(), "istesso shell: {line}");
    };
    let mut client = match connect_peer_or_say_why(
        paths,
        &options.device,
        options.address.as_deref(),
        CONNECT_WITHIN,
    ) {
        Ok(client) => client,
        Err(why) => {
            say(&format!("cannot reach {}: {why}", options.device));
            return 255;
        }
    };
    let device = client.peer().name.clone();
    let device_id = client.peer().id.clone();
    let _ = client.set_timeout(ANSWER_WITHIN);
    let (project, remembered) = match (&options.project, options.home) {
        (_, true) => (None, false),
        (Some(project), false) => (Some(project.clone()), false),
        (None, false) => {
            let project = remembered_project(paths, &device_id);
            let remembered = project.is_some();
            (project, remembered)
        }
    };

    let terminal = sys::is_terminal(STDIN);
    let (cols, rows) = sys::size(STDIN)
        .or_else(|| sys::size(STDOUT))
        .unwrap_or((80, 24));
    let variable = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let request = ShellRequest {
        cols,
        rows,
        term: variable("TERM"),
        locale: variable("LC_ALL")
            .or_else(|| variable("LC_CTYPE"))
            .or_else(|| variable("LANG")),
        project: project.clone(),
        forward: options.forward.enabled,
    };
    let peer_id = client.peer().id.clone();
    match client.open_shell(request) {
        // The device took the folder: the next shell without one opens there.
        Ok(ShellAnswer::Open) => remember_project(paths, &device_id, project.as_deref()),
        Ok(ShellAnswer::Refused { message, .. }) => {
            say(&format!("{device} did not open a shell: {message}"));
            if remembered {
                say("(that folder was remembered from last time; --home starts at home and forgets it)");
            }
            return 255;
        }
        Err(LinkError::Remote(message)) => {
            say(&format!("{device} did not open a shell: {message}"));
            return 255;
        }
        Err(err) => {
            say(&format!(
                "{device} did not answer the request for a shell: {err}"
            ));
            return 255;
        }
    }

    let channel = client.into_channel();
    let stream = channel.stream();
    let _ = stream
        .set_read_timeout(Some(SILENCE))
        .and_then(|_| stream.set_write_timeout(Some(SILENCE)));
    let (mut reader, writer) = match channel.split() {
        Ok(halves) => halves,
        Err(err) => {
            say(&format!("cannot use the connection to {device}: {err}"));
            return 255;
        }
    };
    // A frame from the host is never larger than this; a larger one is refused
    // before it is taken in.
    reader.set_max_message(1 + istesso_link::shell::MAX_DATA_ACCEPTED);
    let writer = Arc::new(Mutex::new(writer));
    let address = reader.stream().peer_addr();

    // Signals are taken by one thread of their own, from here on: the window
    // changing size, and the terminal or someone else ending this program.
    let signals = sys::block_signals(&[
        libc::SIGWINCH,
        libc::SIGHUP,
        libc::SIGTERM,
        libc::SIGINT,
        libc::SIGQUIT,
        libc::SIGUSR1,
        libc::SIGUSR2,
        libc::SIGALRM,
    ]);
    // The shell's workspace: its ports are forwarded here, and its requests
    // to open a page are taken here. Notices go on a line of their own.
    // Made after the signals are blocked: its threads must not take them
    // (they would go to the handler that does nothing, and be lost).
    let forwarder = match (options.forward.enabled, address) {
        (true, Ok(address)) => {
            let notice: crate::workspace::device::Notice = Arc::new(|line: &str| {
                let _ = write!(std::io::stderr(), "\r\nistesso: {line}\r\n");
            });
            match crate::workspace::device::Forwarder::new(
                paths,
                &device,
                &peer_id,
                address,
                options.forward.clone(),
                notice,
            ) {
                Ok(forwarder) => Some(forwarder),
                Err(err) => {
                    say(&format!("ports are not forwarded: {err}"));
                    None
                }
            }
        }
        _ => None,
    };
    if terminal {
        if let Err(err) = sys::RawMode::enter(STDIN) {
            say(&format!("cannot pass keys on as they are typed: {err}"));
            return 255;
        }
        // However this program ends, the terminal is put back as it was.
        let before = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            sys::RawMode::restore(STDIN);
            before(info);
        }));
    }
    {
        let writer = writer.clone();
        std::thread::spawn(move || watch_signals(signals, writer));
    }
    {
        let writer = writer.clone();
        std::thread::spawn(move || pass_keys(writer));
    }
    {
        let writer = writer.clone();
        std::thread::spawn(move || say_alive(writer));
    }

    // SAFETY: the standard output stays open for the life of the program; the
    // file is never dropped, so it is never closed here.
    let mut out = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(STDOUT) });
    let (status, why) = loop {
        let message = match reader.recv() {
            Ok(message) => message,
            Err(LinkError::Timeout) => {
                break (
                    255,
                    Some(format!(
                        "lost the connection to {device}: nothing heard for {} seconds",
                        SILENCE.as_secs()
                    )),
                )
            }
            Err(LinkError::Closed) => {
                break (
                    255,
                    Some(format!(
                        "{device} closed the connection before the shell ended"
                    )),
                )
            }
            Err(err) => break (255, Some(format!("lost the connection to {device}: {err}"))),
        };
        match ShellFrame::decode(&message) {
            Ok(ShellFrame::Data(bytes)) => {
                if out.write_all(&bytes).and_then(|_| out.flush()).is_err() {
                    break (255, None);
                }
            }
            Ok(ShellFrame::Exit(end)) => {
                let why = match end {
                    ShellEnd::Revoked => Some(format!(
                        "{device} ended the shell: this device may no longer have one there"
                    )),
                    ShellEnd::Host => Some(format!("{device} ended the shell")),
                    ShellEnd::Code(_) | ShellEnd::Signal(_) => None,
                };
                break (end.exit_status(), why);
            }
            Ok(ShellFrame::Ports(list)) => {
                if let Some(forwarder) = &forwarder {
                    forwarder.update(list);
                }
            }
            Ok(ShellFrame::OpenUrl(address)) => {
                if let Some(forwarder) = &forwarder {
                    forwarder.open_url(&address);
                }
            }
            // "Still here" is heard by the read itself; frames of a later version are
            // skipped.
            Ok(ShellFrame::Alive | ShellFrame::Resize { .. } | ShellFrame::Unknown(_)) => {}
            Err(err) => {
                break (
                    255,
                    Some(format!(
                        "{device} sent something that is not part of a shell: {err}"
                    )),
                )
            }
        }
    };
    finish(status, why.as_deref())
}

/// Puts the terminal back as it was, says why the shell ended when it did
/// not end by itself, and ends this program. The other threads end with it.
fn finish(status: u8, why: Option<&str>) -> ! {
    sys::RawMode::restore(STDIN);
    crate::workspace::device::cleanup();
    if let Some(why) = why {
        // On a line of its own: the shell may have left the cursor anywhere.
        let _ = writeln!(std::io::stderr(), "\r\nistesso shell: {why}\r");
    }
    std::process::exit(status.into())
}

fn send(writer: &Mutex<ChannelWriter>, frame: &[u8]) -> bool {
    writer
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .send(frame)
        .is_ok()
}

/// What is typed, to the other machine, as it comes.
fn pass_keys(writer: Arc<Mutex<ChannelWriter>>) {
    let mut buffer = vec![0u8; MAX_DATA];
    loop {
        match sys::read(STDIN, &mut buffer) {
            // The end of the input that is not a terminal (a pipe, a file): the shell
            // there goes on until it ends by itself.
            Ok(0) => return,
            Ok(n) => {
                if !send(&writer, &ShellFrame::data(&buffer[..n])) {
                    return;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

/// "Still here", now and then, so that the host can tell a device that is
/// gone from one whose person is reading.
fn say_alive(writer: Arc<Mutex<ChannelWriter>>) {
    loop {
        std::thread::sleep(super::ALIVE_EVERY);
        if !send(&writer, &ShellFrame::Alive.encode()) {
            return;
        }
    }
}

/// A new size of the window goes to the other machine. When the terminal
/// is closed, or this program is told to end, it ends, and the shell on the
/// other machine is hung up (the host sees the connection close).
fn watch_signals(signals: libc::sigset_t, writer: Arc<Mutex<ChannelWriter>>) {
    loop {
        match sys::wait_signal(&signals) {
            libc::SIGWINCH => {
                if let Some((cols, rows)) = sys::size(STDIN).or_else(|| sys::size(STDOUT)) {
                    send(&writer, &ShellFrame::Resize { cols, rows }.encode());
                }
            }
            // The connection closes with this program; the writing half is not
            // waited for, as it may be in the middle of a slow write.
            signal @ (libc::SIGHUP
            | libc::SIGTERM
            | libc::SIGINT
            | libc::SIGQUIT
            | libc::SIGUSR1
            | libc::SIGUSR2
            | libc::SIGALRM) => {
                finish((128 + signal) as u8, None);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_folder_is_remembered_for_each_device_and_can_be_forgotten() {
        let dir = crate::testing::TempDir::new("projects");
        let paths = dir.paths();
        let (one, other) = ("1a2b-3c4d", "5e6f-7a8b");
        assert_eq!(remembered_project(&paths, one), None);
        remember_project(&paths, one, Some("~/src/istesso"));
        remember_project(&paths, other, Some("/srv/other"));
        assert_eq!(
            remembered_project(&paths, one).as_deref(),
            Some("~/src/istesso")
        );
        assert_eq!(
            remembered_project(&paths, other).as_deref(),
            Some("/srv/other")
        );
        // Only its owner can read it.
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(paths.last_project(one).unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0);
        // Home forgets it; the other device keeps its own.
        remember_project(&paths, one, None);
        assert_eq!(remembered_project(&paths, one), None);
        assert_eq!(
            remembered_project(&paths, other).as_deref(),
            Some("/srv/other")
        );
        // An id that is no fingerprint gets no file at all.
        remember_project(&paths, "../x", Some("/"));
        assert_eq!(remembered_project(&paths, "../x"), None);
    }
}
