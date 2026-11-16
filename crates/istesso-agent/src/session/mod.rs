//! Streaming sessions: an application of this host, shown as a window on
//! another device (`docs/adr/0010-session-start.md`).
//!
//! This is the host's side of the control plane. A paired device asks for an
//! application by its id; the agent starts the program that makes the
//! picture (the *runner*, which belongs to the media path), then the
//! application inside the runner's display, follows both, and ends both
//! together. How the picture and the input travel is not known here.
//!
//! * [`Sessions`] is the table: it answers the three session requests and
//!   keeps one supervising thread per session.
//! * [`Runner`] and [`Launcher`] are the two things a session is made of.
//!   The real ones start processes ([`CommandRunner`], [`DesktopLauncher`]);
//!   tests put their own in.
//! * [`client`] is the other end: what a device does to open a window.

pub mod client;
mod process;
#[cfg(test)]
pub(crate) mod testing;

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use istesso_catalog::launch::LaunchPlan;
use istesso_link::{ErrorCode, Request, Response};
use istesso_pair::{Peer, PeerStore, Permission};
use istesso_proto::session::address_text;
use istesso_proto::{
    AppEntry, DisplayMode, EndReason, RunnerCommand, RunnerEvent, SessionEnd, SessionId,
    SessionInfo, SessionPeer, SessionState, StreamKind,
};
use rand_core::{OsRng, RngCore};

use crate::paths::Paths;
use crate::serve::Log;

pub use process::{app_environment, CommandRunner, DesktopLauncher};

/// The rules of this first slice that are still open with the person who
/// uses it (ADR 0010, "決めてほしいこと"). Each is written down here and
/// nowhere else.
pub mod policy {
    use std::time::Duration;

    use istesso_pair::Permission;

    /// What a device needs to start an application on this host: to look
    /// and to control. A newly paired device has both, so it may.
    pub const START_NEEDS: [Permission; 2] = [Permission::View, Permission::Control];

