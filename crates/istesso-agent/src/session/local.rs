//! The person at the host: every session of this machine, whichever device
//! started it, and a way to end each one
//! (`docs/adr/0010-session-start.md`, "ホストにいる人").
//!
//! A device only ever sees the sessions it may use. Somebody has to see the
//! rest: the sessions of a device that was unpaired keep running, because
//! work must not disappear, and they hold places. That somebody is the
//! person who is logged in at the host.
//!
//! The agent answers them on a socket in its runtime directory
//! ([`Paths::agent_sockets`]). Who may ask is decided by the file system and
//! by nothing else: the directory is this user's alone (mode 0700, made so
//! and checked before the socket is opened), so the kernel lets only this
//! user, and root, reach the socket. The asking side checks the same
//! before it believes an answer. Nothing that travels here is a secret and
//! nothing is encrypted. A request is one line of JSON, and so is its
//! answer, with the version the runner's lines carry.

use std::time::{Duration, Instant};

use istesso_link::ErrorCode;
use istesso_proto::{SessionId, SessionInfo, SessionState};
use serde::{Deserialize, Serialize};

use super::Sessions;
use crate::paths::Paths;

/// The device that started a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartedBy {
    pub id: String,
    /// What it was called when it started the session.
    pub name: String,
}

/// A session as the person at the host sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostSession {
    /// The session's report, without what the media path agreed on.
    pub info: SessionInfo,
    pub app_name: String,
    pub started_by: StartedBy,
    /// The device that started it is no longer paired, or lacks a right
    /// that starting takes: it cannot be shown the session or end it. Only
    /// the person at the host can.
    #[serde(default)]
    pub revoked: bool,
}

/// What the person at the host asks the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "ask", rename_all = "snake_case")]
pub enum HostRequest {
    /// Every session that is not over.
    Sessions,
    /// One session, also for a while after it is over.
    Session { session: SessionId },
    /// Ask the session's application to close, or with `force` end it.
    Stop {
        session: SessionId,
        #[serde(default)]
        force: bool,
    },
    /// The list of paired devices was changed: look at it now. Said by
    /// the command that removes a pairing, so that the device is cut off
    /// at that moment and not when the agent next looks.
    PairingsChanged,
}

/// What the agent answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "answer", rename_all = "snake_case")]
pub enum HostAnswer {
    Sessions {
        sessions: Vec<HostSession>,
    },
    Session {
        session: Box<HostSession>,
    },
    /// What was asked has been done; there is nothing to report.
    Done,
    Refused {
        code: ErrorCode,
        message: String,
    },
}

impl Sessions {
    /// Answers the person at this machine.
    pub fn answer_host(&self, request: &HostRequest) -> HostAnswer {
        let refused = |(code, message): (ErrorCode, String)| HostAnswer::Refused { code, message };
        let one = |session: HostSession| HostAnswer::Session {
            session: Box::new(session),
        };
        match request {
            HostRequest::Sessions => HostAnswer::Sessions {
                sessions: self.host_list(),
            },
            HostRequest::Session { session } => match self.host_look(session) {
                Some(found) => one(found),
                None => refused((ErrorCode::NotFound, "no such session".to_string())),
            },
            HostRequest::Stop { session, force } => {
                self.host_stop(session, *force).map_or_else(refused, one)
            }
            HostRequest::PairingsChanged => {
                self.pairings_changed();
                HostAnswer::Done
            }
        }
    }
}

/// Answers the person at this machine. The agent puts its own part in
/// front of [`Sessions::answer_host`].
pub type Answerer = std::sync::Arc<dyn Fn(&HostRequest) -> HostAnswer + Send + Sync>;

/// Why the agent on this machine could not be asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoLink {
    /// None answers, for this user.
    NoAgent,
    Other(String),
}

impl std::fmt::Display for NoLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoLink::NoAgent => f.write_str(
                "no istesso agent that keeps sessions answers on this machine for this user; \
                 `istesso agent` starts one",
            ),
            NoLink::Other(why) => f.write_str(why),
        }
    }
}

