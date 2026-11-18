//! Streaming sessions: an application of this host, shown as a window on
//! another device (`docs/adr/0010-session-start.md`).
//!
//! This is the host's side of the control plane. A paired device asks for an
//! application by its id; the agent starts the program that makes the
//! picture (the *runner*, which belongs to the media path), then the
//! application inside the runner's display, and follows both.
//!
//! A session lasts until its application ends or its owner ends it. Whether
//! a client is shown it has nothing to do with that: a client that goes
//! away is *detached*, the picture stops, and the application runs on until
//! a client attaches again. How the picture and the input travel is not
//! known here.
//!
//! A session belongs to the device that started it. When that device is
//! unpaired, or loses a right that starting takes, the session is closed to
//! it: nothing is shown to it and it cannot ask about the session any more.
//! The application is not ended for that. A session whose device was
//! unpaired is marked and from then on open to every device that may start
//! applications here. The person at the host sees every session of the
//! machine and can end each one ([`local`]).
//!
//! * [`Sessions`] is the table: it answers the session requests and keeps
//!   one supervising thread per session.
//! * [`Runner`] and [`Launcher`] are the two things a session is made of.
//!   The real ones start processes ([`CommandRunner`], [`DesktopLauncher`]);
//!   tests put their own in.
//! * [`client`] is the other end: what a device does to open a window.

pub mod client;
pub mod local;
mod process;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use istesso_catalog::launch::LaunchPlan;
use istesso_link::{ErrorCode, Request, Response};
use istesso_pair::{Peer, PeerStore, Permission};
use istesso_proto::session::address_text;
use istesso_proto::{
    AppEntry, DisplayMode, EndReason, PauseReason, RunnerCommand, RunnerEvent, SessionEnd,
    SessionId, SessionInfo, SessionPeer, SessionState, StreamKind,
};
use rand_core::{OsRng, RngCore};

use crate::paths::Paths;
use crate::serve::Log;

pub use local::{HostAnswer, HostRequest, HostSession, StartedBy};
pub use process::{app_environment, CommandRunner, DesktopLauncher};

/// The rules a person decided, or has yet to decide (ADR 0010). Each is
/// written down here and nowhere else.
pub mod policy {
    use std::time::Duration;

    use istesso_pair::Permission;

    /// What a device needs to start an application on this host, and to be
    /// shown one: to look and to control. A newly paired device has both.
    pub const START_NEEDS: [Permission; 2] = [Permission::View, Permission::Control];

    /// A client that is shown a session and has not asked about it for this
    /// long is gone: the picture stops going to it. The application runs
    /// on. A client gives a host that does not answer the same time before
    /// it closes its window.
    pub const CLIENT_LOST_AFTER: Duration = Duration::from_secs(15);

    /// How many sessions a host keeps open at the same time, shown to
    /// someone or not, unless it is set otherwise (`istesso agent
    /// --max-sessions`). This is not what limits everyday use: a person
    /// opens more than a handful of applications of one machine as a
    /// matter of course. It is a safety value against a script or a fault
    /// that starts applications without end. Every session is a runner
    /// with a display of its own and an application; sixty-four of them
    /// is several times what a person keeps open and still far from what
    /// brings a host down. What really runs out is the number of pictures
    /// a host can compress at once, which is counted separately (ADR 0010,
    /// section 15).
    pub const OPEN_SESSIONS: usize = 64;

    /// How many pictures a host compresses at the same time, unless it is
    /// set otherwise (`istesso agent --encoder-slots`). This, not the
    /// number of open sessions, is what runs out. NVIDIA's NVENC
    /// Application Note gives a GeForce 8 sessions per system in the
    /// editions for Video Codec SDK 12.2 and 13.0 and 12 in the edition
    /// for 13.1; which holds depends on the driver and has not been tried
    /// on a host yet, so the smaller number is the default. A session
    /// that finds no slot is opened all the same, with its picture
    /// stopped (ADR 0010, section 15).
    pub const ENCODER_SLOTS: usize = 8;

    /// Whether a device may see, attach to and stop a session of this
    /// host. `rights` is what the list of paired devices gives the device
    /// at the moment it asks.
    ///
    /// Every device that has what starting takes may, whichever device
    /// started the session: an application that was opened at home is
    /// carried on from a device elsewhere (the person's decision of
    /// 2026-11-16). Nobody may without the rights. One device is shown a
    /// session at a time.
    pub fn may_use(rights: &[Permission]) -> bool {
        START_NEEDS.iter().all(|need| rights.contains(need))
    }
}

/// The largest `offer` or `answer` that is carried, as JSON text.
pub const MAX_MEDIA_BYTES: usize = 64 * 1024;

/// The times and limits a host goes by.
#[derive(Debug, Clone)]
pub struct Rules {
    /// See [`policy::CLIENT_LOST_AFTER`].
    pub client_lost_after: Duration,
    /// How long a runner may take to say that it is ready.
    pub runner_ready_within: Duration,
    /// How long a runner may take to answer an `attach`.
    pub attach_within: Duration,
    /// How long an application that was asked to end (SIGTERM) is waited
    /// for, with its display still there, before what is left is left as
    /// it is (ADR 0010, 3). Never followed by force.
    pub app_end_wait: Duration,
    /// How long an application that is ended by force gets to end by
    /// itself.
    pub app_stop_grace: Duration,
    /// How long a runner gets to end after its input was closed, and again
    /// after it was told to.
    pub runner_stop_grace: Duration,
    /// How long a session that is over is still reported, so that a late
    /// question gets the reason.
    pub remember_ended_for: Duration,
    /// The longest an answer to `session_wait` is held back. Shorter than
    /// the agent waits for a caller's next message.
    pub wait_at_most: Duration,
    /// Sessions that may be open at the same time, shown to someone or
    /// not. See [`policy::OPEN_SESSIONS`].
    pub max_sessions: usize,
    /// Pictures that are compressed at the same time. See
    /// [`policy::ENCODER_SLOTS`].
    pub encoder_slots: usize,
    /// After an encoder was refused, how long the host makes fewer
    /// pictures than it is set to before it tries its setting again.
    pub encoder_retry_after: Duration,
    /// After an encoder was refused, how long a runner is left alone
    /// before it is told to try again: the window that gave its slot up
    /// needs a moment to close its encoder.
    pub encoder_settle: Duration,
    /// How often a session is looked after.
    pub tick: Duration,
    /// How often the list of paired devices is read again for a session.
    pub recheck_rights_every: Duration,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            client_lost_after: policy::CLIENT_LOST_AFTER,
            runner_ready_within: Duration::from_secs(15),
            attach_within: Duration::from_secs(10),
            app_end_wait: Duration::from_secs(5),
            app_stop_grace: Duration::from_secs(3),
            runner_stop_grace: Duration::from_secs(2),
            remember_ended_for: Duration::from_secs(60),
            wait_at_most: Duration::from_secs(3),
            max_sessions: policy::OPEN_SESSIONS,
            encoder_slots: policy::ENCODER_SLOTS,
            encoder_retry_after: Duration::from_secs(60),
            encoder_settle: Duration::from_millis(500),
            tick: Duration::from_millis(100),
            recheck_rights_every: Duration::from_secs(1),
        }
    }
}

// ---------------------------------------------------------------------------
// What a session is made of
// ---------------------------------------------------------------------------

/// Starts the host's side of a stream, once per session.
pub trait Runner: Send + Sync {
    /// `start` is always [`RunnerCommand::Start`]. An error is one line for
    /// the device that asked.
    fn start(&self, start: &RunnerCommand) -> Result<Box<dyn RunningStream>, String>;
}

/// What a stream's host side did since it was last asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    Said(RunnerEvent),
    /// It is no longer running; with how it ended.
    Exited(String),
}

/// The host's side of one stream, from its start to the end of the session.
pub trait RunningStream: Send {
    /// The next thing it did, without waiting.
    fn poll(&mut self) -> Option<StreamEvent>;
    /// Tells it what comes after the start: a client attaches, the client
    /// is gone, the application should close.
    fn tell(&mut self, command: &RunnerCommand) -> Result<(), String>;
    /// Ends it: the agreed way first, by force when that takes longer than
    /// `grace`.
    fn stop(&mut self, grace: Duration);
}