    /// A client that has not asked about its session for this long is
    /// gone, and the session is stopped, application and all. A client
    /// gives a host that does not answer the same time before it closes
    /// its window. (R-03 wants the application to stay; see the ADR.)
    pub const CLIENT_LOST_AFTER: Duration = Duration::from_secs(15);
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
    /// How long an application gets to end after it was asked to.
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
    /// Sessions that may run at the same time.
    pub max_sessions: usize,
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
            app_stop_grace: Duration::from_secs(3),
            runner_stop_grace: Duration::from_secs(2),
            remember_ended_for: Duration::from_secs(60),
            wait_at_most: Duration::from_secs(3),
            max_sessions: 8,
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

/// The host's side of one stream, starting or running.
pub trait RunningStream: Send {
    /// The next thing it did, without waiting.
    fn poll(&mut self) -> Option<StreamEvent>;
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
    /// `Some` once it has ended by itself, with how.
    fn poll(&mut self) -> Option<String>;
    /// Asks it to end, and ends it when that takes longer than `grace`.
    fn stop(&mut self, grace: Duration);
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
    table: Mutex<HashMap<String, Entry>>,
    /// Signalled whenever a session changes or is asked to stop.
    changed: Condvar,
}

struct Entry {
    info: SessionInfo,
    owner: Owner,
    /// When the owner last asked about the session.
    seen: Instant,
    /// Questions of the owner that are being held back right now.
    waiting: usize,
    /// The owner asked for the session to stop.
    stop: bool,
    ended_at: Option<Instant>,
}

/// The device that started a session. Nobody else may ask about it.
#[derive(Clone)]
struct Owner {
    id: String,
    public_key: String,
}

impl Owner {
    fn is(&self, peer: &Peer) -> bool {
        self.id == peer.id && self.public_key == peer.public_key
    }
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

fn ending(reason: EndReason, message: impl Into<String>) -> SessionEnd {
    SessionEnd {
        reason,
        message: message.into(),
    }
}

/// What the device may do on this host at this moment: read from the list
/// on disk, not remembered from when a conversation or a session began.
/// `None` when it is no longer paired.
fn rights(paths: &Paths, id: &str, public_key: &str) -> Option<Vec<Permission>> {
    let peers = PeerStore::open(paths.peers()).ok()?;
    peers
        .peers()
        .iter()
        .find(|known| known.id == id && known.public_key == public_key)
        .map(|known| known.permissions.clone())
}

fn may_start(paths: &Paths, id: &str, public_key: &str) -> bool {
    rights(paths, id, public_key)
        .is_some_and(|rights| policy::START_NEEDS.iter().all(|need| rights.contains(need)))
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
                table: Mutex::new(HashMap::new()),
                changed: Condvar::new(),
            }),
        }
    }

    /// Answers `session_start`, `session_wait` and `session_stop`.
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
        match request {
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
                self.start(peer, remote, &asked, apps)
            }
            Request::SessionWait {
                session,
                after,
                timeout_ms,
            } => self.wait(peer, session, *after, *timeout_ms),
            Request::SessionStop { session } => self.stop(peer, session),
            _ => refuse(ErrorCode::Unsupported, "not a question about a session"),
        }
    }

    fn start(
        &self,
        peer: &Peer,
        remote: SocketAddr,
        asked: &Asked<'_>,
        apps: impl FnOnce() -> Vec<AppEntry>,
    ) -> Response {
        let inner = &self.inner;
        // Looked up now, not when the conversation began: a conversation
        // can stay open for a long time.
        if !may_start(&inner.paths, &peer.id, &peer.public_key) {
            return refuse(
                ErrorCode::Forbidden,
                "this device may not start applications on the host",
            );
        }
        if asked.kind != StreamKind::AppWindow {
            return refuse(
                ErrorCode::Unavailable,
                "only application windows can be shown so far",
            );
        }
        let DisplayMode { width, height, fps } = asked.display;
        let sane = (16..=16384).contains(&width)
            && (16..=16384).contains(&height)
            && (1..=1000).contains(&fps);
        if !sane {
            return refuse(
                ErrorCode::Unavailable,
                format!("a picture of {width}x{height} at {fps} frames a second cannot be made"),
            );
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
        let info = SessionInfo {
            id: id.clone(),
            app: app.id.clone(),
            kind: asked.kind,
            state: SessionState::Starting,
            revision: 1,
            display: None,
            answer: None,
            end: None,
        };
        {
            let mut table = inner.lock();
            let live = table.values().filter(|e| e.ended_at.is_none()).count();
            if live >= inner.rules.max_sessions {
                return refuse(
                    ErrorCode::Unavailable,
                    format!("this host already runs {live} sessions, as many as it takes"),
                );
            }
            table.insert(
                id.0.clone(),
                Entry {
                    info: info.clone(),
                    owner: Owner {
                        id: peer.id.clone(),
                        public_key: peer.public_key.clone(),
                    },
                    seen: Instant::now(),
                    waiting: 0,
                    stop: false,
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
            app_name: app.name,
            runner,
            plan,
            start: RunnerCommand::Start {
                session: info.id.clone(),
                kind: asked.kind,
                display: asked.display,
                peer: SessionPeer {
                    id: peer.id.clone(),
                    // Where the conversation comes from, not what the
                    // device says about itself: the picture is sent
                    // nowhere else.
                    address: address_text(&remote),
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

    fn wait(&self, peer: &Peer, session: &SessionId, after: u64, timeout_ms: u32) -> Response {
        let inner = &self.inner;
        let hold = Duration::from_millis(timeout_ms.into()).min(inner.rules.wait_at_most);
        let deadline = Instant::now() + hold;
        let mut table = inner.lock();
        loop {
            let Some(entry) = owned(&mut table, session, peer) else {
                return refuse(ErrorCode::NotFound, "no such session");
            };
            entry.seen = Instant::now();
            let left = deadline.saturating_duration_since(Instant::now());
            if entry.info.revision > after || left.is_zero() {
                return report(&entry.info);
            }
            entry.waiting += 1;
            table = inner
                .changed
                .wait_timeout(table, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
            if let Some(entry) = table.get_mut(&session.0) {
                entry.waiting -= 1;
            }
        }
    }

    fn stop(&self, peer: &Peer, session: &SessionId) -> Response {
        let inner = &self.inner;
        // The answer says `stopping` or `ended`; the supervisor gets there
        // within a tick.
        let deadline = Instant::now() + inner.rules.tick * 10;
        let mut table = inner.lock();
        loop {
            let Some(entry) = owned(&mut table, session, peer) else {
                return refuse(ErrorCode::NotFound, "no such session");
            };
            entry.seen = Instant::now();
            entry.stop = true;
            inner.changed.notify_all();
            let left = deadline.saturating_duration_since(Instant::now());
            let stopping = matches!(
                entry.info.state,
                SessionState::Stopping | SessionState::Ended
            );
            if stopping || left.is_zero() {
                return report(&entry.info);
            }
            table = inner
                .changed
                .wait_timeout(table, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// The report of a session, whoever asks. For tests.
    #[cfg(test)]
    pub(crate) fn look(&self, session: &SessionId) -> Option<SessionInfo> {
        self.inner.lock().get(&session.0).map(|e| e.info.clone())
    }
}

/// The session `session`, when it is one of `peer`'s. Somebody else's
/// session is not there for `peer`, like one that never was.
fn owned<'a>(
    table: &'a mut HashMap<String, Entry>,
    session: &SessionId,
    peer: &Peer,
) -> Option<&'a mut Entry> {
    if !session.is_well_formed() {
        return None;
    }
    table.get_mut(&session.0).filter(|e| e.owner.is(peer))
}

fn fits(media: &serde_json::Value) -> bool {
    serde_json::to_vec(media).is_ok_and(|text| text.len() <= MAX_MEDIA_BYTES)
}

// ---------------------------------------------------------------------------
// Looking after one session
// ---------------------------------------------------------------------------

/// What the supervising thread of a session is given.
struct Job {
    id: SessionId,
    app_name: String,
    runner: Arc<dyn Runner>,
    plan: LaunchPlan,
    start: RunnerCommand,
    input_socket: std::path::PathBuf,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Entry>> {
        self.table.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn forget_old(&self) {
        let keep = self.rules.remember_ended_for;
        self.lock()
            .retain(|_, e| e.waiting > 0 || e.ended_at.is_none_or(|at| at.elapsed() < keep));
    }

    /// Changes the report of a session and wakes everyone who waits for it.
    fn update(&self, id: &SessionId, change: impl FnOnce(&mut SessionInfo)) {
        if let Some(entry) = self.lock().get_mut(&id.0) {
            change(&mut entry.info);
            entry.info.revision += 1;
            if entry.info.state == SessionState::Ended {
                entry.ended_at = Some(Instant::now());
            }
        }
        self.changed.notify_all();
    }

    /// Why the session has to stop now, if it has to: its owner asked for
    /// it, went away, or is no longer allowed.
    fn must_stop(&self, id: &SessionId, rights_read: &mut Instant) -> Option<SessionEnd> {
        let owner = {
            let table = self.lock();
            let entry = table.get(&id.0)?;
            if entry.stop {
                return Some(ending(EndReason::Stopped, ""));
            }
            let lost_after = self.rules.client_lost_after;
            if entry.waiting == 0 && entry.seen.elapsed() > lost_after {
                return Some(ending(
                    EndReason::ClientLost,
                    format!(
                        "the device that showed it has not asked about it for {} seconds",
                        lost_after.as_secs_f32()
                    ),
                ));
            }
            entry.owner.clone()
        };
        if rights_read.elapsed() >= self.rules.recheck_rights_every {
            *rights_read = Instant::now();
            if !may_start(&self.paths, &owner.id, &owner.public_key) {
                return Some(ending(
                    EndReason::Revoked,
                    "the device is no longer allowed to start applications on this host",
                ));
            }
        }
        None
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
        let mut rights_read = Instant::now();

        let end = 'session: {
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
                        break 'session ending(
                            EndReason::StreamFailed,
                            format!(
                                "the host's side of the stream ended before it was ready ({how})"
                            ),
                        )
                    }
                    Some(StreamEvent::Said(RunnerEvent::Empty)) => continue,
                    None => {}
                }
                if asked.elapsed() > rules.runner_ready_within {
                    break 'session ending(
                        EndReason::StreamFailed,
                        format!(
                            "the host's side of the stream was not ready within {} seconds",
                            rules.runner_ready_within.as_secs_f32()
                        ),
                    );
                }
                if let Some(end) = self.must_stop(id, &mut rights_read) {
                    break 'session end;
                }
                self.pause();
            };
            if !fits(&answer) {
                break 'session ending(
                    EndReason::StreamFailed,
                    "the answer of the host's media path is larger than 64 KiB",
                );
            }

            // The application, on the display the runner made.
            let application = match self.launcher.launch(&job.plan, &env) {
                Ok(application) => app.insert(application),
                Err(why) => {
                    break 'session ending(
                        EndReason::AppFailed,
                        format!("the application could not be started ({why})"),
                    )
                }
            };
            self.update(id, |info| {
                info.state = SessionState::Running;
                info.display = Some(display);
                info.answer = Some(answer);
            });
            (self.log)(format!("session {id}: {} is running", job.app_name));

            loop {
                if let Some(how) = application.poll() {
                    break 'session ending(
                        EndReason::AppExited,
                        format!("the application ended ({how})"),
                    );
                }
                match running.poll() {
                    Some(StreamEvent::Said(RunnerEvent::Empty)) => {
                        break 'session ending(
                            EndReason::AppExited,
                            "the application closed its last window",
                        )
                    }
                    Some(StreamEvent::Said(RunnerEvent::Error { message })) => {
                        break 'session ending(EndReason::StreamFailed, message)
                    }
                    Some(StreamEvent::Exited(how)) => {
                        break 'session ending(
                            EndReason::StreamFailed,
                            format!("the host's side of the stream ended ({how})"),
                        )
                    }
                    Some(StreamEvent::Said(RunnerEvent::Ready { .. })) => continue,
                    None => {}
                }
                if let Some(end) = self.must_stop(id, &mut rights_read) {
                    break 'session end;
                }
                self.pause();
            }
        };

        self.update(id, |info| {
            info.state = SessionState::Stopping;
            info.end = Some(end.clone());
        });
        // The application first: it can end while its display is still
        // there.
        if let Some(mut app) = app {
            app.stop(rules.app_stop_grace);
        }
        if let Some(mut stream) = stream {
            stream.stop(rules.runner_stop_grace);
        }
        // The runner made the socket; it is of no use to anyone now.
        let _ = std::fs::remove_file(&job.input_socket);
        self.update(id, |info| info.state = SessionState::Ended);
        (self.log)(format!(
            "session {id}: {} ended: {:?}{}{}",
            job.app_name,
            end.reason,
            if end.message.is_empty() { "" } else { ", " },
            end.message
        ));
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

