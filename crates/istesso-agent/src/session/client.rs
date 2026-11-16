//! The client's side of a streaming session: what a device does to open a
//! window for an application that runs on another one.
//!
//! [`open_window`] starts the program that shows the picture (the
//! *viewer*, which belongs to the media path), asks the host to start the
//! application or to show one that is running, hands the viewer what the
//! host's side answered, and then keeps three things in view: what the host
//! says about the session, what the viewer does, and whether the person
//! wants to stop.
//!
//! The application outlives the window. Stopping the command, or a viewer
//! that goes away, *leaves* the session: the picture stops and the
//! application runs on. Closing the window *asks* the application to close;
//! the window stays until the application has ended, because the
//! application may have a question first.
//!
//! For now this runs inside the command that opens the window. It will move
//! into the resident agent when there is a launcher that outlives one
//! command.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use istesso_link::{Client, LocalKeys, RemoteError};
use istesso_pair::{Identity, PeerStore};
use istesso_proto::session::address_text;
use istesso_proto::{
    DisplayMode, EndReason, SessionEnd, SessionId, SessionInfo, SessionPeer, SessionState,
    StreamKind, ViewerCommand, ViewerEvent, ViewerWish,
};

use super::policy;
use super::process::{how_it_ended, LineProcess, Said};
use crate::cache;
use crate::paths::Paths;
use crate::serve::Log;

// ---------------------------------------------------------------------------
// The viewer
// ---------------------------------------------------------------------------

/// Starts the program that shows a session's picture in a window.
pub trait Viewer: Send + Sync {
    fn start(&self) -> Result<Box<dyn RunningViewer>, String>;
}

/// What a viewer did since it was last asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewerSaid {
    Said(ViewerEvent),
    /// It is no longer running. `ok` when it ended without an error, the
    /// way it does when the person quits it.
    Exited {
        ok: bool,
        how: String,
    },
}

/// One viewer, from before its window opens until it is gone.
pub trait RunningViewer: Send {
    fn tell(&mut self, command: &ViewerCommand) -> Result<(), String>;
    /// The next thing it did, without waiting.
    fn poll(&mut self) -> Option<ViewerSaid>;
    /// Closes the window and ends the viewer.
    fn stop(&mut self, grace: Duration);
}

/// A viewer that is a program: the one the person named. It gets no
/// arguments; see the ADR for what it is told and what it says.
pub struct CommandViewer {
    program: PathBuf,
    log: Log,
}

impl CommandViewer {
    pub fn new(program: impl Into<PathBuf>, log: Log) -> Self {
        CommandViewer {
            program: program.into(),
            log,
        }
    }
}

impl Viewer for CommandViewer {
    fn start(&self) -> Result<Box<dyn RunningViewer>, String> {
        let process = LineProcess::spawn(&self.program, "viewer".to_string(), self.log.clone())
            .map_err(|e| format!("the viewer could not be started ({e})"))?;
        Ok(Box::new(ProcessViewer(process)))
    }
}

struct ProcessViewer(LineProcess<ViewerEvent>);

impl RunningViewer for ProcessViewer {
    fn tell(&mut self, command: &ViewerCommand) -> Result<(), String> {
        self.0
            .tell(command)
            .map_err(|e| format!("the viewer does not listen any more ({e})"))
    }

    fn poll(&mut self) -> Option<ViewerSaid> {
        self.0.poll().map(|said| match said {
            Said::Line(event) => ViewerSaid::Said(event),
            Said::Exited(status) => ViewerSaid::Exited {
                ok: status.success(),
                how: how_it_ended(status),
            },
        })
    }

    fn stop(&mut self, grace: Duration) {
        self.0.stop(grace);
    }
}

/// A viewer that is closed when it goes out of scope, whichever way that
/// happens.
struct Window {
    viewer: Box<dyn RunningViewer>,
    grace: Duration,
}

impl Drop for Window {
    fn drop(&mut self) {
        self.viewer.stop(self.grace);
    }
}

// ---------------------------------------------------------------------------
// Reaching the host
// ---------------------------------------------------------------------------

/// Opens a conversation with the host. Called again whenever one is needed:
/// to follow the session, after a conversation broke, and to say something
/// while another conversation waits for an answer.
pub type Link = Arc<dyn Fn() -> Result<Client, String> + Send + Sync>;

