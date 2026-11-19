//! Requests to open a web page, from inside a workspace on the host.
//!
//! * The agent listens on a Unix socket in a folder that only its user may
//!   enter ([`super::private_dir`]). A shell that asked for forwards gets
//!   `BROWSER` pointed at `istesso-open`, and (on Linux) an `xdg-open` of
//!   Istesso's first on its `PATH`: both are this program under another
//!   name.
//! * `istesso-open <address>` sends the address, on one line. The agent
//!   asks the system which process is at the other end of the socket
//!   (`SO_PEERCRED`), and passes the address on only when that process
//!   belongs to the workspace of an open shell, to that shell's device.
//! * The device decides what is opened ([`super::device`]).
//!
//! Written down: that a request was passed on or refused, and why in a
//! word; the port when it is one on the host. Never the address.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use istesso_link::forward::MAX_URL;
use istesso_link::shell::ShellFrame;

use super::url;
use super::watch::Workspaces;
use crate::serve::Log;
use crate::Paths;

/// The variable that tells `istesso-open` where the agent listens.
pub const SOCKET_VARIABLE: &str = "ISTESSO_OPEN_SOCKET";
/// The names this program answers to as the opener of pages.
pub const OPEN_NAME: &str = "istesso-open";
pub const XDG_OPEN_NAME: &str = "xdg-open";

/// How long a request may take to arrive, and its answer.
const WAIT: Duration = Duration::from_secs(2);
/// How long the ports are looked at before a request for one of them is
/// passed on.
const LOOK_WITHIN: Duration = Duration::from_secs(1);

/// Where the agent takes requests, and the folder with the programs that
/// make them.
pub struct OpenServer {
    pub socket: PathBuf,
    /// `istesso-open` and `xdg-open`, when this program is `istesso`.
    pub bin: Option<PathBuf>,
}

impl OpenServer {
    /// What a shell of a workspace gets in its environment.
    pub fn environment(&self) -> Vec<(String, String)> {
        let mut env = vec![(
            SOCKET_VARIABLE.to_string(),
            self.socket.display().to_string(),
        )];
        if let Some(bin) = &self.bin {
            env.push((
                "BROWSER".to_string(),
                bin.join(OPEN_NAME).display().to_string(),
            ));
        }
        env
    }
}

/// Starts taking requests.
pub fn start(paths: &Paths, workspaces: Arc<Workspaces>, log: Log) -> std::io::Result<OpenServer> {
    let dir = super::private_dir(paths)?;
    let socket = dir.join("open.sock");
    remove_socket(&socket);
    let listener = UnixListener::bind(&socket)?;
    let bin = make_bin(&dir).ok().flatten();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (workspaces, log) = (workspaces.clone(), log.clone());
            std::thread::spawn(move || answer(stream, &workspaces, &log));
        }
    });
    Ok(OpenServer { socket, bin })
}

/// Removes a socket left from an earlier run at `path`, and nothing else.
fn remove_socket(path: &Path) {
    use std::os::unix::fs::FileTypeExt;
    if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_socket()) {
        let _ = std::fs::remove_file(path);
    }
}

/// `bin/istesso-open` and `bin/xdg-open`, links to this program, when it is
/// `istesso` (in tests it is not, and none are made).
fn make_bin(dir: &Path) -> std::io::Result<Option<PathBuf>> {
    use std::os::unix::fs::DirBuilderExt;
    let program = std::env::current_exe()?;
    if program.file_name().and_then(|name| name.to_str()) != Some("istesso") {
        return Ok(None);
    }
    let bin = dir.join("bin");
    match std::fs::DirBuilder::new().mode(0o700).create(&bin) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let names: &[&str] = if cfg!(target_os = "linux") {
        &[OPEN_NAME, XDG_OPEN_NAME]
    } else {
        &[OPEN_NAME]
    };
    for name in names {
        let link = bin.join(name);
        if std::fs::symlink_metadata(&link).is_ok_and(|meta| meta.file_type().is_symlink()) {
            std::fs::remove_file(&link)?;
        }
        std::os::unix::fs::symlink(&program, &link)?;
    }
    Ok(Some(bin))
}