#[cfg(test)]
mod tests {
    use istesso_pair::{Hello, Identity};

    use super::testing::{app, quick_rules, FakeLauncher, FakeRunner, Journal};
    use super::*;
    use crate::testing::TempDir;

    const LAPTOP: &str = "192.0.2.10:50000";

    /// A host with one paired device, a fake runner and a fake launcher.
    struct Host {
        _dir: TempDir,
        paths: Paths,
        sessions: Sessions,
        journal: Journal,
        runner: Arc<FakeRunner>,
        launcher: Arc<FakeLauncher>,
        laptop: Peer,
    }

    fn pair(paths: &Paths, name: &str, permissions: Vec<Permission>) -> Peer {
        let mut peer = Peer::from_hello(&Hello {
            device_name: name.to_string(),
            public_key: Identity::generate().public_key(),
        });
        peer.permissions = permissions;
        let mut store = PeerStore::open(paths.peers()).unwrap();
        store.add(peer.clone()).unwrap();
        peer
    }

    fn host_with(rules: Rules) -> Host {
        let dir = TempDir::new("session");
        let paths = dir.paths();
        let journal = Journal::default();
        let runner = Arc::new(FakeRunner::new(&journal));
        let launcher = Arc::new(FakeLauncher::new(&journal));
        let sessions = Sessions::with(
            paths.clone(),
            Arc::new(|_| {}),
            Some(runner.clone()),
            launcher.clone(),
            rules,
        );
        let laptop = pair(&paths, "laptop", Permission::defaults());
        Host {
            _dir: dir,
            paths,
            sessions,
            journal,
            runner,
            launcher,
            laptop,
        }
    }