/// Tells the agent on this machine that a device was unpaired, so that it
/// cuts the device off now, and asks what that device had started here.
/// `None` when no agent runs here: then nothing of that device does.
pub fn after_unpairing(paths: &Paths, device: &str) -> Result<Option<Vec<HostSession>>, String> {
    let mut link = match HostLink::open(paths) {
        Ok(link) => link,
        Err(NoLink::NoAgent) => return Ok(None),
        Err(other) => return Err(other.to_string()),
    };
    match link.ask(&HostRequest::PairingsChanged)? {
        HostAnswer::Done => {}
        HostAnswer::Refused { message, .. } => return Err(message),
        _ => return Err(unexpected()),
    }
    match link.ask(&HostRequest::Sessions)? {
        HostAnswer::Sessions { sessions } => Ok(Some(
            sessions
                .into_iter()
                .filter(|session| session.started_by.id == device)
                .collect(),
        )),
        HostAnswer::Refused { message, .. } => Err(message),
        _ => Err(unexpected()),
    }
}

/// Every session of this machine that is not over, asked of the agent that
/// runs here.
pub fn sessions_here(paths: &Paths) -> Result<Vec<HostSession>, String> {
    let mut link = HostLink::open(paths).map_err(|why| why.to_string())?;
    match link.ask(&HostRequest::Sessions)? {
        HostAnswer::Sessions { sessions } => Ok(sessions),
        HostAnswer::Refused { message, .. } => Err(message),
        _ => Err(unexpected()),
    }
}

/// Asks a session's application to close, or with `force` ends it, and
/// watches for at most `watch` what becomes of it. The session as it was
/// last seen: still running, if the application had a question.
pub fn stop_here(
    paths: &Paths,
    session: &SessionId,
    force: bool,
    watch: Duration,
) -> Result<HostSession, String> {
    let mut link = HostLink::open(paths).map_err(|why| why.to_string())?;
    let mut ask = |request: &HostRequest| match link.ask(request)? {
        HostAnswer::Session { session } => Ok(*session),
        HostAnswer::Refused { message, .. } => Err(message),
        _ => Err(unexpected()),
    };
    let mut last = ask(&HostRequest::Stop {
        session: session.clone(),
        force,
    })?;
    let until = Instant::now() + watch;
    while last.info.state != SessionState::Ended && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
        last = ask(&HostRequest::Session {
            session: session.clone(),
        })?;
    }
    Ok(last)
}

fn unexpected() -> String {
    "the agent on this machine answered something else than it was asked".to_string()
}

#[cfg(unix)]
pub use unix::{listen, serve, HostLink};

#[cfg(unix)]
mod unix {
    use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use istesso_link::ErrorCode;
    use istesso_proto::session::{decode_line, encode_line};

    use super::{Answerer, HostAnswer, HostRequest, NoLink};
    use crate::paths::Paths;

    /// The longest request that is read.
    const MAX_REQUEST: u64 = 4096;
    /// The longest answer that is read: a host has a handful of sessions.
    const MAX_ANSWER: u64 = 1024 * 1024;
    /// Conversations at the same time; further ones are dropped.
    const MAX_CONVERSATIONS: usize = 16;
    /// How long either side waits for the other's next line.
    const PATIENCE: Duration = Duration::from_secs(5);