/// Starts this host's applications.
pub trait Launcher: Send + Sync {
    /// How the application is started, or one line that says why it cannot
    /// be shown as a window of its own.
    fn plan(&self, app: &AppEntry) -> Result<LaunchPlan, String>;
    /// Starts it with `env`, which a runner gave, added to its environment.
    fn launch(
        &self,
        plan: &LaunchPlan,
        env: &BTreeMap<String, String>,
    ) -> Result<Box<dyn RunningApp>, String>;
}

/// An application that was started for a session.
pub trait RunningApp: Send {
    /// The process that was started, where there is one.
    fn pid(&self) -> Option<u32> {
        None
    }
    /// The process group the application and what it started are in,
    /// where there is one: what a person is told when it stays.
    fn group(&self) -> Option<u32> {
        None
    }
    /// `Some` once it has ended by itself, with how.
    fn poll(&mut self) -> Option<String>;
    /// Asks it to end (SIGTERM to its group), once, and returns at once.
    /// Never by force: what does not listen stays (ADR 0010, 3).
    fn ask_to_end(&mut self);
    /// Ends it by force: asks, and makes it end (SIGKILL) when that takes
    /// longer than `grace`. Only ever on a person's `force`.
    fn end_by_force(&mut self, grace: Duration);
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// The sessions of this host.
#[derive(Clone)]
pub struct Sessions {
    inner: Arc<Inner>,
}

struct Inner {
    paths: Paths,
    log: Log,
    runner: Option<Arc<dyn Runner>>,
    launcher: Arc<dyn Launcher>,
    rules: Rules,
    table: Mutex<Table>,
    /// Signalled whenever a session changes or is asked to do something.
    changed: Condvar,
    /// Goes up when somebody says that the list of paired devices changed,
    /// so that no session waits for its next reading of it.
    pairings_epoch: AtomicU64,
}

#[derive(Default)]
struct Table {
    sessions: HashMap<String, Entry>,
    /// How many sessions were started, to list them in that order.
    started: u64,
    /// Set after an encoder was refused: how many pictures the host makes
    /// at once for the time being, fewer than it is set to, and since
    /// when. The setting is tried again after a while.
    fewer: Option<(usize, Instant)>,
}

struct Entry {
    info: SessionInfo,
    app_name: String,
    /// The device that started it.
    owner: Device,
    /// The device that is shown it, or was the last to be.
    viewer: Device,
    /// That device said that its window cannot be seen.
    hidden: bool,
    /// When a client was last attached, or its person last turned to the
    /// window: the windows that were turned to last keep their picture
    /// when there are more of them than encoder slots.
    touched: Instant,
    number: u64,
    /// When the client that is shown the session last asked about it.
    seen: Instant,
    /// Questions of that client that are being held back right now.
    waiting: usize,
    /// What the supervisor is to do next.
    orders: Orders,
    /// How the `attach` orders went, by their number, until the device that
    /// gave them has been told.
    attached: Vec<(u64, Result<(), String>)>,
    /// The device that started it is no longer paired, or lacks a right
    /// that starting takes. As the supervisor last found it.
    revoked: bool,
    ended_at: Option<Instant>,
}

/// A device as a session remembers it.
#[derive(Clone)]
struct Device {
    id: String,
    public_key: String,
    /// What it was called at the time. It may not be in the list of paired
    /// devices any more.
    name: String,
}

impl Device {
    fn of(peer: &Peer) -> Self {
        Device {
            id: peer.id.clone(),
            public_key: peer.public_key.clone(),
            name: peer.name.clone(),
        }
    }
}

/// What the list of paired devices says about a device at this moment:
/// read from disk, not remembered from when a conversation or a session
/// began.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Pairing {
    /// The list cannot be read: it is not there, it is damaged, or the
    /// system does not hand it out. Nothing is known. That is not an
    /// unpairing: nothing is allowed, and nothing is concluded.
    Unknown,
    /// The list was read, and the device is not in it.
    Unpaired,
    /// The list was read, and holds the device with these rights.
    Paired(Vec<Permission>),
}

/// Reads the list of paired devices. `None` when it cannot be read.
///
/// A list that is not there counts as one that cannot be read. Taking the
/// last pairing away leaves an empty list behind, not none, so a missing
/// file says that something went wrong and not that everyone was
/// unpaired.
pub(crate) fn read_peers(paths: &Paths) -> Option<PeerStore> {
    let path = paths.peers();
    if !path.is_file() {
        return None;
    }
    let peers = PeerStore::open(&path).ok()?;
    // The file can go between the look and the reading, which then gives
    // an empty list. An empty list is believed when the file is there.
    (!peers.peers().is_empty() || path.is_file()).then_some(peers)
}

pub(crate) fn pairing(peers: Option<&PeerStore>, id: &str, public_key: &str) -> Pairing {
    let Some(peers) = peers else {
        return Pairing::Unknown;
    };
    let known = peers
        .peers()
        .iter()
        .find(|known| known.id == id && known.public_key == public_key);
    match known {
        None => Pairing::Unpaired,
        Some(known) => Pairing::Paired(known.permissions.clone()),
    }
}

/// [`pairing`], from a reading of the list that is made now.
pub(crate) fn pairing_now(paths: &Paths, id: &str, public_key: &str) -> Pairing {
    pairing(read_peers(paths).as_ref(), id, public_key)
}

/// What a device may do with sessions on this host at this moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// It is paired and has what starting takes.
    Allowed,
    /// It is paired and lacks a right that starting takes.
    Barred,
    /// The list of paired devices was read, and it is not in it.
    Unpaired,
    /// The list of paired devices cannot be read.
    Unknown,
}

fn standing(peers: Option<&PeerStore>, device: &Device) -> Standing {
    match pairing(peers, &device.id, &device.public_key) {
        Pairing::Unknown => Standing::Unknown,
        Pairing::Unpaired => Standing::Unpaired,
        Pairing::Paired(rights) if may_start(&rights) => Standing::Allowed,
        Pairing::Paired(_) => Standing::Barred,
    }
}

/// When a session's supervisor last read what devices may do.
#[derive(Default)]
struct Checked {
    at: Option<Instant>,
    epoch: u64,
}

/// A device that asks about a session, with what the list of paired
/// devices gives it at that moment.
#[derive(Clone, Copy)]
struct Caller<'a> {
    peer: &'a Peer,
    rights: &'a [Permission],
}

/// Who asks about a session.
#[derive(Clone, Copy)]
enum Asker<'a> {
    /// A paired device, on the control channel.
    Device(Caller<'a>),
    /// The person at this machine ([`local`]).
    Host,
}

/// Why a device that lost its rights is not shown a session.
const NOT_ALLOWED: &str = "its device may no longer use applications of this host";
/// Why an application is not started for such a device.
const NOT_STARTED: &str =
    "the device that asked for it may no longer start applications on this host";
/// Why nothing is allowed while the list of paired devices cannot be read.
const CANNOT_READ: &str = "the host cannot read its list of paired devices right now";

#[derive(Default)]
struct Orders {
    /// End the application.
    force: bool,
    /// Ask the application to close.
    close: bool,
    /// The client that is shown the session leaves.
    detach: bool,
    /// Show the session to this client.
    attach: Option<Attach>,
}

struct Attach {
    number: u64,
    /// The device that asks to be shown the session.
    device: Device,
    peer: SessionPeer,
    display: DisplayMode,
    offer: serde_json::Value,
}

/// What a device asked for with `session_start`.
struct Asked<'a> {
    app: &'a str,
    kind: StreamKind,
    display: DisplayMode,
    offer: &'a serde_json::Value,
}

fn refuse(code: ErrorCode, message: impl Into<String>) -> Response {
    Response::error(code, message)
}

fn report(info: &SessionInfo) -> Response {
    Response::Session {
        session: Box::new(info.clone()),
    }
}

/// A session's report for a device that asks about it. What the media path
/// agreed on is for the client that is shown the session and for nobody
/// else: a session can be open to several devices.
fn report_to(info: &SessionInfo, shown: bool) -> Response {
    let mut info = info.clone();
    if !shown {
        info.answer = None;
    }
    Response::Session {
        session: Box::new(info),
    }
}