    fn host() -> Host {
        host_with(quick_rules())
    }

    fn mode() -> DisplayMode {
        DisplayMode {
            width: 1280,
            height: 720,
            fps: 60,
        }
    }

    impl Host {
        fn ask(&self, peer: &Peer, request: Request) -> Response {
            self.sessions
                .answer(&request, peer, LAPTOP.parse().unwrap(), || {
                    vec![app("editor.desktop", "Editor")]
                })
        }

        fn start_as(&self, peer: &Peer, app: &str) -> Response {
            self.ask(
                peer,
                Request::SessionStart {
                    app: app.to_string(),
                    kind: StreamKind::AppWindow,
                    display: mode(),
                    offer: serde_json::json!({"port": 50004}),
                },
            )
        }

        /// Starts the editor for the laptop and returns the first report.
        fn start(&self) -> SessionInfo {
            session(self.start_as(&self.laptop, "editor.desktop"))
        }

        fn wait(&self, id: &SessionId, after: u64, timeout_ms: u32) -> Response {
            self.ask(
                &self.laptop,
                Request::SessionWait {
                    session: id.clone(),
                    after,
                    timeout_ms,
                },
            )
        }

        /// Asks until the session is in `state`, as a client does.
        fn follow_until(&self, id: &SessionId, state: SessionState) -> SessionInfo {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut after = 0;
            loop {
                let info = session(self.wait(id, after, 200));
                if info.state == state {
                    return info;
                }
                assert!(Instant::now() < deadline, "still {:?}", info.state);
                after = info.revision;
            }
        }