/// A [`Link`] to the paired device `host` (its id), at the addresses this
/// device remembers for it.
pub fn link_to(paths: &Paths, host: &str, timeout: Duration) -> Result<Link, String> {
    let identity = Identity::load_or_create(&paths.identity()).map_err(|e| e.to_string())?;
    let keys = Arc::new(LocalKeys::new(&identity).map_err(|e| e.to_string())?);
    let (paths, host) = (paths.clone(), host.to_string());
    Ok(Arc::new(move || {
        // Both are read every time: a pairing can be removed and an address
        // can change while a window is open.
        let peers = Arc::new(PeerStore::open(paths.peers()).map_err(|e| e.to_string())?);
        let addresses = cache::load(&paths, &host)
            .map(|snapshot| snapshot.addresses)
            .unwrap_or_default();
        if addresses.is_empty() {
            return Err("no address is known for the host".to_string());
        }
        crate::survey::connect_any(addresses, keys.clone(), peers, &host, timeout)
            .map(|(client, _)| client)
            .map_err(|e| e.to_string())
    }))
}

/// The times a client goes by.
#[derive(Debug, Clone)]
pub struct ClientRules {
    /// How long a viewer may take to say what it wants.
    pub offer_within: Duration,
    /// How long the host is asked to hold an answer back while nothing
    /// changes. The host may answer sooner.
    pub hold: Duration,
    /// How long an answer may take beyond that.
    pub answer_within: Duration,
    /// How long the host may take to show a session that is running: its
    /// runner has to set the stream up first.
    pub attach_within: Duration,
    /// A host that has not answered for this long is gone. The same time
    /// the host gives a client ([`policy::CLIENT_LOST_AFTER`]): by then it
    /// has stopped the picture anyway.
    pub host_lost_after: Duration,
    /// How long to wait before trying to reach the host again.
    pub retry_every: Duration,
    /// How often the viewer and the person are looked at.
    pub tick: Duration,
    /// How long a viewer gets to close its window.
    pub viewer_stop_grace: Duration,
}

impl Default for ClientRules {
    fn default() -> Self {
        ClientRules {
            offer_within: Duration::from_secs(5),
            hold: Duration::from_secs(2),
            answer_within: Duration::from_secs(5),
            attach_within: Duration::from_secs(15),
            host_lost_after: policy::CLIENT_LOST_AFTER,
            retry_every: Duration::from_millis(500),
            tick: Duration::from_millis(50),
            viewer_stop_grace: Duration::from_secs(2),
        }
    }
}

/// Why nothing came of a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// The viewer did not start, or did not say what it wants.
    Viewer(String),
    /// The host could not be reached.
    Unreachable(String),
    /// The host answered that it will not, or cannot.
    Refused(RemoteError),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Viewer(why) | OpenError::Unreachable(why) => f.write_str(why),
            OpenError::Refused(refusal) => write!(f, "{}", refusal.message),
        }
    }
}

fn failed(error: istesso_link::Error) -> OpenError {
    match error {
        istesso_link::Error::Remote(refusal) => OpenError::Refused(refusal),
        other => OpenError::Unreachable(other.to_string()),
    }
}

/// The sessions this device has on a host that are not over, oldest first.
pub fn sessions_on(link: &Link, rules: &ClientRules) -> Result<Vec<SessionInfo>, OpenError> {
    let mut client = link().map_err(OpenError::Unreachable)?;
    client.set_timeout(rules.answer_within).map_err(failed)?;
    client.session_list().map_err(failed)
}

/// Asks the application of a session to close, or with `force` ends it,
/// and then watches for up to `watch` what becomes of the session. The
/// last report says: `ended` when it is over; anything else when the
/// application is still there, which after a request to close means that
/// it may be asking something.
pub fn stop_session(
    link: &Link,
    session: &SessionId,
    force: bool,
    watch: Duration,
    rules: &ClientRules,
) -> Result<SessionInfo, OpenError> {
    let mut client = link().map_err(OpenError::Unreachable)?;
    client.set_timeout(rules.answer_within).map_err(failed)?;
    let mut info = client.session_stop(session, force).map_err(failed)?;
    client
        .set_timeout(rules.hold + rules.answer_within)
        .map_err(failed)?;
    let until = Instant::now() + watch;
    while info.state != SessionState::Ended {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        // Only asking: this is not the client that is shown the session.
        info = client
            .session_wait(session, info.revision, left.min(rules.hold), 0)
            .map_err(failed)?;
    }
    Ok(info)
}

// ---------------------------------------------------------------------------
// Opening a window
// ---------------------------------------------------------------------------

/// What is to be opened.
#[derive(Debug, Clone)]
pub struct Opening {
    /// The id of the paired device that has the application.
    pub host: String,
    /// The application's id on that device.
    pub app: String,
    /// For the window's title.
    pub title: String,
    pub kind: StreamKind,
    /// What the person asked for; the viewer decides the rest.
    pub wish: ViewerWish,
    /// A session of the application that already runs on the host, to be
    /// shown again. Without one the application is started.
    pub session: Option<SessionId>,
}