/// The user and process at the other end of a Unix socket, as the system
/// says.
#[cfg(target_os = "linux")]
pub(crate) fn peer_of(stream: &UnixStream) -> Option<(u32, i32)> {
    use std::os::fd::AsRawFd;
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: the buffer is a ucred, and its length is given.
    let got = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    (got == 0).then_some((credentials.uid, credentials.pid))
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn peer_of(stream: &UnixStream) -> Option<(u32, i32)> {
    use std::os::fd::AsRawFd;
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: two plain out-parameters.
    let got = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    (got == 0).then_some((uid, -1))
}

/// Takes one request and answers it with `ok` or `refused: <why>`.
fn answer(stream: UnixStream, workspaces: &Workspaces, log: &Log) {
    let _ = stream.set_read_timeout(Some(WAIT));
    let _ = stream.set_write_timeout(Some(WAIT));
    let verdict = decide(&stream, workspaces, log);
    let line = match verdict {
        Ok(()) => "ok\n".to_string(),
        Err(why) => format!("refused: {why}\n"),
    };
    let _ = (&stream).write_all(line.as_bytes());
}

fn decide(stream: &UnixStream, workspaces: &Workspaces, log: &Log) -> Result<(), String> {
    // SAFETY: getuid cannot fail.
    let me = unsafe { libc::getuid() };
    let (uid, pid) = peer_of(stream).ok_or("cannot tell who asks")?;
    if uid != me {
        return Err("only programs of this user may ask".into());
    }
    let mut line = Vec::new();
    BufReader::new(stream.take(MAX_URL as u64 + 1))
        .read_until(b'\n', &mut line)
        .map_err(|_| "the request did not arrive")?;
    if line.pop() != Some(b'\n') {
        return Err("the request did not arrive whole".into());
    }
    let text = String::from_utf8(line).map_err(|_| url::Refusal::Malformed.to_string())?;
    let address = url::parse(&text).map_err(|why| why.to_string())?;
    let workspace = workspaces.holding(pid).ok_or_else(|| {
        log(
            "refused a request to open a page: it does not come from the workspace of a shell"
                .into(),
        );
        "only programs started in a shell of another device may ask".to_string()
    })?;
    if !workspace.may_ask() {
        log(format!(
            "refused a request to open a page from the workspace of the shell for {}: too many",
            workspace.device_name()
        ));
        return Err("too many requests; wait a few seconds".into());
    }
    let what = if address.is_on_the_host() {
        // A server that has just begun to listen asks to be opened at once:
        // its port is looked for first, and listed before the request.
        workspace.look_now(LOOK_WITHIN);
        format!("for port {} on the host", address.port())
    } else {
        "for an outside address".to_string()
    };
    if !workspace.send(&ShellFrame::OpenUrl(address.as_given())) {
        return Err("the shell is over".into());
    }
    log(format!(
        "passed on a request to open a page {what} from the workspace of the shell for {}",
        workspace.device_name()
    ));
    Ok(())
}

/// `istesso-open <address>`, or `xdg-open …`: what this program does when
/// it runs under one of those names. Returns the exit status.
pub fn run_as(name: &str, args: &[OsString]) -> u8 {
    let say = |line: &str| {
        let _ = writeln!(std::io::stderr(), "{name}: {line}");
    };
    let address = match args {
        [one] => one.to_str().map(str::to_string),
        _ => None,
    };
    let web = address
        .as_deref()
        .map(|text| {
            let lower = text.get(..8).unwrap_or(text).to_ascii_lowercase();
            lower.starts_with("http://") || lower.starts_with("https://")
        })
        .unwrap_or(false);
    if name == XDG_OPEN_NAME && !web {
        return pass_to_the_real_xdg_open(args, &say);
    }
    let Some(address) = address else {
        say("give one address of a web page (http or https)");
        return 2;
    };
    if let Err(why) = url::parse(&address) {
        say(&why.to_string());
        return 1;
    }
    let Some(socket) = std::env::var_os(SOCKET_VARIABLE).map(PathBuf::from) else {
        say("this is not a shell opened from another device with Istesso");
        return 1;
    };
    let reply = (|| -> std::io::Result<String> {
        let mut stream = UnixStream::connect(&socket)?;
        stream.set_read_timeout(Some(WAIT * 2))?;
        stream.write_all(format!("{address}\n").as_bytes())?;
        let mut reply = String::new();
        BufReader::new(stream).read_line(&mut reply)?;
        Ok(reply)
    })();
    match reply {
        Ok(reply) if reply.trim_end() == "ok" => 0,
        Ok(reply) => {
            say(reply.trim_end());
            1
        }
        Err(err) => {
            say(&format!("cannot reach the Istesso agent: {err}"));
            1
        }
    }
}

/// Anything but one web address goes to the `xdg-open` that comes after
/// this one on the `PATH`, as it would have without Istesso.
fn pass_to_the_real_xdg_open(args: &[OsString], say: &dyn Fn(&str)) -> u8 {
    use std::os::unix::process::CommandExt;
    let me = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .ok();
    let real = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(XDG_OPEN_NAME))
            .find(|candidate| {
                candidate.is_file() && candidate.canonicalize().ok().as_ref() != me.as_ref()
            })
    });
    let Some(real) = real else {
        say("no other xdg-open is installed");
        return 3;
    };
    let err = std::process::Command::new(real).args(args).exec();
    say(&format!("cannot start xdg-open: {err}"));
    3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_opener_refuses_what_is_not_a_web_address_before_asking_anyone() {
        // No socket is named: these must fail before one would be needed.
        for args in [
            vec![],
            vec![OsString::from("http://a"), OsString::from("http://b")],
        ] {
            assert_eq!(run_as(OPEN_NAME, &args), 2);
        }
        for bad in ["file:///etc/passwd", "javascript:alert(1)", "http://a b/"] {
            assert_eq!(run_as(OPEN_NAME, &[OsString::from(bad)]), 1, "{bad}");
        }
    }

    #[test]
    fn the_peer_of_a_socket_is_this_user() {
        let (a, _b) = UnixStream::pair().unwrap();
        // SAFETY: getuid cannot fail.
        let me = unsafe { libc::getuid() };
        assert_eq!(peer_of(&a).map(|(uid, _)| uid), Some(me));
        #[cfg(target_os = "linux")]
        assert_eq!(
            peer_of(&a).map(|(_, pid)| pid),
            Some(std::process::id() as i32)
        );
    }
}