        fn stop(&self, id: &SessionId) -> Response {
            self.ask(
                &self.laptop,
                Request::SessionStop {
                    session: id.clone(),
                },
            )
        }
    }

    fn session(response: Response) -> SessionInfo {
        match response {
            Response::Session { session } => *session,
            other => panic!("not a report: {other:?}"),
        }
    }

    fn refusal(response: Response) -> (ErrorCode, String) {
        match response {
            Response::Error { code, message } => (code, message),
            other => panic!("not a refusal: {other:?}"),
        }
    }

    #[test]
    fn an_application_is_started_inside_the_runners_display() {
        let host = host();
        let started = host.start();
        assert!(started.id.is_well_formed());
        assert_eq!(started.app, "editor.desktop");
        assert_eq!(started.state, SessionState::Starting);
        assert_eq!(started.revision, 1);
        assert_eq!(
            (started.display, started.answer, started.end),
            (None, None, None)
        );

        let running = host.follow_until(&started.id, SessionState::Running);
        assert_eq!(running.revision, 2);
        // What the runner said it produces, and its answer, are passed on.
        assert_eq!(running.display, Some(host.runner.display()));
        assert_eq!(running.answer, Some(serde_json::json!({"fake": "answer"})));
        assert_eq!(running.end, None);

        // The runner was told who the picture is for and where input comes
        // from; the device's own word is not what the address is taken from.
        match host.runner.started().pop().unwrap() {
            RunnerCommand::Start {
                session,
                kind,
                display,
                peer,
                input_socket,
                offer,
            } => {
                assert_eq!(session, started.id);
                assert_eq!(kind, StreamKind::AppWindow);
                assert_eq!(display, mode());
                assert_eq!(peer.id, host.laptop.id);
                assert_eq!(peer.address, "192.0.2.10");
                assert_eq!(offer, serde_json::json!({"port": 50004}));
                let socket = std::path::PathBuf::from(input_socket);
                assert_eq!(
                    Some(socket.clone()),
                    Some(host.paths.session_input_socket(&started.id))
                );
                assert_eq!(
                    socket.file_name().unwrap().to_string_lossy(),
                    format!("input-{}.sock", started.id)
                );
                // The place for it is there, and private.
                let dir = socket.parent().unwrap();
                assert!(dir.is_dir());
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = std::fs::metadata(dir).unwrap().permissions().mode();
                    assert_eq!(mode & 0o777, 0o700);
                }
            }
        }
        // The application got the runner's environment, after the runner
        // was ready.
        assert_eq!(
            host.journal.all(),
            [
                "runner start",
                "app launch editor WAYLAND_DISPLAY=wayland-fake"
            ]
        );
    }

    #[test]
    fn stopping_ends_the_application_before_the_stream() {
        let host = host();
        let id = host.start().id;
        host.follow_until(&id, SessionState::Running);

        let answer = session(host.stop(&id));
        assert!(matches!(
            answer.state,
            SessionState::Stopping | SessionState::Ended
        ));
        assert_eq!(answer.end.as_ref().unwrap().reason, EndReason::Stopped);
        let ended = host.follow_until(&id, SessionState::Ended);
        assert_eq!(ended.end.unwrap().reason, EndReason::Stopped);
        assert_eq!(host.journal.all()[2..], ["app stop", "runner stop"]);
        // Asking again does no harm and says the same.
        let again = session(host.stop(&id));
        assert_eq!(again.state, SessionState::Ended);
        assert_eq!(host.journal.all().len(), 4);
    }

    #[test]
    fn an_application_that_ends_takes_its_session_with_it() {
        let host = host();
        let id = host.start().id;
        let running = host.follow_until(&id, SessionState::Running);

        // The client is waiting with a long timeout; the answer comes as
        // soon as the application is gone, not when the time is up.
        let waiter = {
            let (sessions, laptop, id) = (host.sessions.clone(), host.laptop.clone(), id.clone());
            std::thread::spawn(move || {
                let asked = Instant::now();
                let request = Request::SessionWait {
                    session: id,
                    after: running.revision,
                    timeout_ms: 3000,
                };
                let answer = sessions.answer(&request, &laptop, LAPTOP.parse().unwrap(), Vec::new);
                (session(answer), asked.elapsed())
            })
        };
        std::thread::sleep(Duration::from_millis(100));
        host.launcher.end_app("exit status 0");
        let (info, took) = waiter.join().unwrap();
        assert!(matches!(
            info.state,
            SessionState::Stopping | SessionState::Ended
        ));
        assert!(took < Duration::from_millis(1500), "{took:?}");

        let ended = host.follow_until(&id, SessionState::Ended);
        let end = ended.end.unwrap();
        assert_eq!(end.reason, EndReason::AppExited);
        assert_eq!(end.message, "the application ended (exit status 0)");
        // Both are stopped in the usual order. For an application that is
        // already gone there is nothing left to do.
        assert_eq!(host.journal.all()[2..], ["app stop", "runner stop"]);
    }

    #[test]
    fn a_stream_that_cannot_start_ends_the_session_before_any_application() {
        // The runner refuses.
        let host = host();
        host.runner.refuse("no encoder on this machine");
        let id = host.start().id;
        let end = host.follow_until(&id, SessionState::Ended).end.unwrap();
        assert_eq!(end.reason, EndReason::StreamFailed);
        assert_eq!(end.message, "no encoder on this machine");
        assert!(host.journal.all().is_empty());

        // The runner starts, says it cannot go on, and ends.
        let host = self::host();
        host.runner.instead_of_ready(vec![
            StreamEvent::Said(RunnerEvent::Error {
                message: "the GPU is busy".into(),
            }),
            StreamEvent::Exited("exit status 1".into()),
        ]);
        let id = host.start().id;
        let end = host.follow_until(&id, SessionState::Ended).end.unwrap();
        assert_eq!(
            (end.reason, end.message.as_str()),
            (EndReason::StreamFailed, "the GPU is busy")
        );
        assert_eq!(host.journal.all(), ["runner start", "runner stop"]);

        // The runner ends without a word.
        let host = self::host();
        host.runner
            .instead_of_ready(vec![StreamEvent::Exited("signal 11".into())]);
        let id = host.start().id;
        let end = host.follow_until(&id, SessionState::Ended).end.unwrap();
        assert_eq!(end.reason, EndReason::StreamFailed);
        assert!(
            end.message.contains("before it was ready (signal 11)"),
            "{}",
            end.message
        );

        // The runner never says that it is ready.
        let host = host_with(Rules {
            runner_ready_within: Duration::from_millis(150),
            ..quick_rules()
        });
        host.runner.instead_of_ready(Vec::new());
        let id = host.start().id;
        let end = host.follow_until(&id, SessionState::Ended).end.unwrap();
        assert_eq!(end.reason, EndReason::StreamFailed);
        assert!(end.message.contains("not ready within"), "{}", end.message);
        assert_eq!(host.journal.all(), ["runner start", "runner stop"]);
    }

    #[test]
    fn a_stream_that_dies_ends_the_application() {
        let host = host();
        let id = host.start().id;
        host.follow_until(&id, SessionState::Running);
        host.runner.say(StreamEvent::Exited("signal 9".into()));
        let end = host.follow_until(&id, SessionState::Ended).end.unwrap();
        assert_eq!(end.reason, EndReason::StreamFailed);
        assert_eq!(
            end.message,
            "the host's side of the stream ended (signal 9)"
        );
        assert_eq!(host.journal.all()[2..], ["app stop", "runner stop"]);
    }

    #[test]
    fn a_runner_can_say_that_the_last_window_closed() {
        let host = host();
        let id = host.start().id;
        host.follow_until(&id, SessionState::Running);
        host.runner.say(StreamEvent::Said(RunnerEvent::Empty));
        let end = host.follow_until(&id, SessionState::Ended).end.unwrap();
        assert_eq!(end.reason, EndReason::AppExited);
        assert_eq!(host.journal.all()[2..], ["app stop", "runner stop"]);
    }

    #[test]
    fn an_application_that_does_not_start_ends_the_stream() {
        let host = host();
        host.launcher.fail_launch("No such file or directory");
        let id = host.start().id;
        let end = host.follow_until(&id, SessionState::Ended).end.unwrap();
        assert_eq!(end.reason, EndReason::AppFailed);
        assert_eq!(
            end.message,
            "the application could not be started (No such file or directory)"
        );
        assert_eq!(host.journal.all(), ["runner start", "runner stop"]);
    }

    #[test]
    fn a_client_that_stops_asking_loses_its_session() {
        let host = host_with(Rules {
            client_lost_after: Duration::from_millis(400),
            ..quick_rules()
        });
        let id = host.start().id;
        // While the client keeps asking, nothing happens, however long.
        let asked = Instant::now();
        let mut after = 0;
        while asked.elapsed() < Duration::from_millis(1200) {
            let info = session(host.wait(&id, after, 150));
            assert!(
                matches!(info.state, SessionState::Starting | SessionState::Running),
                "{:?}",
                info.end
            );
            after = info.revision;
        }
        // It goes quiet.
        std::thread::sleep(Duration::from_millis(900));
        let info = session(host.wait(&id, 0, 0));
        let end = info.end.expect("the session is being stopped");
        assert_eq!(end.reason, EndReason::ClientLost);
        host.follow_until(&id, SessionState::Ended);
        assert_eq!(host.journal.all()[2..], ["app stop", "runner stop"]);
    }

    #[test]
    fn taking_the_right_away_ends_a_running_session() {
        let host = host();
        let id = host.start().id;
        host.follow_until(&id, SessionState::Running);

        // The laptop may still look, but no longer control.
        let mut record = host.laptop.clone();
        record.permissions = vec![Permission::View];
        PeerStore::open(host.paths.peers())
            .unwrap()
            .add(record)
            .unwrap();
        let ended = host.follow_until(&id, SessionState::Ended);
        assert_eq!(ended.end.unwrap().reason, EndReason::Revoked);
        assert_eq!(host.journal.all()[2..], ["app stop", "runner stop"]);
        // And it cannot start another one.
        let (code, _) = refusal(host.start_as(&host.laptop, "editor.desktop"));
        assert_eq!(code, ErrorCode::Forbidden);
    }

    #[test]
    fn removing_the_pairing_ends_a_running_session() {
        let host = host();
        let id = host.start().id;
        host.follow_until(&id, SessionState::Running);
        PeerStore::open(host.paths.peers())
            .unwrap()
            .remove(&host.laptop.id)
            .unwrap();
        let ended = host.follow_until(&id, SessionState::Ended);
        assert_eq!(ended.end.unwrap().reason, EndReason::Revoked);
    }

    #[test]
    fn a_session_is_only_there_for_the_device_that_started_it() {
        let host = host();
        let id = host.start().id;
        host.follow_until(&id, SessionState::Running);

        let tablet = pair(&host.paths, "tablet", Permission::defaults());
        for request in [
            Request::SessionWait {
                session: id.clone(),
                after: 0,
                timeout_ms: 0,
            },
            Request::SessionStop {
                session: id.clone(),
            },
        ] {
            let (code, _) = refusal(host.ask(&tablet, request));
            assert_eq!(code, ErrorCode::NotFound);
        }
        // A device that took over the name and the id, but not the key.
        let mut impostor = host.laptop.clone();
        impostor.public_key = "00".repeat(32);
        let (code, _) = refusal(host.ask(
            &impostor,
            Request::SessionStop {
                session: id.clone(),
            },
        ));
        assert_eq!(code, ErrorCode::NotFound);
        // The session is untouched.
        assert_eq!(
            host.sessions.look(&id).unwrap().state,
            SessionState::Running
        );

        // Ids that are not ids, and ids of nothing.
        for bad in [
            "",
            "../../etc/passwd",
            "5B0C9E3A7F21D684",
            "0000000000000000",
        ] {
            let (code, _) = refusal(host.wait(&SessionId(bad.to_string()), 0, 0));
            assert_eq!(code, ErrorCode::NotFound, "{bad}");
        }
    }

    #[test]
    fn starting_is_refused_with_a_reason() {
        let host = host();
        // A device that may only look.
        let viewer = pair(&host.paths, "kiosk", vec![Permission::View]);
        let (code, message) = refusal(host.start_as(&viewer, "editor.desktop"));
        assert_eq!(code, ErrorCode::Forbidden);
        assert!(message.contains("may not start"), "{message}");
        // A device whose pairing was removed while its conversation is open.
        let mut gone = host.laptop.clone();
        gone.id = "ffff-ffff-ffff-ffff".into();
        let (code, _) = refusal(host.start_as(&gone, "editor.desktop"));
        assert_eq!(code, ErrorCode::Forbidden);

        // An application the host does not list.
        let (code, _) = refusal(host.start_as(&host.laptop, "/bin/sh"));
        assert_eq!(code, ErrorCode::NotFound);

        // One it lists but cannot show as a window.
        host.launcher
            .refuse_plan("the application runs in a terminal");
        let (code, message) = refusal(host.start_as(&host.laptop, "editor.desktop"));
        assert_eq!(code, ErrorCode::Unavailable);
        assert_eq!(message, "the application runs in a terminal");
        host.launcher.allow_plan();

        // Requests that cannot be met.
        let ask = |kind, width, offer: serde_json::Value| {
            refusal(host.ask(
                &host.laptop,
                Request::SessionStart {
                    app: "editor.desktop".into(),
                    kind,
                    display: DisplayMode {
                        width,
                        height: 720,
                        fps: 60,
                    },
                    offer,
                },
            ))
        };
        assert_eq!(
            ask(StreamKind::Game, 1280, serde_json::Value::Null).0,
            ErrorCode::Unavailable
        );
        assert_eq!(
            ask(StreamKind::AppWindow, 0, serde_json::Value::Null).0,
            ErrorCode::Unavailable
        );
        let huge = serde_json::Value::String("x".repeat(MAX_MEDIA_BYTES));
        let (code, message) = ask(StreamKind::AppWindow, 1280, huge);
        assert_eq!(code, ErrorCode::Unavailable);
        assert!(message.contains("64 KiB"), "{message}");
        // None of this started anything.
        assert!(host.journal.all().is_empty());

        // A host without a runner.
        let bare = Sessions::with(
            host.paths.clone(),
            Arc::new(|_| {}),
            None,
            host.launcher.clone(),
            quick_rules(),
        );
        let request = Request::SessionStart {
            app: "editor.desktop".into(),
            kind: StreamKind::AppWindow,
            display: mode(),
            offer: serde_json::Value::Null,
        };
        let (code, message) = refusal(bare.answer(
            &request,
            &host.laptop,
            LAPTOP.parse().unwrap(),
            || vec![app("editor.desktop", "Editor")],
        ));
        assert_eq!(code, ErrorCode::Unavailable);
        assert!(message.contains("no session runner"), "{message}");
    }

    #[test]
    fn a_host_runs_only_so_many_sessions() {
        let host = host_with(Rules {
            max_sessions: 2,
            ..quick_rules()
        });
        let first = host.start().id;
        let second = host.start().id;
        assert_ne!(first, second);
        let (code, message) = refusal(host.start_as(&host.laptop, "editor.desktop"));
        assert_eq!(code, ErrorCode::Unavailable);
        assert!(message.contains("2 sessions"), "{message}");
        // One that is over makes room.
        session(host.stop(&first));
        host.follow_until(&first, SessionState::Ended);
        host.start();
    }

    #[test]
    fn a_question_is_answered_at_once_or_held_back() {
        let host = host();
        let id = host.start().id;
        let running = host.follow_until(&id, SessionState::Running);

        // Something the caller has not seen: at once.
        let asked = Instant::now();
        assert_eq!(session(host.wait(&id, 0, 3000)), running);
        assert!(asked.elapsed() < Duration::from_millis(300));
        // Nothing new and no patience: at once.
        let asked = Instant::now();
        assert_eq!(session(host.wait(&id, running.revision, 0)), running);
        assert!(asked.elapsed() < Duration::from_millis(300));
        // Nothing new: for as long as the caller said ...
        let asked = Instant::now();
        assert_eq!(session(host.wait(&id, running.revision, 300)), running);
        let took = asked.elapsed();
        assert!(
            took >= Duration::from_millis(280) && took < Duration::from_millis(1500),
            "{took:?}"
        );
        // ... but never longer than the host allows.
        let asked = Instant::now();
        session(host.wait(&id, running.revision, 60_000));
        let took = asked.elapsed();
        let cap = quick_rules().wait_at_most;
        assert!(
            took >= cap - Duration::from_millis(20) && took < cap * 3,
            "{took:?}"
        );
    }

    #[test]
    fn a_session_that_is_over_is_remembered_for_a_while() {
        let host = host_with(Rules {
            remember_ended_for: Duration::from_millis(300),
            ..quick_rules()
        });
        let id = host.start().id;
        host.follow_until(&id, SessionState::Running);
        session(host.stop(&id));
        let ended = host.follow_until(&id, SessionState::Ended);
        // A late question still gets the reason ...
        assert_eq!(session(host.wait(&id, 0, 0)), ended);
        // ... until the host has forgotten.
        std::thread::sleep(Duration::from_millis(400));
        let (code, _) = refusal(host.wait(&id, 0, 0));
        assert_eq!(code, ErrorCode::NotFound);
    }
}