    /// Makes sure that `dir` is a directory of this user that nobody else
    /// can enter: that is all that keeps other users from the socket in
    /// it. With `make` it is created, and closed to others if it was open;
    /// without, a directory that is open to others is an error.
    pub(super) fn users_alone(dir: &Path, make: bool) -> std::io::Result<()> {
        if make {
            crate::session::private_dir(dir)?;
        }
        // Not through a link: the directory itself has to be this user's.
        let meta = std::fs::symlink_metadata(dir)?;
        let wrong = |what: &str| {
            std::io::Error::new(
                ErrorKind::PermissionDenied,
                format!("{} {what}", dir.display()),
            )
        };
        if !meta.is_dir() {
            return Err(wrong("is not a directory"));
        }
        if meta.uid() != rustix::process::getuid().as_raw() {
            return Err(wrong("belongs to another user"));
        }
        if meta.mode() & 0o077 != 0 {
            if !make {
                return Err(wrong("can be entered by other users"));
            }
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    /// Opens the socket the person at this machine asks on, in the first
    /// of its places that can be made this user's alone. An error when
    /// none can, or when another agent of this user already answers.
    pub fn listen(paths: &Paths) -> std::io::Result<UnixListener> {
        let mut first = None;
        for path in paths.agent_sockets() {
            match listen_at(&path) {
                Ok(listener) => return Ok(listener),
                Err(taken) if taken.kind() == ErrorKind::AddrInUse => return Err(taken),
                Err(why) => {
                    first.get_or_insert(why);
                }
            }
        }
        Err(first.unwrap_or_else(|| std::io::Error::other("the socket has no place")))
    }

    fn listen_at(path: &Path) -> std::io::Result<UnixListener> {
        let dir = path
            .parent()
            .ok_or_else(|| std::io::Error::other("the socket has no directory"))?;
        users_alone(dir, true)?;
        // A socket that is there already is either in use, or was left by
        // an agent that ended. Anything else is not removed.
        if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_socket()) {
            if UnixStream::connect(path).is_ok() {
                return Err(std::io::Error::new(
                    ErrorKind::AddrInUse,
                    format!(
                        "another agent of this user already answers on {}",
                        path.display()
                    ),
                ));
            }
            std::fs::remove_file(path)?;
        }
        let listener = UnixListener::bind(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(listener)
    }

    /// Answers on the socket until the process ends.
    pub fn serve(listener: UnixListener, answer: Answerer) {
        let open = Arc::new(AtomicUsize::new(0));
        for stream in listener.incoming() {
            let Ok(stream) = stream else {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            };
            if open.fetch_add(1, Ordering::SeqCst) >= MAX_CONVERSATIONS {
                open.fetch_sub(1, Ordering::SeqCst);
                continue;
            }
            let (answer, open) = (answer.clone(), open.clone());
            std::thread::spawn(move || {
                converse(stream, &answer);
                open.fetch_sub(1, Ordering::SeqCst);
            });
        }
    }

    /// One conversation: a line in, a line out, until the other side
    /// closes it or says something that is not a request.
    fn converse(stream: UnixStream, answer: &Answerer) {
        let _ = stream.set_read_timeout(Some(PATIENCE));
        let _ = stream.set_write_timeout(Some(PATIENCE));
        let Ok(mut out) = stream.try_clone() else {
            return;
        };
        let mut lines = BufReader::new(stream);
        loop {
            let mut line = String::new();
            let read = (&mut lines).take(MAX_REQUEST).read_line(&mut line);
            if !matches!(read, Ok(n) if n > 0) || !line.ends_with('\n') {
                return;
            }
            let answer = match decode_line::<HostRequest>(&line) {
                Some(request) => answer(&request),
                None => HostAnswer::Refused {
                    code: ErrorCode::Unsupported,
                    message: "the agent on this machine does not know that request".to_string(),
                },
            };
            if writeln!(out, "{}", encode_line(&answer)).is_err() {
                return;
            }
        }
    }

    /// A conversation with the agent that runs on this machine for this
    /// user.
    pub struct HostLink {
        out: UnixStream,
        lines: BufReader<UnixStream>,
    }

    impl HostLink {
        /// Finds the agent in the places where its socket can be.
        pub fn open(paths: &Paths) -> Result<Self, NoLink> {
            let mut doubt = None;
            for path in paths.agent_sockets() {
                match Self::open_at(&path) {
                    Ok(link) => return Ok(link),
                    Err(NoLink::NoAgent) => {}
                    Err(other) => {
                        doubt.get_or_insert(other);
                    }
                }
            }
            Err(doubt.unwrap_or(NoLink::NoAgent))
        }

        fn open_at(path: &Path) -> Result<Self, NoLink> {
            let dir = path.parent().ok_or(NoLink::NoAgent)?;
            // An answer is only believed from a socket nobody else could
            // have put there.
            match users_alone(dir, false) {
                Ok(()) => {}
                Err(missing) if missing.kind() == ErrorKind::NotFound => {
                    return Err(NoLink::NoAgent)
                }
                Err(why) => {
                    return Err(NoLink::Other(format!(
                        "not asking the agent on this machine: {why}"
                    )))
                }
            }
            let cannot = |why: std::io::Error| {
                NoLink::Other(format!("the agent on this machine cannot be asked: {why}"))
            };
            let out = UnixStream::connect(path).map_err(|why| match why.kind() {
                ErrorKind::NotFound | ErrorKind::ConnectionRefused => NoLink::NoAgent,
                _ => cannot(why),
            })?;
            let read = out
                .set_read_timeout(Some(PATIENCE))
                .and_then(|()| out.set_write_timeout(Some(PATIENCE)))
                .and_then(|()| out.try_clone())
                .map_err(cannot)?;
            Ok(HostLink {
                out,
                lines: BufReader::new(read),
            })
        }

        pub fn ask(&mut self, request: &HostRequest) -> Result<HostAnswer, String> {
            let gone = |why: std::io::Error| {
                format!("the agent on this machine ended the conversation: {why}")
            };
            writeln!(self.out, "{}", encode_line(request)).map_err(gone)?;
            let mut line = String::new();
            let read = (&mut self.lines)
                .take(MAX_ANSWER)
                .read_line(&mut line)
                .map_err(gone)?;
            if read == 0 {
                return Err("the agent on this machine ended the conversation".to_string());
            }
            decode_line(&line).ok_or_else(|| {
                "the agent on this machine answered something this istesso does not understand; \
                 they may be different versions"
                    .to_string()
            })
        }
    }
}

/// Where the agent cannot answer the person at the machine this way.
#[cfg(not(unix))]
pub struct HostLink;

#[cfg(not(unix))]
impl HostLink {
    pub fn open(_paths: &Paths) -> Result<Self, NoLink> {
        Err(NoLink::Other(
            "the agent cannot be asked about its sessions on this system yet".to_string(),
        ))
    }

    pub fn ask(&mut self, _request: &HostRequest) -> Result<HostAnswer, String> {
        Err("the agent cannot be asked about its sessions on this system yet".to_string())
    }
}

#[cfg(test)]
mod tests {
    use istesso_proto::session::{decode_line, encode_line};
    use istesso_proto::StreamKind;

    use super::*;

    fn editor() -> HostSession {
        HostSession {
            info: SessionInfo {
                id: SessionId("5b0c9e3a7f21d684".to_string()),
                app: "editor.desktop".to_string(),
                kind: StreamKind::AppWindow,
                state: SessionState::Detached,
                revision: 4,
                attachment: 1,
                display: None,
                answer: None,
                end: None,
                left_by: None,
                shown_on: None,
            },
            app_name: "Editor".to_string(),
            started_by: StartedBy {
                id: "1a2b-3c4d-5e6f-7a8b".to_string(),
                name: "laptop".to_string(),
            },
            revoked: true,
        }
    }

    #[test]
    fn requests_and_answers_are_single_lines() {
        let id = SessionId("5b0c9e3a7f21d684".to_string());
        assert_eq!(
            encode_line(&HostRequest::Sessions),
            r#"{"v":1,"ask":"sessions"}"#
        );
        assert_eq!(
            encode_line(&HostRequest::Session {
                session: id.clone()
            }),
            r#"{"v":1,"ask":"session","session":"5b0c9e3a7f21d684"}"#
        );
        let stop = HostRequest::Stop {
            session: id.clone(),
            force: true,
        };
        assert_eq!(
            encode_line(&stop),
            r#"{"v":1,"ask":"stop","session":"5b0c9e3a7f21d684","force":true}"#
        );
        assert_eq!(decode_line::<HostRequest>(&encode_line(&stop)), Some(stop));
        // Asking is the default; ending by force has to be said.
        assert_eq!(
            decode_line::<HostRequest>(r#"{"v":1,"ask":"stop","session":"5b0c9e3a7f21d684"}"#),
            Some(HostRequest::Stop {
                session: id,
                force: false
            })
        );
        // Another version, or something this one does not know, is not a
        // request.
        assert_eq!(
            decode_line::<HostRequest>(r#"{"v":2,"ask":"sessions"}"#),
            None
        );
        assert_eq!(
            decode_line::<HostRequest>(r#"{"v":1,"ask":"shutdown"}"#),
            None
        );

        let answer = HostAnswer::Sessions {
            sessions: vec![editor()],
        };
        let line = encode_line(&answer);
        assert!(
            line.starts_with(
                r#"{"v":1,"answer":"sessions","sessions":[{"info":{"id":"5b0c9e3a7f21d684","#
            ),
            "{line}"
        );
        assert!(
            line.ends_with(
                r#""app_name":"Editor","started_by":{"id":"1a2b-3c4d-5e6f-7a8b","name":"laptop"},"revoked":true}]}"#
            ),
            "{line}"
        );
        assert!(!line.contains('\n'));
        assert_eq!(decode_line::<HostAnswer>(&line), Some(answer));

        let one = HostAnswer::Session {
            session: Box::new(editor()),
        };
        assert_eq!(decode_line::<HostAnswer>(&encode_line(&one)), Some(one));
        assert_eq!(
            encode_line(&HostRequest::PairingsChanged),
            r#"{"v":1,"ask":"pairings_changed"}"#
        );
        assert_eq!(encode_line(&HostAnswer::Done), r#"{"v":1,"answer":"done"}"#);
        let refused = HostAnswer::Refused {
            code: ErrorCode::NotFound,
            message: "no such session".to_string(),
        };
        assert_eq!(
            encode_line(&refused),
            r#"{"v":1,"answer":"refused","code":"not_found","message":"no such session"}"#
        );
    }
}