fn ending(reason: EndReason, message: impl Into<String>) -> SessionEnd {
    SessionEnd {
        reason,
        message: message.into(),
    }
}

fn may_start(rights: &[Permission]) -> bool {
    policy::START_NEEDS.iter().all(|need| rights.contains(need))
}

fn fits(media: &serde_json::Value) -> bool {
    serde_json::to_vec(media).is_ok_and(|text| text.len() <= MAX_MEDIA_BYTES)
}

/// Why a picture of this size and rate cannot be asked for, if it cannot.
fn unreasonable(display: DisplayMode) -> Option<String> {
    let DisplayMode { width, height, fps } = display;
    let sane = (16..=16384).contains(&width)
        && (16..=16384).contains(&height)
        && (1..=1000).contains(&fps);
    (!sane)
        .then(|| format!("a picture of {width}x{height} at {fps} frames a second cannot be made"))
}

/// True for the requests this table answers.
pub fn is_session_request(request: &Request) -> bool {
    matches!(
        request,
        Request::SessionStart { .. }
            | Request::SessionWait { .. }
            | Request::SessionStop { .. }
            | Request::SessionList
            | Request::SessionAttach { .. }
            | Request::SessionDetach { .. }
            | Request::SessionShow { .. }
    )
}

impl Sessions {
    /// A table for a host that has no runner: it refuses to start sessions
    /// and says why.
    pub fn new(paths: Paths, log: Log) -> Self {
        Self::with(
            paths,
            log,
            None,
            Arc::new(DesktopLauncher),
            Rules::default(),
        )
    }

    pub fn with(
        paths: Paths,
        log: Log,
        runner: Option<Arc<dyn Runner>>,
        launcher: Arc<dyn Launcher>,
        rules: Rules,
    ) -> Self {
        Sessions {
            inner: Arc::new(Inner {
                paths,
                log,
                runner,
                launcher,
                rules,
                table: Mutex::new(Table::default()),
                changed: Condvar::new(),
                pairings_epoch: AtomicU64::new(0),
            }),
        }
    }

    /// Answers the session requests ([`is_session_request`]).
    ///
    /// `peer` is the device that asks, as its handshake proved it, and
    /// `remote` is where its connection comes from. `apps` lists this
    /// host's applications; it is only called for `session_start`.
    pub fn answer(
        &self,
        request: &Request,
        peer: &Peer,
        remote: SocketAddr,
        apps: impl FnOnce() -> Vec<AppEntry>,
    ) -> Response {
        self.inner.forget_old();
        // Looked up now, not when the conversation began: a conversation
        // can stay open for as long as a window does.
        let rights = match self.admit(request, peer) {
            Ok(rights) => rights,
            Err((code, why)) => return refuse(code, why),
        };
        let caller = Caller {
            peer,
            rights: &rights,
        };
        let response = match request {
            Request::SessionStart {
                app,
                kind,
                display,
                offer,
            } => {
                let asked = Asked {
                    app,
                    kind: *kind,
                    display: *display,
                    offer,
                };
                self.start(caller, remote, &asked, apps)
            }
            Request::SessionAttach {
                session,
                display,
                offer,
            } => self.attach(caller, remote, session, *display, offer),
            Request::SessionWait {
                session,
                after,
                timeout_ms,
                attachment,
            } => self.wait(caller, session, *after, *timeout_ms, *attachment),
            Request::SessionDetach {
                session,
                attachment,
            } => self.detach(caller, session, *attachment),
            Request::SessionStop { session, force } => {
                self.stop(Asker::Device(caller), session, *force)
            }
            Request::SessionList => self.list(caller),
            Request::SessionShow {
                session,
                attachment,
                visible,
            } => self.show(caller, session, *attachment, *visible),
            _ => refuse(ErrorCode::Unsupported, "not a question about a session"),
        };
        // These two are held back, for seconds. They are answered only to
        // a device that still has the rights when the answer is ready.
        if matches!(
            request,
            Request::SessionWait { .. } | Request::SessionAttach { .. }
        ) {
            if let Err((code, why)) = self.admit(request, peer) {
                return refuse(code, why);
            }
        }
        response
    }

    /// What `peer` may do at this moment, if it may make `request`: every
    /// request about a session takes what starting takes, whichever
    /// session it is about. A device that lost a right is not shown
    /// sessions, and does not follow, list or end them either. A list of
    /// paired devices that cannot be read admits nobody.
    fn admit(
        &self,
        request: &Request,
        peer: &Peer,
    ) -> Result<Vec<Permission>, (ErrorCode, &'static str)> {
        match pairing_now(&self.inner.paths, &peer.id, &peer.public_key) {
            Pairing::Unknown => Err((ErrorCode::Unavailable, CANNOT_READ)),
            Pairing::Unpaired => Err((
                ErrorCode::Forbidden,
                "this device is no longer paired with the host",
            )),
            Pairing::Paired(rights) if may_start(&rights) => Ok(rights),
            Pairing::Paired(_) => Err((
                ErrorCode::Forbidden,
                match request {
                    Request::SessionStart { .. } => {
                        "this device may not start applications on the host"
                    }
                    _ => {
                        "this device may not use applications of the host: \
                         that takes the right to look and to control"
                    }
                },
            )),
        }
    }

    fn start(
        &self,
        caller: Caller<'_>,
        remote: SocketAddr,
        asked: &Asked<'_>,
        apps: impl FnOnce() -> Vec<AppEntry>,
    ) -> Response {
        let inner = &self.inner;
        let peer = caller.peer;
        if asked.kind != StreamKind::AppWindow {
            return refuse(
                ErrorCode::Unavailable,
                "only application windows can be shown so far",
            );
        }
        if let Some(why) = unreasonable(asked.display) {
            return refuse(ErrorCode::Unavailable, why);
        }
        if !fits(asked.offer) {
            return refuse(
                ErrorCode::Unavailable,
                "the offer of the media path is larger than 64 KiB",
            );
        }
        let Some(runner) = inner.runner.clone() else {
            return refuse(
                ErrorCode::Unavailable,
                "this host has no session runner set up, so it cannot show applications to other devices",
            );
        };
        // Only what this host lists itself can be asked for, by its id.
        let Some(app) = apps().into_iter().find(|app| app.id == asked.app) else {
            return refuse(ErrorCode::NotFound, "no such application on this host");
        };
        let plan = match inner.launcher.plan(&app) {
            Ok(plan) => plan,
            Err(why) => return refuse(ErrorCode::Unavailable, why),
        };

        let id = SessionId(format!("{:016x}", OsRng.next_u64()));
        let input_socket = inner.paths.session_input_socket(&id);
        let owner = Device::of(peer);
        let info = SessionInfo {
            id: id.clone(),
            app: app.id.clone(),
            kind: asked.kind,
            state: SessionState::Starting,
            revision: 1,
            // The device that starts a session is the first to be shown it.
            attachment: 1,
            display: None,
            answer: None,
            end: None,
            left_by: None,
            shown_on: None,
            paused: None,
            opening: false,
            left_over: None,
        };
        {
            let mut table = inner.lock();
            if let Some(full) = table.full_for(caller, &inner.rules) {
                return refuse(ErrorCode::Unavailable, full);
            }
            table.started += 1;
            let number = table.started;
            table.sessions.insert(
                id.0.clone(),
                Entry {
                    info: info.clone(),
                    app_name: app.name.clone(),
                    owner: owner.clone(),
                    // The device that starts a session is the first to be
                    // shown it.
                    viewer: owner.clone(),
                    hidden: false,
                    touched: Instant::now(),
                    number,
                    seen: Instant::now(),
                    waiting: 0,
                    orders: Orders::default(),
                    attached: Vec::new(),
                    revoked: false,
                    ended_at: None,
                },
            );
        }
        (inner.log)(format!(
            "session {id}: starting {} for {}",
            app.name, peer.name
        ));

        let job = Job {
            id,
            owner,
            app_name: app.name,
            runner,
            plan,
            start: RunnerCommand::Start {
                session: info.id.clone(),
                kind: asked.kind,
                display: asked.display,
                // Where the conversation comes from, not what the device
                // says about itself: the picture is sent nowhere else.
                peer: SessionPeer {
                    id: peer.id.clone(),
                    address: address_text(&remote),
                    public_key: Some(peer.public_key.clone()),
                },
                input_socket: input_socket.to_string_lossy().into_owned(),
                offer: asked.offer.clone(),
            },
            input_socket,
        };
        let inner = self.inner.clone();
        std::thread::spawn(move || inner.supervise(job));
        report(&info)
    }