/// Why a window that was open is closed again. Only [`Outcome::Ended`]
/// means that the application is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The session is over, and the host says why.
    Ended(SessionEnd),
    /// The person stopped the command. The application runs on.
    Left(SessionId),
    /// The viewer went away: the person quit it (`ok`), or it broke. The
    /// application runs on.
    ViewerGone {
        session: SessionId,
        ok: bool,
        how: String,
    },
    /// The host stopped showing the session to this client: it had not
    /// heard from it for too long, or its right was taken away. The
    /// application runs on.
    Detached(SessionId),
    /// Another window is shown the session now.
    TakenOver {
        session: SessionId,
        /// The device that is shown the session now, as the host calls it.
        by: Option<String>,
    },
    /// The host stopped answering, or no longer knows the session.
    HostLost { session: SessionId, why: String },
}

/// What a person would want to know while a window opens and stays open.
#[derive(Debug)]
pub enum Progress<'a> {
    /// The host said something new about the session.
    Session(&'a SessionInfo),
    /// The person wanted to close the window, and the application was asked
    /// to close. The window stays for its answer.
    AskedToClose,
}

enum HostSaid {
    Report(SessionInfo),
    Lost(String),
}

/// Opens a window for an application of another device and stays until the
/// window is closed again.
///
/// `interrupted` is set from outside when the person wants to stop the
/// command.
pub fn open_window(
    link: Link,
    viewer: &dyn Viewer,
    opening: &Opening,
    interrupted: &AtomicBool,
    rules: &ClientRules,
    progress: &mut dyn FnMut(Progress<'_>),
) -> Result<Outcome, OpenError> {
    // The viewer first: it knows the display the window will be on, and so
    // which picture to ask the host for.
    let mut window = Window {
        viewer: viewer.start().map_err(OpenError::Viewer)?,
        grace: rules.viewer_stop_grace,
    };
    window
        .viewer
        .tell(&ViewerCommand::Prepare {
            title: opening.title.clone(),
            kind: opening.kind,
            wish: opening.wish,
        })
        .map_err(OpenError::Viewer)?;
    let (wanted, offer) = offer_of(window.viewer.as_mut(), rules.offer_within)?;

    let mut client = link().map_err(OpenError::Unreachable)?;
    client.set_timeout(rules.answer_within).map_err(failed)?;
    // Where the conversation goes is where the picture comes from.
    let host = SessionPeer {
        id: opening.host.clone(),
        address: address_text(&client.remote_addr().map_err(failed)?),
    };
    let first = match &opening.session {
        None => client
            .session_start(&opening.app, opening.kind, wanted, offer)
            .map_err(failed)?,
        Some(session) => {
            // Answered when the host's side sends the picture here.
            client.set_timeout(rules.attach_within).map_err(failed)?;
            client
                .session_attach(session, wanted, offer)
                .map_err(failed)?
        }
    };
    progress(Progress::Session(&first));
    let session = first.id.clone();
    // The number this window has among those that were shown the session.
    let mine = first.attachment;
    let show = |info: &SessionInfo| ViewerCommand::Show {
        session: info.id.clone(),
        host: host.clone(),
        display: info.display.unwrap_or(wanted),
        answer: info.answer.clone().unwrap_or_default(),
    };
    let mut shown = false;
    let mut leave = false;
    let mut early: Option<Outcome> = None;
    if first.state == SessionState::Running {
        shown = true;
        if let Err(why) = window.viewer.tell(&show(&first)) {
            leave = true;
            early = Some(Outcome::ViewerGone {
                session: session.clone(),
                ok: false,
                how: why,
            });
        }
    }

    // One thread keeps asking the host; this one looks at the viewer and
    // the person in between.
    let (sender, reports) = mpsc::channel();
    let quit = Arc::new(AtomicBool::new(false));
    let follower = {
        let (link, session, rules, quit) =
            (link.clone(), session.clone(), rules.clone(), quit.clone());
        let after = first.revision;
        std::thread::spawn(move || {
            follow(client, link, &session, after, mine, &rules, &quit, sender)
        })
    };

    let outcome = loop {
        if let Some(outcome) = early.take() {
            break outcome;
        }
        match reports.recv_timeout(rules.tick) {
            Ok(HostSaid::Report(info)) => match info.state {
                SessionState::Stopping | SessionState::Ended => {
                    progress(Progress::Session(&info));
                    break Outcome::Ended(info.end.unwrap_or(SessionEnd {
                        reason: EndReason::Other,
                        message: String::new(),
                    }));
                }
                // A newer window has it. This one has nothing to leave.
                _ if info.attachment != mine => {
                    break Outcome::TakenOver {
                        session: session.clone(),
                        by: info.shown_on.clone(),
                    }
                }
                SessionState::Running if !shown => {
                    shown = true;
                    if let Err(why) = window.viewer.tell(&show(&info)) {
                        leave = true;
                        break Outcome::ViewerGone {
                            session: session.clone(),
                            ok: false,
                            how: why,
                        };
                    }
                    progress(Progress::Session(&info));
                }
                SessionState::Detached => break Outcome::Detached(session.clone()),
                _ => {}
            },
            Ok(HostSaid::Lost(why)) => {
                break Outcome::HostLost {
                    session: session.clone(),
                    why,
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                leave = true;
                break Outcome::HostLost {
                    session: session.clone(),
                    why: "the host is no longer followed".into(),
                };
            }
        }
        if interrupted.load(Ordering::SeqCst) {
            // Stopping the command is not closing the application: what
            // the person was doing stays where it is.
            leave = true;
            break Outcome::Left(session.clone());
        }
        match window.viewer.poll() {
            // The person wants the window closed. The application is asked;
            // the window stays, because the application may ask back.
            Some(ViewerSaid::Said(ViewerEvent::Close)) => {
                let asked = link().and_then(|mut client| {
                    client
                        .set_timeout(rules.answer_within)
                        .and_then(|_| client.session_stop(&session, false))
                        .map_err(|e| e.to_string())
                });
                if asked.is_ok() {
                    progress(Progress::AskedToClose);
                }
            }
            Some(ViewerSaid::Exited { ok, how }) => {
                leave = true;
                break Outcome::ViewerGone {
                    session: session.clone(),
                    ok,
                    how,
                };
            }
            Some(ViewerSaid::Said(ViewerEvent::Error { message })) => {
                leave = true;
                break Outcome::ViewerGone {
                    session: session.clone(),
                    ok: false,
                    how: message,
                };
            }
            _ => {}
        }
    };

    quit.store(true, Ordering::SeqCst);
    drop(window);
    if leave {
        // Said on a conversation of its own: the follower's is waiting for
        // an answer, and gets one as soon as the host has heard this.
        if let Ok(mut client) = link() {
            let _ = client.set_timeout(rules.answer_within);
            let _ = client.session_detach(&session, mine);
        }
    }
    let _ = follower.join();
    Ok(outcome)
}

/// Waits for the viewer to say which picture it wants.
fn offer_of(
    viewer: &mut dyn RunningViewer,
    within: Duration,
) -> Result<(DisplayMode, serde_json::Value), OpenError> {
    let deadline = Instant::now() + within;
    loop {
        match viewer.poll() {
            Some(ViewerSaid::Said(ViewerEvent::Offer { display, offer })) => {
                return Ok((display, offer))
            }
            Some(ViewerSaid::Said(ViewerEvent::Error { message })) => {
                return Err(OpenError::Viewer(message))
            }
            Some(ViewerSaid::Exited { how, .. }) => {
                return Err(OpenError::Viewer(format!(
                    "the viewer ended before it said what it wants ({how})"
                )))
            }
            Some(ViewerSaid::Said(_)) => continue,
            None => {}
        }
        if Instant::now() >= deadline {
            return Err(OpenError::Viewer(format!(
                "the viewer did not say what it wants within {} seconds",
                within.as_secs_f32()
            )));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Keeps asking the host about the session as attachment `mine`, which
/// also tells the host that this client is still there, and passes on what
/// changes.
#[allow(clippy::too_many_arguments)]
fn follow(
    mut client: Client,
    link: Link,
    session: &SessionId,
    mut after: u64,
    mine: u64,
    rules: &ClientRules,
    quit: &AtomicBool,
    said: Sender<HostSaid>,
) {
    let patience = rules.hold + rules.answer_within;
    let _ = client.set_timeout(patience);
    let mut heard = Instant::now();
    while !quit.load(Ordering::SeqCst) {
        match client.session_wait(session, after, rules.hold, mine) {
            Ok(info) => {
                heard = Instant::now();
                let news = info.revision > after;
                after = info.revision;
                let over = info.state == SessionState::Ended;
                let nobody_listens = news && said.send(HostSaid::Report(info)).is_err();
                if nobody_listens || over {
                    return;
                }
            }
            // The host answers, and does not have the session (any more),
            // or does not let this device ask.
            Err(istesso_link::Error::Remote(refusal)) => {
                let _ = said.send(HostSaid::Lost(format!(
                    "the host no longer lets this device see the session ({})",
                    refusal.message
                )));
                return;
            }
            // The conversation broke. Another one is opened for as long as
            // the host keeps sending the picture.
            Err(trouble) => loop {
                if quit.load(Ordering::SeqCst) {
                    return;
                }
                if heard.elapsed() > rules.host_lost_after {
                    let _ = said.send(HostSaid::Lost(format!(
                        "the host has not answered for {} seconds ({trouble})",
                        rules.host_lost_after.as_secs_f32()
                    )));
                    return;
                }
                match link() {
                    Ok(fresh) => {
                        client = fresh;
                        let _ = client.set_timeout(patience);
                        break;
                    }
                    Err(_) => std::thread::sleep(rules.retry_every),
                }
            },
        }
    }
}

#[cfg(test)]
mod tests;