    fn list(&self, caller: Caller<'_>) -> Response {
        let table = self.inner.lock();
        let mut mine: Vec<&Entry> = table
            .sessions
            .values()
            .filter(|e| e.ended_at.is_none() && e.is_for(caller))
            .collect();
        mine.sort_by_key(|e| e.number);
        Response::Sessions {
            sessions: mine
                .into_iter()
                .map(|e| {
                    // A list is not the place for what the media path
                    // agreed on with the client that is shown the session.
                    let mut info = e.info.clone();
                    info.answer = None;
                    info
                })
                .collect(),
        }
    }

    fn wait(
        &self,
        caller: Caller<'_>,
        session: &SessionId,
        after: u64,
        timeout_ms: u32,
        attachment: u64,
    ) -> Response {
        let inner = &self.inner;
        let hold = Duration::from_millis(timeout_ms.into()).min(inner.rules.wait_at_most);
        let deadline = Instant::now() + hold;
        let mut table = inner.lock();
        loop {
            let Some(entry) = table.owned(session, caller) else {
                return refuse(ErrorCode::NotFound, "no such session");
            };
            // Only the client that is shown the session speaks for it: a
            // question from anywhere else does not keep the picture going.
            let shown = entry.shows(caller, attachment);
            if shown {
                entry.seen = Instant::now();
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if entry.info.revision > after || left.is_zero() {
                return report_to(&entry.info, shown);
            }
            if shown {
                entry.waiting += 1;
            }
            table = inner
                .changed
                .wait_timeout(table, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
            if shown {
                if let Some(entry) = table.sessions.get_mut(&session.0) {
                    entry.waiting = entry.waiting.saturating_sub(1);
                }
            }
        }
    }

    /// Shows a session that runs to the caller. Answered when the runner
    /// sends the picture there, or when it is clear that it will not: the
    /// session goes on either way.
    fn attach(
        &self,
        caller: Caller<'_>,
        remote: SocketAddr,
        session: &SessionId,
        display: DisplayMode,
        offer: &serde_json::Value,
    ) -> Response {
        let inner = &self.inner;
        let peer = caller.peer;
        if let Some(why) = unreasonable(display) {
            return refuse(ErrorCode::Unavailable, why);
        }
        if !fits(offer) {
            return refuse(
                ErrorCode::Unavailable,
                "the offer of the media path is larger than 64 KiB",
            );
        }
        let number = OsRng.next_u64();
        // A little longer than the runner is given, so that the supervisor
        // is the one that gives up.
        let deadline = Instant::now() + inner.rules.attach_within + inner.rules.tick * 20;
        let mut table = inner.lock();
        {
            let Some(entry) = table.owned(session, caller) else {
                return refuse(ErrorCode::NotFound, "no such session");
            };
            match entry.info.state {
                SessionState::Running | SessionState::Detached => {}
                SessionState::Starting => {
                    return refuse(ErrorCode::Unavailable, "the session is still starting")
                }
                _ => return refuse(ErrorCode::Unavailable, "the session is ending"),
            }
            entry.orders.attach = Some(Attach {
                number,
                device: Device::of(peer),
                peer: SessionPeer {
                    id: peer.id.clone(),
                    address: address_text(&remote),
                    public_key: Some(peer.public_key.clone()),
                },
                display,
                offer: offer.clone(),
            });
        }
        inner.changed.notify_all();
        loop {
            let Some(entry) = table.owned(session, caller) else {
                return refuse(ErrorCode::NotFound, "no such session");
            };
            if let Some(at) = entry.attached.iter().position(|(n, _)| *n == number) {
                return match entry.attached.swap_remove(at).1 {
                    Ok(()) => report(&entry.info),
                    Err(why) => refuse(ErrorCode::Unavailable, why),
                };
            }
            if entry.ended_at.is_some() || entry.info.state == SessionState::Stopping {
                return refuse(
                    ErrorCode::Unavailable,
                    "the session ended before it could be shown",
                );
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return refuse(
                    ErrorCode::Unavailable,
                    "the host's side of the stream did not answer",
                );
            }
            table = inner
                .changed
                .wait_timeout(table, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    fn detach(&self, caller: Caller<'_>, session: &SessionId, attachment: u64) -> Response {
        let inner = &self.inner;
        let deadline = Instant::now() + inner.rules.tick * 10;
        let mut table = inner.lock();
        loop {
            let Some(entry) = table.owned(session, caller) else {
                return refuse(ErrorCode::NotFound, "no such session");
            };
            // A window that has been replaced has nothing to leave any more.
            let shown = entry.shows(caller, attachment)
                && matches!(
                    entry.info.state,
                    SessionState::Starting | SessionState::Running
                );
            if shown {
                entry.orders.detach = true;
                inner.changed.notify_all();
            }
            let left = deadline.saturating_duration_since(Instant::now());
            // A session that is still starting leaves as soon as it runs;
            // there is nothing to wait for now.
            if !shown || entry.info.state == SessionState::Starting || left.is_zero() {
                return report_to(&entry.info, shown);
            }
            table = inner
                .changed
                .wait_timeout(table, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// The client that is shown the session says whether its window can
    /// be seen. One that cannot needs no picture, and gives its encoder
    /// slot to a window that can; one the person turned to goes to the
    /// front of the line for a slot.
    fn show(
        &self,
        caller: Caller<'_>,
        session: &SessionId,
        attachment: u64,
        visible: bool,
    ) -> Response {
        let inner = &self.inner;
        let mut table = inner.lock();
        let Some(entry) = table.owned(session, caller) else {
            return refuse(ErrorCode::NotFound, "no such session");
        };
        // A window that has been replaced has nothing to say about it.
        let shown = entry.shows(caller, attachment);
        if shown {
            entry.hidden = !visible;
            entry.seen = Instant::now();
            if visible {
                entry.touched = Instant::now();
            }
        }
        table.allot(&inner.rules);
        let report = table
            .owned(session, caller)
            .map(|entry| report_to(&entry.info, shown));
        drop(table);
        inner.changed.notify_all();
        report.unwrap_or_else(|| refuse(ErrorCode::NotFound, "no such session"))
    }

    fn stop(&self, asker: Asker<'_>, session: &SessionId, force: bool) -> Response {
        let inner = &self.inner;
        let deadline = Instant::now() + inner.rules.tick * 10;
        let mut table = inner.lock();
        loop {
            let Some(entry) = table.get(session, asker) else {
                return refuse(ErrorCode::NotFound, "no such session");
            };
            let state = entry.info.state;
            if !force {
                // Asking is all that is done. What the application makes of
                // it shows in the session's state, later or never. A left
                // over application is asked again.
                if !matches!(state, SessionState::Stopping | SessionState::Ended) {
                    entry.orders.close = true;
                    inner.changed.notify_all();
                }
                return report_to(&entry.info, false);
            }
            if state == SessionState::Ended {
                return report_to(&entry.info, false);
            }
            // A person's force is kept whatever the session is doing: while
            // it is stopping (the application is being asked) and while it is
            // left over, it is what ends the application.
            entry.orders.force = true;
            let left = deadline.saturating_duration_since(Instant::now());
            if state == SessionState::Stopping || left.is_zero() {
                inner.changed.notify_all();
                return report_to(&entry.info, false);
            }
            inner.changed.notify_all();
            table = inner
                .changed
                .wait_timeout(table, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Whether `peer` is the device that is shown `session` at this moment.
    /// Keyboard and pointer input for a session is taken from that device
    /// and from no other (the person's decision of 2026-11-17).
    pub fn shown_to(&self, session: &SessionId, peer: &Peer) -> bool {
        self.inner
            .lock()
            .sessions
            .get(&session.0)
            .is_some_and(|entry| {
                entry.ended_at.is_none()
                    && entry.info.state == SessionState::Running
                    && entry.viewer.id == peer.id
                    && entry.viewer.public_key == peer.public_key
            })
    }

    /// Where the runner of `session` takes input, when `peer` is the device
    /// that is shown the session.
    pub fn input_socket_of(&self, session: &SessionId, peer: &Peer) -> Option<std::path::PathBuf> {
        self.shown_to(session, peer)
            .then(|| self.inner.paths.session_input_socket(session))
    }

    /// Somebody changed the list of paired devices: every session reads it
    /// now instead of when its turn comes.
    pub fn pairings_changed(&self) {
        self.inner.pairings_epoch.fetch_add(1, Ordering::SeqCst);
        self.inner.changed.notify_all();
    }

    /// Every session of this host that is not over, whichever device
    /// started it, in the order they were started. For the person at this
    /// machine.
    pub fn host_list(&self) -> Vec<HostSession> {
        self.inner.forget_old();
        let table = self.inner.lock();
        let mut all: Vec<&Entry> = table
            .sessions
            .values()
            .filter(|e| e.ended_at.is_none())
            .collect();
        all.sort_by_key(|e| e.number);
        all.into_iter().map(Entry::for_host).collect()
    }

    /// One session, whichever device started it; also for a while after it
    /// is over.
    pub fn host_look(&self, session: &SessionId) -> Option<HostSession> {
        self.inner
            .lock()
            .get(session, Asker::Host)
            .map(|entry| entry.for_host())
    }

    /// Asks a session's application to close, or with `force` ends it: what
    /// the device that started it can do, done by the person at this
    /// machine.
    pub fn host_stop(
        &self,
        session: &SessionId,
        force: bool,
    ) -> Result<HostSession, (ErrorCode, String)> {
        self.inner.forget_old();
        match self.stop(Asker::Host, session, force) {
            Response::Error { code, message } => Err((code, message)),
            _ => self
                .host_look(session)
                .ok_or_else(|| (ErrorCode::NotFound, "no such session".to_string())),
        }
    }

    /// The report of a session, whoever asks. For tests.
    #[cfg(test)]
    pub(crate) fn look(&self, session: &SessionId) -> Option<SessionInfo> {
        self.inner
            .lock()
            .sessions
            .get(&session.0)
            .map(|e| e.info.clone())
    }
}

impl Entry {
    fn is_for(&self, caller: Caller<'_>) -> bool {
        policy::may_use(caller.rights)
    }

    /// Whether `caller` is the client that is shown the session as
    /// attachment `attachment`.
    ///
    /// An attachment belongs to the device it was given to. Its number
    /// counts up and is no secret, and every device with the rights may
    /// use every session, so the number alone proves nothing: a second
    /// device that names it must not keep the first one's picture going,
    /// leave for it, hide its window or be told what the media path
    /// agreed on with it.
    fn shows(&self, caller: Caller<'_>, attachment: u64) -> bool {
        attachment != 0
            && attachment == self.info.attachment
            && self.viewer.id == caller.peer.id
            && self.viewer.public_key == caller.peer.public_key
    }

    /// What the person at the host is told about it.
    fn for_host(&self) -> HostSession {
        let mut info = self.info.clone();
        // What the media path agreed on is for the client that is shown
        // the session. It has no place in a list.
        info.answer = None;
        HostSession {
            info,
            app_name: self.app_name.clone(),
            started_by: StartedBy {
                id: self.owner.id.clone(),
                name: self.owner.name.clone(),
            },
            revoked: self.revoked,
        }
    }
}

impl Table {
    /// The session `session`, when `asker` may use it. A session that is
    /// not for a device is not there for it, like one that never was.
    fn get(&mut self, session: &SessionId, asker: Asker<'_>) -> Option<&mut Entry> {
        if !session.is_well_formed() {
            return None;
        }
        self.sessions
            .get_mut(&session.0)
            .filter(|entry| match asker {
                Asker::Device(caller) => entry.is_for(caller),
                Asker::Host => true,
            })
    }

    fn owned(&mut self, session: &SessionId, caller: Caller<'_>) -> Option<&mut Entry> {
        self.get(session, Asker::Device(caller))
    }

    /// Decides which of the sessions that are shown to someone have their
    /// picture made, when the host has `slots` encoders to give.
    ///
    /// A session nobody is shown holds none. Neither does one whose
    /// window cannot be seen. Of the rest, the ones that were turned to
    /// most recently get a slot each, and the others wait with their
    /// picture stopped: they are open, attached and running, and say why
    /// they show nothing new. No session is refused or ended for a slot.
    ///
    /// The number of slots is what the host is set to, or fewer for a
    /// while after an encoder was refused: the setting is only a guess at
    /// what the card gives, and other programs on the machine take from
    /// the same number.
    fn allot(&mut self, rules: &Rules) {
        fn set(entry: &mut Entry, paused: Option<PauseReason>) {
            if entry.info.paused != paused {
                entry.info.paused = paused;
                entry.info.revision += 1;
            }
        }
        let mut wanting: Vec<&mut Entry> = Vec::new();
        for entry in self.sessions.values_mut() {
            let shown = entry.ended_at.is_none()
                && matches!(
                    entry.info.state,
                    SessionState::Starting | SessionState::Running
                );
            if !shown {
                set(entry, None);
            } else if entry.hidden {
                set(entry, Some(PauseReason::Hidden));
            } else {
                wanting.push(entry);
            }
        }
        if self
            .fewer
            .is_some_and(|(_, since)| since.elapsed() >= rules.encoder_retry_after)
        {
            self.fewer = None;
        }
        let slots = self
            .fewer
            .map_or(rules.encoder_slots, |(now, _)| now.min(rules.encoder_slots));
        wanting.sort_by(|a, b| b.touched.cmp(&a.touched).then(b.number.cmp(&a.number)));
        for (place, entry) in wanting.into_iter().enumerate() {
            set(entry, (place >= slots).then_some(PauseReason::NoSlot));
        }
    }

    /// Why one more session cannot be started, when it cannot: the host
    /// keeps as many open as it is set to. Nothing is ended to make room.
    /// The device is told which sessions nobody is shown, so that a person
    /// can decide.
    fn full_for(&self, caller: Caller<'_>, rules: &Rules) -> Option<String> {
        let live: Vec<&Entry> = self
            .sessions
            .values()
            .filter(|e| e.ended_at.is_none())
            .collect();
        if live.len() < rules.max_sessions {
            return None;
        }
        let mut idle: Vec<&&Entry> = live
            .iter()
            .filter(|e| e.info.state == SessionState::Detached && e.is_for(caller))
            .collect();
        idle.sort_by_key(|e| e.number);
        let idle: Vec<String> = idle
            .iter()
            .map(|e| match &e.info.left_by {
                Some(device) => format!("{} ({}, opened by {device})", e.app_name, e.info.id),
                None => format!("{} ({})", e.app_name, e.info.id),
            })
            .collect();
        let mut left: Vec<&&Entry> = live
            .iter()
            .filter(|e| e.info.state == SessionState::LeftOver && e.is_for(caller))
            .collect();
        left.sort_by_key(|e| e.number);
        let left: Vec<String> = left
            .iter()
            .map(|e| format!("{} ({})", e.app_name, e.info.id))
            .collect();
        let mut why = format!(
            "this host already keeps {} sessions open, as many as it is set to \
             (`istesso agent --max-sessions`)",
            live.len()
        );
        if !idle.is_empty() {
            why.push_str(&format!("; nobody is shown: {}", idle.join(", ")));
        }
        if !left.is_empty() {
            why.push_str(&format!(
                "; without a window, their processes left over (`istesso sessions stop <session> \
                 --force` ends them): {}",
                left.join(", ")
            ));
        }
        Some(why)
    }
}

// ---------------------------------------------------------------------------
// Looking after one session
// ---------------------------------------------------------------------------

/// What the supervising thread of a session is given.
struct Job {
    id: SessionId,
    owner: Device,
    app_name: String,
    runner: Arc<dyn Runner>,
    plan: LaunchPlan,
    start: RunnerCommand,
    input_socket: std::path::PathBuf,
}

/// What the supervisor found to do in one round.
#[derive(Default)]
struct Round {
    force: bool,
    close: bool,
    attach: Option<Attach>,
    /// The client that is shown the session is to be left, and why.
    leave: Option<String>,
    /// The session is shown to a client, and its picture is to be stopped
    /// for now.
    pause: bool,
    /// The session is shown to a client.
    running: bool,
    /// The device that started the session may not use it now.
    revoked: bool,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, Table> {
        self.table.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn forget_old(&self) {
        let keep = self.rules.remember_ended_for;
        self.lock()
            .sessions
            .retain(|_, e| e.ended_at.is_none_or(|at| at.elapsed() < keep));
    }

    /// Changes a session and wakes everyone who waits for it. The report's
    /// revision goes up.
    fn update(&self, id: &SessionId, change: impl FnOnce(&mut Entry)) {
        {
            let mut table = self.lock();
            if let Some(entry) = table.sessions.get_mut(&id.0) {
                change(entry);
                entry.info.revision += 1;
                if entry.info.state == SessionState::Ended {
                    entry.ended_at = Some(Instant::now());
                }
            }
            // A session that came, went or changed hands changes which
            // windows have an encoder.
            table.allot(&self.rules);
        }
        self.changed.notify_all();
    }

    /// Nobody is shown the session any more. The application runs on.
    fn detached(&self, id: &SessionId, why: &str) {
        self.update(id, |entry| {
            entry.info.state = SessionState::Detached;
            entry.info.answer = None;
            entry.info.shown_on = None;
        });
        (self.log)(format!("session {id}: nobody is shown it now ({why})"));
    }

    /// A runner's encoder was refused, though its session had a slot. The
    /// setting was too high for this moment: the card gives fewer than it
    /// says, or another program on the machine took one. Nothing crashes
    /// and nothing ends. The host counts one picture fewer for a while,
    /// which stops the picture of the window that was turned to longest
    /// ago; that window's report says why, so that its client can tell
    /// the person.
    ///
    /// A session that holds no slot has nothing to be refused: nobody is
    /// shown it, its window cannot be seen, or it was just told to stop.
    /// What its runner says then changes nothing.
    fn encoder_refused(&self, job: &Job, why: &str) {
        fn makes_a_picture(entry: &Entry) -> bool {
            entry.ended_at.is_none()
                && matches!(
                    entry.info.state,
                    SessionState::Starting | SessionState::Running
                )
                && !entry.hidden
                && entry.info.paused.is_none()
        }
        let makes = {
            let mut table = self.lock();
            if !table.sessions.get(&job.id.0).is_some_and(makes_a_picture) {
                drop(table);
                (self.log)(format!(
                    "session {}: its runner says that the encoder was refused, while the \
                     session holds none; nothing changes",
                    job.id
                ));
                return;
            }
            let making = table
                .sessions
                .values()
                .filter(|entry| makes_a_picture(entry))
                .count();
            // The one that was refused counted as making a picture.
            let makes = making.saturating_sub(1);
            table.fewer = Some((makes, Instant::now()));
            table.allot(&self.rules);
            makes
        };
        self.changed.notify_all();
        (self.log)(format!(
            "session {}: the encoder was refused{}{why}; this host makes {makes} pictures at once \
             for the next {} seconds, and a window that is not being looked at has its picture \
             stopped",
            job.id,
            if why.is_empty() { "" } else { ": " },
            self.rules.encoder_retry_after.as_secs()
        ));
    }

    /// Tells the device that gave attach order `number` how it went.
    fn attach_went(&self, id: &SessionId, number: u64, outcome: Result<(), String>) {
        if let Some(entry) = self.lock().sessions.get_mut(&id.0) {
            entry.attached.push((number, outcome));
        }
        self.changed.notify_all();
    }

    /// What `device` may do right now: read from the list of paired
    /// devices.
    fn standing_of(&self, device: &Device) -> Standing {
        standing(read_peers(&self.paths).as_ref(), device)
    }

    /// Notes what the device that started a session may do with it now.
    /// A session whose device was unpaired is kept, and says from then on
    /// who opened it: that does not go away when the device is paired
    /// again.
    fn mark(&self, job: &Job, owner: Standing) {
        let revoked = owner != Standing::Allowed;
        let (id, name, app) = (&job.id, &job.owner.name, &job.app_name);
        let mut said = Vec::new();
        {
            let mut table = self.lock();
            let Some(entry) = table.sessions.get_mut(&id.0) else {
                return;
            };
            if entry.revoked != revoked {
                entry.revoked = revoked;
                said.push(if revoked {
                    format!("{name} may not use it now; {app} keeps running")
                } else {
                    format!("{name} may use it again")
                });
            }
            if owner == Standing::Unpaired && entry.info.left_by.is_none() {
                entry.info.left_by = Some(name.clone());
                entry.info.revision += 1;
                said.push(format!(
                    "{name} was unpaired; {app} is kept for every device that may start \
                     applications here, and for `istesso sessions --local`"
                ));
            }
        }
        if !said.is_empty() {
            self.changed.notify_all();
        }
        for line in said {
            (self.log)(format!("session {id}: {line}"));
        }
    }

    /// Takes what was asked for since the last round, looks whether the
    /// client that is shown the session is still there, and reads again
    /// what the devices that have to do with the session may do.
    /// `attaching` is the client that is being attached, if one is: the
    /// one before it no longer counts.
    fn round(&self, job: &Job, checked: &mut Checked, attaching: Option<&Device>) -> Round {
        let id = &job.id;
        let mut round = Round::default();
        // Whose rights keep the picture going: the client that is being
        // attached, or else the one that is shown the session.
        let subject = {
            let mut table = self.lock();
            // The setting is tried again a while after an encoder was
            // refused.
            let again = self.rules.encoder_retry_after;
            if table
                .fewer
                .is_some_and(|(_, since)| since.elapsed() >= again)
            {
                table.allot(&self.rules);
            }
            let Some(entry) = table.sessions.get_mut(&id.0) else {
                return round;
            };
            let orders = std::mem::take(&mut entry.orders);
            round.force = orders.force;
            round.close = orders.close;
            round.attach = orders.attach;
            round.revoked = entry.revoked;
            round.running = entry.info.state == SessionState::Running && attaching.is_none();
            round.pause = round.running && entry.info.paused.is_some();
            let showing = matches!(
                entry.info.state,
                SessionState::Starting | SessionState::Running
            );
            if showing && attaching.is_none() && round.attach.is_none() {
                let lost_after = self.rules.client_lost_after;
                if orders.detach {
                    round.leave = Some("its client left".to_string());
                } else if entry.waiting == 0 && entry.seen.elapsed() > lost_after {
                    round.leave = Some(format!(
                        "its client has not asked about it for {} seconds",
                        lost_after.as_secs_f32()
                    ));
                }
            }
            attaching
                .cloned()
                .or_else(|| showing.then(|| entry.viewer.clone()))
        };
        // The list of paired devices is read for every session, shown or
        // not: a session nobody is shown still holds a place and can be
        // attached to. It is read every so often, at once when somebody
        // says that it changed, and always before a client is attached.
        let epoch = self.pairings_epoch.load(Ordering::SeqCst);
        let due = round.attach.is_some()
            || epoch != checked.epoch
            || checked
                .at
                .is_none_or(|at| at.elapsed() >= self.rules.recheck_rights_every);
        if !due {
            return round;
        }
        *checked = Checked {
            at: Some(Instant::now()),
            epoch,
        };
        let Some(peers) = read_peers(&self.paths) else {
            // A list that cannot be read is not an unpairing: the session
            // keeps its application and its marks, and is open to nobody
            // it was not open to. But nothing is allowed while nothing is
            // known. Nobody new is shown the session, and the picture that
            // is being sent stops. The list is read again when it is next
            // due.
            if let Some(attach) = round.attach.take() {
                self.attach_went(id, attach.number, Err(CANNOT_READ.to_string()));
            }
            if subject.is_some() {
                round.leave = Some(CANNOT_READ.to_string());
            }
            return round;
        };
        let peers = Some(&peers);
        let owner = standing(peers, &job.owner);
        self.mark(job, owner);
        round.revoked = owner != Standing::Allowed;
        // A pairing that is removed, or a right that is taken away, stops
        // the picture and keeps it stopped. It does not end the
        // application.
        if let Some(attach) = &round.attach {
            if standing(peers, &attach.device) != Standing::Allowed {
                self.attach_went(id, attach.number, Err(NOT_ALLOWED.to_string()));
                round.attach = None;
            }
        }
        if subject.is_some_and(|device| standing(peers, &device) != Standing::Allowed) {
            round.leave = Some(NOT_ALLOWED.to_string());
        }
        round
    }

    /// Rests for one tick, or until something changes.
    fn pause(&self) {
        let table = self.lock();
        let _ = self.changed.wait_timeout(table, self.rules.tick);
    }

    /// Runs one session from its start to its end. Everything that can go
    /// wrong ends up as the session's `end`.
    fn supervise(&self, job: Job) {
        let rules = &self.rules;
        let id = &job.id;
        let mut stream: Option<Box<dyn RunningStream>> = None;
        let mut app: Option<Box<dyn RunningApp>> = None;
        let mut checked = Checked::default();
        // Whether a person asked to end the application by force: the one
        // way that ends it so (ADR 0010, 3).
        let mut forced = false;

        let mut end = 'session: {
            // The host's side of the stream comes first: it makes the
            // display the application appears on.
            if let Some(socket) = job.input_socket.parent() {
                let _ = private_dir(socket);
            }
            let running = match job.runner.start(&job.start) {
                Ok(running) => stream.insert(running),
                Err(why) => break 'session ending(EndReason::StreamFailed, why),
            };
            let asked = Instant::now();
            // The client may leave before there is anything to show, and
            // the application may be asked to close before it runs.
            let mut left_early: Option<String> = None;
            let mut close_early = false;
            let (env, display, answer) = loop {
                match running.poll() {
                    Some(StreamEvent::Said(RunnerEvent::Ready {
                        env,
                        display,
                        answer,
                    })) => break (env, display, answer),
                    Some(StreamEvent::Said(RunnerEvent::Error { message })) => {
                        break 'session ending(EndReason::StreamFailed, message)
                    }
                    Some(StreamEvent::Exited(how)) => {
                        let why = format!(
                            "the host's side of the stream ended before it was ready ({how})"
                        );
                        break 'session ending(EndReason::StreamFailed, why);
                    }
                    Some(StreamEvent::Said(_)) => continue,
                    None => {}
                }
                if asked.elapsed() > rules.runner_ready_within {
                    let why = format!(
                        "the host's side of the stream was not ready within {} seconds",
                        rules.runner_ready_within.as_secs_f32()
                    );
                    break 'session ending(EndReason::StreamFailed, why);
                }
                let round = self.round(&job, &mut checked, None);
                if round.force {
                    forced = true;
                    break 'session ending(EndReason::Stopped, "");
                }
                if round.revoked {
                    // No application runs yet, so no work is lost.
                    break 'session ending(EndReason::Stopped, NOT_STARTED);
                }
                left_early = left_early.or(round.leave);
                close_early |= round.close;
                self.pause();
            };
            if !fits(&answer) {
                break 'session ending(
                    EndReason::StreamFailed,
                    "the answer of the host's media path is larger than 64 KiB",
                );
            }

            // The application, on the display the runner made. Nothing is
            // started for a device that lost its rights while the runner
            // got ready.
            match self.standing_of(&job.owner) {
                Standing::Allowed => {}
                Standing::Unknown => {
                    let why = format!("the application was not started: {CANNOT_READ}");
                    break 'session ending(EndReason::AppFailed, why);
                }
                Standing::Barred | Standing::Unpaired => {
                    break 'session ending(EndReason::Stopped, NOT_STARTED)
                }
            }
            let application = match self.launcher.launch(&job.plan, &env) {
                Ok(application) => app.insert(application),
                Err(why) => {
                    let why = format!("the application could not be started ({why})");
                    break 'session ending(EndReason::AppFailed, why);
                }
            };
            self.update(id, |entry| {
                entry.info.state = SessionState::Running;
                entry.info.display = Some(display);
                entry.info.answer = Some(answer);
                entry.info.shown_on = Some(entry.viewer.name.clone());
                entry.seen = Instant::now();
            });
            (self.log)(format!("session {id}: {} is running", job.app_name));
            if let Some(pid) = application.pid() {
                let _ = running.tell(&RunnerCommand::Application { pid });
            }
            if let Some(why) = left_early {
                let _ = running.tell(&RunnerCommand::Detach);
                self.detached(id, &why);
            }
            if close_early {
                (self.log)(format!("session {id}: asking {} to close", job.app_name));
                let _ = running.tell(&RunnerCommand::Close);
            }

            // The attach order that the runner has been told and has not
            // answered yet, and since when.
            let mut attaching: Option<(u64, Instant, Device)> = None;
            // Whether the runner was told to stop making the picture for
            // the client that is attached, or has said that it cannot.
            let mut paused = false;
            // After an encoder was refused: when the runner may be told to
            // try again.
            let mut try_encoder_at: Option<Instant> = None;
            loop {
                if let Some(how) = application.poll() {
                    let why = format!("the application ended ({how})");
                    break 'session ending(EndReason::AppExited, why);
                }
                match running.poll() {
                    Some(StreamEvent::Said(RunnerEvent::Empty)) => {
                        break 'session ending(
                            EndReason::AppExited,
                            "the application closed its last window",
                        )
                    }
                    Some(StreamEvent::Said(RunnerEvent::Opening)) => {
                        // A client can show that the application is opening.
                        self.update(id, |entry| entry.info.opening = true);
                        continue;
                    }
                    Some(StreamEvent::Said(RunnerEvent::Windows { count })) => {
                        if count > 0 {
                            self.update(id, |entry| entry.info.opening = false);
                        }
                        continue;
                    }
                    Some(StreamEvent::Said(RunnerEvent::NoWindowYet { after_s })) => {
                        // The runner concludes nothing. The application's
                        // processes live (it would have ended above
                        // otherwise): it is ended as after `empty`, asked
                        // and never forced.
                        let why =
                            format!("the application showed no window within {after_s} seconds");
                        break 'session ending(EndReason::AppExited, why);
                    }
                    Some(StreamEvent::Said(RunnerEvent::Error { message })) => {
                        break 'session ending(EndReason::StreamFailed, message)
                    }
                    Some(StreamEvent::Exited(how)) => {
                        let why = format!("the host's side of the stream ended ({how})");
                        break 'session ending(EndReason::StreamFailed, why);
                    }
                    Some(StreamEvent::Said(RunnerEvent::Attached { display, answer })) => {
                        let Some((number, _, device)) = attaching.take() else {
                            continue;
                        };
                        if !fits(&answer) {
                            let why = "the answer of the host's media path is larger than 64 KiB";
                            let _ = running.tell(&RunnerCommand::Detach);
                            self.detached(id, why);
                            self.attach_went(id, number, Err(why.to_string()));
                            continue;
                        }
                        self.update(id, |entry| {
                            entry.info.state = SessionState::Running;
                            entry.info.attachment += 1;
                            entry.info.display = Some(display);
                            entry.info.answer = Some(answer);
                            entry.info.shown_on = Some(device.name.clone());
                            entry.viewer = device;
                            entry.hidden = false;
                            entry.touched = Instant::now();
                            entry.seen = Instant::now();
                            entry.attached.push((number, Ok(())));
                        });
                        (self.log)(format!("session {id}: shown to a client again"));
                        // A runner that attaches a client makes its picture.
                        paused = false;
                        continue;
                    }
                    Some(StreamEvent::Said(RunnerEvent::Detached { message })) => {
                        let why = if message.is_empty() {
                            "the host's side of the stream sends to nobody".to_string()
                        } else {
                            message
                        };
                        self.detached(id, &why);
                        if let Some((number, ..)) = attaching.take() {
                            self.attach_went(id, number, Err(why));
                        }
                        continue;
                    }
                    Some(StreamEvent::Said(RunnerEvent::NoEncoder { message })) => {
                        // Nothing ends for that. The host makes one picture
                        // fewer for a while, and the window that was turned
                        // to longest ago gives its picture up, with the
                        // reason in its report. This runner has no encoder
                        // now; if its window keeps a slot, it is told to try
                        // again once the other has let go of one.
                        self.encoder_refused(&job, &message);
                        paused = true;
                        try_encoder_at = Some(Instant::now() + rules.encoder_settle);
                        continue;
                    }
                    Some(StreamEvent::Said(RunnerEvent::Ready { .. })) => continue,
                    None => {}
                }
                let late = |(_, since, _): &mut (u64, Instant, Device)| {
                    since.elapsed() > rules.attach_within
                };
                if let Some((number, ..)) = attaching.take_if(late) {
                    let why = format!(
                        "the host's side of the stream did not take the client within {} seconds",
                        rules.attach_within.as_secs_f32()
                    );
                    let _ = running.tell(&RunnerCommand::Detach);
                    self.detached(id, &why);
                    self.attach_went(id, number, Err(why));
                }

                let being_attached = attaching.as_ref().map(|(_, _, device)| device);
                let round = self.round(&job, &mut checked, being_attached);
                if round.force {
                    forced = true;
                    break 'session ending(EndReason::Stopped, "");
                }
                if round.close {
                    // Only the compositor can ask a window to close.
                    (self.log)(format!("session {id}: asking {} to close", job.app_name));
                    let _ = running.tell(&RunnerCommand::Close);
                }
                if let Some(attach) = round.attach {
                    // A client that was still being attached has lost its
                    // turn to this one.
                    if let Some((before, ..)) = attaching.take() {
                        let why = "another window took the session".to_string();
                        self.attach_went(id, before, Err(why));
                    }
                    let told = running.tell(&RunnerCommand::Attach {
                        peer: attach.peer,
                        display: attach.display,
                        offer: attach.offer,
                    });
                    match told {
                        Ok(()) => {
                            attaching = Some((attach.number, Instant::now(), attach.device));
                        }
                        Err(why) => self.attach_went(id, attach.number, Err(why)),
                    }
                } else if let Some(why) = round.leave {
                    let _ = running.tell(&RunnerCommand::Detach);
                    self.detached(id, &why);
                    // A client that was being attached is not shown it
                    // either.
                    if let Some((number, ..)) = attaching.take() {
                        self.attach_went(id, number, Err(why));
                    }
                } else if !round.running {
                    // Nobody is attached: there is no picture to stop.
                    paused = false;
                } else if round.pause != paused
                    && (round.pause || try_encoder_at.is_none_or(|at| Instant::now() >= at))
                {
                    // The session stays as it is, attached and running. It
                    // only holds no encoder for now, or holds one again.
                    paused = round.pause;
                    try_encoder_at = None;
                    let (line, what) = if paused {
                        (RunnerCommand::Pause, "its picture is stopped for now")
                    } else {
                        (RunnerCommand::Resume, "its picture is made again")
                    };
                    let _ = running.tell(&line);
                    (self.log)(format!("session {id}: {what}"));
                }
                self.pause();
            }
        };

        let mut was_shown = false;
        self.update(id, |entry| {
            was_shown = entry.info.state == SessionState::Running;
            entry.info.state = SessionState::Stopping;
            entry.info.answer = None;
            entry.info.shown_on = None;
            entry.info.opening = false;
            entry.info.end = Some(end.clone());
        });
        // The application first: it can end while its display is still
        // there. Only a person's force ends it by force. On every other way
        // it is asked to end, and what does not listen is left as it is,
        // for a person to see and decide on (ADR 0010, 3).
        let mut stays = None;
        if let Some(app) = app
            .as_mut()
            .and_then(|app| app.poll().is_none().then_some(app))
        {
            // The session is over for its client: nobody is sent its picture
            // while the application is given time to end (ADR 0010, 12: an
            // unpaired device is cut within 0.1 s).
            if let Some(stream) = stream.as_mut().filter(|_| was_shown) {
                let _ = stream.tell(&RunnerCommand::Detach);
            }
            if forced {
                app.end_by_force(rules.app_stop_grace);
            } else {
                app.ask_to_end();
                match self.ends_within(id, app.as_mut(), rules.app_end_wait) {
                    Waited::Ended(how) => {
                        add_to(&mut end, &format!("it ended when asked to ({how})"))
                    }
                    Waited::Forced => {
                        app.end_by_force(rules.app_stop_grace);
                        add_to(&mut end, "a person ended it by force");
                        end.reason = EndReason::Stopped;
                    }
                    Waited::Stays => stays = Some(app.group()),
                }
            }
        }
        if let Some(mut stream) = stream {
            stream.stop(rules.runner_stop_grace);
        }
        // The runner made the socket; it is of no use to anyone now.
        let _ = std::fs::remove_file(&job.input_socket);
        if let (Some(group), Some(app)) = (stays, app.as_mut()) {
            self.left_over(id, &mut end, group, app.as_mut());
        }
        self.update(id, |entry| {
            entry.info.state = SessionState::Ended;
            entry.info.left_over = None;
            entry.info.end = Some(end.clone());
        });
        (self.log)(format!(
            "session {id}: {} ended: {:?}{}{}",
            job.app_name,
            end.reason,
            if end.message.is_empty() { "" } else { ", " },
            end.message
        ));
    }
}

impl Inner {
    /// Waits for an application that was asked to end, for up to `time`,
    /// or until a person asks to end it by force.
    fn ends_within(&self, id: &SessionId, app: &mut dyn RunningApp, time: Duration) -> Waited {
        let deadline = Instant::now() + time;
        loop {
            if let Some(how) = app.poll() {
                return Waited::Ended(how);
            }
            if self.take_force(id) {
                return Waited::Forced;
            }
            if Instant::now() >= deadline {
                return Waited::Stays;
            }
            std::thread::sleep(self.rules.tick.min(Duration::from_millis(50)));
        }
    }

    /// Whether a person asked to end the session's application by force
    /// since this was last asked.
    fn take_force(&self, id: &SessionId) -> bool {
        let mut table = self.lock();
        table
            .sessions
            .get_mut(&id.0)
            .is_some_and(|entry| std::mem::take(&mut entry.orders.force))
    }

    /// The session is over, and the application's processes stay after they
    /// were asked to end. They are shown as they are (`left_over`) until they
    /// are gone, or a person ends them by force. Asked again when a person
    /// asks without force. Nothing else is ever sent to them.
    fn left_over(
        &self,
        id: &SessionId,
        end: &mut SessionEnd,
        group: Option<u32>,
        app: &mut dyn RunningApp,
    ) {
        let Some(group) = group else {
            // Nothing to show a person by: it is left without a word more.
            add_to(end, "it did not end when asked to");
            return;
        };
        add_to(
            end,
            &format!(
                "its processes are still there after they were asked to end (process group {group}); \
                 to end them by force: istesso sessions stop {id} --force (on the host: add --local)"
            ),
        );
        self.update(id, |entry| {
            entry.info.state = SessionState::LeftOver;
            entry.info.left_over = Some(group);
            entry.info.end = Some(end.clone());
            entry.orders = Orders::default();
        });
        (self.log)(format!(
            "session {id}: the application's processes stay after they were asked to end (process group {group})"
        ));
        loop {
            if let Some(how) = app.poll() {
                add_to(end, &format!("they ended later ({how})"));
                return;
            }
            let orders = {
                let mut table = self.lock();
                match table.sessions.get_mut(&id.0) {
                    Some(entry) => std::mem::take(&mut entry.orders),
                    None => Orders::default(),
                }
            };
            if orders.force {
                // A person's word: the one way to end it by force. Stopping
                // until it is over, so that whoever asked waits for the end.
                self.update(id, |entry| entry.info.state = SessionState::Stopping);
                app.end_by_force(self.rules.app_stop_grace);
                add_to(end, "a person ended them by force");
                end.reason = EndReason::Stopped;
                return;
            }
            if orders.close {
                app.ask_to_end();
            }
            self.pause();
        }
    }
}

/// What became of an application that was asked to end.
enum Waited {
    Ended(String),
    /// A person asked to end it by force meanwhile.
    Forced,
    /// Still there.
    Stays,
}

/// Adds a sentence to the message of a session's end.
fn add_to(end: &mut SessionEnd, more: &str) {
    if end.message.is_empty() {
        end.message = more.to_string();
    } else {
        end.message = format!("{}; {more}", end.message);
    }
}

/// Makes a directory only its owner can enter, for sockets.
fn private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}
