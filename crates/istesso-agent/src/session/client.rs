//! The client's side of a streaming session: what a device does to open a
//! window for an application that runs on another one.
//!
//! [`open_window`] starts the program that shows the picture (the
//! *viewer*, which belongs to the media path), asks the host to start the
//! application, hands the viewer what the host's side answered, and then
//! keeps three things in view: what the host says about the session, what
//! the viewer does, and whether the person wants to stop. Whichever ends
//! first ends the other two.
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
    /// It is no longer running. `ok` when it ended the way it does after
    /// the person closed the window.
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
// Opening a window
// ---------------------------------------------------------------------------

/// Opens a conversation with the host. Called again whenever one is needed:
/// to follow the session, after a conversation broke, and to say stop.
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
    /// A host that has not answered for this long is gone. The same time
    /// the host gives a client ([`policy::CLIENT_LOST_AFTER`]): by then it
    /// has ended the session anyway.
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
            host_lost_after: policy::CLIENT_LOST_AFTER,
            retry_every: Duration::from_millis(500),
            tick: Duration::from_millis(50),
            viewer_stop_grace: Duration::from_secs(2),
        }
    }
}

/// Why a window that was open is closed again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The person closed the window. The host was asked to stop the
    /// application: in this slice, closing the window ends it.
    Closed,
    /// The person stopped the command.
    Interrupted,
    /// The host ended the session, and says why.
    Ended(SessionEnd),
    /// The host stopped answering, or no longer knows the session.
    HostLost(String),
    /// The viewer broke.
    ViewerFailed(String),
}

/// Why no window was opened.
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

enum HostSaid {
    Report(SessionInfo),
    Lost(String),
}

/// Opens a window for an application of another device and stays until it
/// is closed again.
///
/// `progress` is given every report of the host that changes what a person
/// would want to know: the session was accepted, it runs, it ends.
/// `interrupted` is set from outside when the person wants to stop.
pub fn open_window(
    link: Link,
    viewer: &dyn Viewer,
    opening: &Opening,
    interrupted: &AtomicBool,
    rules: &ClientRules,
    progress: &mut dyn FnMut(&SessionInfo),
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
    let unreachable = |e: istesso_link::Error| OpenError::Unreachable(e.to_string());
    client
        .set_timeout(rules.answer_within)
        .map_err(unreachable)?;
    // Where the conversation goes is where the picture comes from.
    let host = SessionPeer {
        id: opening.host.clone(),
        address: address_text(&client.remote_addr().map_err(unreachable)?),
    };
    let started = match client.session_start(&opening.app, opening.kind, wanted, offer) {
        Ok(started) => started,
        Err(istesso_link::Error::Remote(refusal)) => return Err(OpenError::Refused(refusal)),
        Err(other) => return Err(unreachable(other)),
    };
    progress(&started);
    let session = started.id.clone();

    // One thread keeps asking the host; this one looks at the viewer and
    // the person in between.
    let (sender, reports) = mpsc::channel();
    let quit = Arc::new(AtomicBool::new(false));
    let follower = {
        let (link, session, rules, quit) =
            (link.clone(), session.clone(), rules.clone(), quit.clone());
        let after = started.revision;
        std::thread::spawn(move || follow(client, link, &session, after, &rules, &quit, sender))
    };

    let mut shown = false;
    let (outcome, ask_host_to_stop) = loop {
        match reports.recv_timeout(rules.tick) {
            Ok(HostSaid::Report(info)) => match info.state {
                SessionState::Running if !shown => {
                    shown = true;
                    let show = ViewerCommand::Show {
                        session: session.clone(),
                        host: host.clone(),
                        display: info.display.unwrap_or(wanted),
                        answer: info.answer.clone().unwrap_or_default(),
                    };
                    if let Err(why) = window.viewer.tell(&show) {
                        break (Outcome::ViewerFailed(why), true);
                    }
                    progress(&info);
                }
                SessionState::Stopping | SessionState::Ended => {
                    progress(&info);
                    let end = info.end.unwrap_or(SessionEnd {
                        reason: EndReason::Other,
                        message: String::new(),
                    });
                    break (Outcome::Ended(end), false);
                }
                _ => {}
            },
            Ok(HostSaid::Lost(why)) => break (Outcome::HostLost(why), false),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                break (
                    Outcome::HostLost("the host is no longer followed".into()),
                    true,
                )
            }
        }
        if interrupted.load(Ordering::SeqCst) {
            break (Outcome::Interrupted, true);
        }
        match window.viewer.poll() {
            // In this slice a closed window ends the application: the host
            // is asked to stop the session.
            Some(ViewerSaid::Exited { ok: true, .. }) => break (Outcome::Closed, true),
            Some(ViewerSaid::Exited { ok: false, how }) => {
                break (
                    Outcome::ViewerFailed(format!("the viewer ended ({how})")),
                    true,
                )
            }
            Some(ViewerSaid::Said(ViewerEvent::Error { message })) => {
                break (Outcome::ViewerFailed(message), true)
            }
            _ => {}
        }
    };

    quit.store(true, Ordering::SeqCst);
    drop(window);
    if ask_host_to_stop {
        // On a conversation of its own: the follower's is waiting for an
        // answer, and gets one as soon as the host has heard this.
        if let Ok(mut client) = link() {
            let _ = client.set_timeout(rules.answer_within);
            let _ = client.session_stop(&session);
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
            Some(ViewerSaid::Said(ViewerEvent::Shown)) => continue,
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

/// Keeps asking the host about the session, which also tells the host that
/// this device is still there, and passes on what changes.
fn follow(
    mut client: Client,
    link: Link,
    session: &SessionId,
    mut after: u64,
    rules: &ClientRules,
    quit: &AtomicBool,
    said: Sender<HostSaid>,
) {
    let patience = rules.hold + rules.answer_within;
    let _ = client.set_timeout(patience);
    let mut heard = Instant::now();
    while !quit.load(Ordering::SeqCst) {
        match client.session_wait(session, after, rules.hold) {
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
            // The host answers, and does not have the session (any more).
            Err(istesso_link::Error::Remote(refusal)) => {
                let _ = said.send(HostSaid::Lost(format!(
                    "the host no longer has the session ({})",
                    refusal.message
                )));
                return;
            }
            // The conversation broke. Another one is opened for as long as
            // the host keeps the session.
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
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};

    use istesso_link::{ErrorCode, Request, Response, Session};
    use istesso_pair::{Hello, Peer, Permission};
    use istesso_proto::RunnerCommand;

    use super::super::testing::{app, quick_rules, FakeLauncher, FakeRunner, FakeViewer, Journal};
    use super::super::Sessions;
    use super::*;
    use crate::serve::{Agent, AgentOptions};
    use crate::testing::{self, TempDir};

    /// A pretend device: its own directory, identity and list of peers.
    struct Device {
        dir: TempDir,
        name: &'static str,
        identity: Identity,
    }

    impl Device {
        fn new(name: &'static str) -> Self {
            let dir = TempDir::new(name);
            let identity = Identity::load_or_create(&dir.paths().identity()).unwrap();
            cache::remember_local_manifest(&dir.paths(), &testing::manifest(name));
            Device {
                dir,
                name,
                identity,
            }
        }

        fn paths(&self) -> Paths {
            self.dir.paths()
        }

        fn id(&self) -> String {
            self.identity.fingerprint()
        }

        fn trust(&self, other: &Device, permissions: Vec<Permission>) {
            let mut peer = Peer::from_hello(&Hello {
                device_name: other.name.to_string(),
                public_key: other.identity.public_key(),
            });
            peer.permissions = permissions;
            PeerStore::open(self.paths().peers())
                .unwrap()
                .add(peer)
                .unwrap();
        }
    }

    /// A laptop and a host that are paired, the host's agent with a runner
    /// and a launcher that start nothing, and the laptop's viewer likewise.
    struct Pair {
        laptop: Device,
        host: Device,
        addr: SocketAddr,
        journal: Journal,
        runner: Arc<FakeRunner>,
        launcher: Arc<FakeLauncher>,
        viewer: Arc<FakeViewer>,
    }

    fn pair_on(bind: std::net::IpAddr, rights: Vec<Permission>) -> Option<Pair> {
        let laptop = Device::new("laptop");
        let host = Device::new("host");
        laptop.trust(&host, Permission::defaults());
        host.trust(&laptop, rights);

        let journal = Journal::default();
        let runner = Arc::new(FakeRunner::new(&journal));
        let launcher = Arc::new(FakeLauncher::new(&journal));
        let options = AgentOptions {
            bind,
            port: 0,
            discovery: false,
            refresh_peers: None,
            verbose: false,
        };
        let sessions = Sessions::with(
            host.paths(),
            Arc::new(|_| {}),
            Some(runner.clone()),
            launcher.clone(),
            quick_rules(),
        );
        let agent = Agent::bind(host.paths(), &options, Arc::new(|_| {}))
            .ok()?
            .with_sessions(sessions, vec![app("editor.desktop", "Editor")]);
        let addr = agent.local_addr().unwrap();
        std::thread::spawn(move || agent.run());
        Some(Pair {
            laptop,
            host,
            addr,
            viewer: Arc::new(FakeViewer::new(&journal)),
            journal,
            runner,
            launcher,
        })
    }

    fn pair() -> Pair {
        pair_on(Ipv4Addr::LOCALHOST.into(), Permission::defaults()).unwrap()
    }

    fn quick_client() -> ClientRules {
        ClientRules {
            offer_within: Duration::from_secs(2),
            hold: Duration::from_millis(300),
            answer_within: Duration::from_secs(5),
            host_lost_after: Duration::from_secs(5),
            retry_every: Duration::from_millis(50),
            tick: Duration::from_millis(10),
            viewer_stop_grace: Duration::from_millis(50),
        }
    }

    impl Pair {
        /// The laptop's way to the host, as `istesso open` finds it: by the
        /// address it remembers.
        fn link(&self) -> Link {
            cache::suggest_address(
                &self.laptop.paths(),
                &self.host.id(),
                self.host.name,
                &self.addr.to_string(),
            )
            .unwrap();
            link_to(
                &self.laptop.paths(),
                &self.host.id(),
                Duration::from_secs(5),
            )
            .unwrap()
        }

        fn opening(&self) -> Opening {
            Opening {
                host: self.host.id(),
                app: "editor.desktop".into(),
                title: "Editor".into(),
                kind: StreamKind::AppWindow,
                wish: ViewerWish {
                    x: Some(100),
                    y: Some(50),
                    ..ViewerWish::default()
                },
            }
        }

        /// Opens the window on another thread and waits until it shows.
        fn open(
            &self,
            interrupted: Arc<AtomicBool>,
        ) -> std::thread::JoinHandle<(Result<Outcome, OpenError>, Vec<SessionState>)> {
            let (link, viewer, opening) = (self.link(), self.viewer.clone(), self.opening());
            let handle = std::thread::spawn(move || {
                let mut states = Vec::new();
                let outcome = open_window(
                    link,
                    viewer.as_ref(),
                    &opening,
                    &interrupted,
                    &quick_client(),
                    &mut |info| states.push(info.state),
                );
                (outcome, states)
            });
            self.wait_for("viewer show");
            handle
        }

        fn wait_for(&self, what: &str) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !self.journal.all().iter().any(|line| line == what) {
                assert!(
                    Instant::now() < deadline,
                    "no '{what}' in {:?}",
                    self.journal.all()
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        /// What happened on the host, in order.
        fn on_host(&self) -> Vec<String> {
            self.journal
                .all()
                .into_iter()
                .filter(|line| !line.starts_with("viewer"))
                .collect()
        }
    }

    #[test]
    fn a_window_opens_for_an_application_of_another_device() {
        let pair = pair();
        let window = pair.open(Arc::default());

        // The host's side was told who the picture is for: the address the
        // conversation came from.
        let RunnerCommand::Start {
            session,
            display,
            peer,
            offer,
            ..
        } = pair.runner.started().pop().unwrap();
        assert_eq!(peer.id, pair.laptop.id());
        assert_eq!(peer.address, "127.0.0.1");
        // It got what the viewer asked for ...
        assert_eq!(display, FakeViewer::DISPLAY);
        assert_eq!(offer, serde_json::json!({"fake": "offer"}));
        // ... and the viewer got what the host's side answered.
        let told = pair.viewer.told();
        assert_eq!(
            told[0],
            ViewerCommand::Prepare {
                title: "Editor".into(),
                kind: StreamKind::AppWindow,
                wish: pair.opening().wish,
            }
        );
        assert_eq!(
            told[1],
            ViewerCommand::Show {
                session,
                host: SessionPeer {
                    id: pair.host.id(),
                    address: "127.0.0.1".into(),
                },
                display: pair.runner.display(),
                answer: serde_json::json!({"fake": "answer"}),
            }
        );
        assert_eq!(told.len(), 2);

        // The application ends: the window closes without being asked.
        pair.launcher.end_app("exit status 0");
        let (outcome, states) = window.join().unwrap();
        assert_eq!(
            outcome,
            Ok(Outcome::Ended(SessionEnd {
                reason: EndReason::AppExited,
                message: "the application ended (exit status 0)".into(),
            }))
        );
        // The last report is the one that says it is over; the stand-ins
        // stop so fast that it may already say "ended".
        assert_eq!(states[..2], [SessionState::Starting, SessionState::Running]);
        assert!(matches!(
            states[2..],
            [SessionState::Stopping | SessionState::Ended]
        ));
        pair.wait_for("viewer stop");
        pair.wait_for("runner stop");
        assert_eq!(
            pair.on_host(),
            [
                "runner start",
                "app launch editor WAYLAND_DISPLAY=wayland-fake",
                "app stop",
                "runner stop"
            ]
        );
    }

    #[test]
    fn closing_the_window_ends_the_application() {
        let pair = pair();
        let window = pair.open(Arc::default());
        pair.viewer.close();
        let (outcome, _) = window.join().unwrap();
        assert_eq!(outcome, Ok(Outcome::Closed));
        pair.wait_for("runner stop");
        assert_eq!(pair.on_host()[2..], ["app stop", "runner stop"]);
    }

    #[test]
    fn a_viewer_that_breaks_ends_the_application_too() {
        let pair = pair();
        let window = pair.open(Arc::default());
        pair.viewer.crash();
        let (outcome, _) = window.join().unwrap();
        assert_eq!(
            outcome,
            Ok(Outcome::ViewerFailed("the viewer ended (signal 11)".into()))
        );
        pair.wait_for("runner stop");
    }

    #[test]
    fn stopping_the_command_ends_the_application() {
        let pair = pair();
        let interrupted = Arc::new(AtomicBool::new(false));
        let window = pair.open(interrupted.clone());
        interrupted.store(true, Ordering::SeqCst);
        let (outcome, _) = window.join().unwrap();
        assert_eq!(outcome, Ok(Outcome::Interrupted));
        pair.wait_for("viewer stop");
        pair.wait_for("runner stop");
        assert_eq!(pair.on_host()[2..], ["app stop", "runner stop"]);
    }

    #[test]
    fn a_refusal_closes_the_viewer_and_keeps_its_code() {
        // The host lets the laptop look, not control.
        let pair = pair_on(Ipv4Addr::LOCALHOST.into(), vec![Permission::View]).unwrap();
        let mut reports = 0;
        let outcome = open_window(
            pair.link(),
            pair.viewer.as_ref(),
            &pair.opening(),
            &AtomicBool::new(false),
            &quick_client(),
            &mut |_| reports += 1,
        );
        match outcome {
            Err(OpenError::Refused(refusal)) => {
                assert_eq!(refusal.code, ErrorCode::Forbidden);
                assert!(refusal.message.contains("may not start"), "{refusal}");
            }
            other => panic!("unexpected: {other:?}"),
        }
        assert_eq!(reports, 0);
        assert_eq!(pair.journal.all(), ["viewer start", "viewer stop"]);

        // An application the host does not have.
        let pair = self::pair();
        let opening = Opening {
            app: "nothing.desktop".into(),
            ..pair.opening()
        };
        let outcome = open_window(
            pair.link(),
            pair.viewer.as_ref(),
            &opening,
            &AtomicBool::new(false),
            &quick_client(),
            &mut |_| {},
        );
        assert!(
            matches!(&outcome, Err(OpenError::Refused(refusal)) if refusal.code == ErrorCode::NotFound),
            "{outcome:?}"
        );
    }

    #[test]
    fn over_ipv6_the_picture_is_sent_to_an_ipv6_address() {
        let Some(pair) = pair_on(Ipv6Addr::LOCALHOST.into(), Permission::defaults()) else {
            eprintln!("skipped: this machine has no IPv6 loopback address");
            return;
        };
        assert!(pair.addr.is_ipv6());
        let window = pair.open(Arc::default());
        let RunnerCommand::Start { peer, .. } = pair.runner.started().pop().unwrap();
        assert_eq!(peer.address, "::1");
        match &pair.viewer.told()[1] {
            ViewerCommand::Show { host, .. } => assert_eq!(host.address, "::1"),
            other => panic!("unexpected: {other:?}"),
        }
        pair.viewer.close();
        assert_eq!(window.join().unwrap().0, Ok(Outcome::Closed));
    }

    #[test]
    fn a_host_that_goes_away_closes_the_window() {
        let laptop = Device::new("laptop");
        let host = Device::new("host");
        laptop.trust(&host, Permission::defaults());
        host.trust(&laptop, Permission::defaults());

        // A host that starts a session, says once that it runs, and is gone.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let host_paths = host.paths();
        let stand_in = std::thread::spawn(move || {
            let identity = Identity::load_or_create(&host_paths.identity()).unwrap();
            let keys = LocalKeys::new(&identity).unwrap();
            let peers = PeerStore::open(host_paths.peers()).unwrap();
            let (stream, _) = listener.accept().unwrap();
            let mut session =
                Session::accept(stream, &keys, &peers, Duration::from_secs(10)).unwrap();
            let report = |state, revision| Response::Session {
                session: Box::new(SessionInfo {
                    id: SessionId("5b0c9e3a7f21d684".into()),
                    app: "editor.desktop".into(),
                    kind: StreamKind::AppWindow,
                    state,
                    revision,
                    display: None,
                    answer: None,
                    end: None,
                }),
            };
            for answer in [
                report(SessionState::Starting, 1),
                report(SessionState::Running, 2),
            ] {
                assert!(matches!(
                    session.next_request().unwrap(),
                    Some(Request::SessionStart { .. } | Request::SessionWait { .. })
                ));
                session.respond(&answer).unwrap();
            }
            // Hangs up, and nobody listens at the address any more.
        });

        let journal = Journal::default();
        let viewer = FakeViewer::new(&journal);
        cache::suggest_address(&laptop.paths(), &host.id(), host.name, &addr.to_string()).unwrap();
        let link = link_to(&laptop.paths(), &host.id(), Duration::from_secs(2)).unwrap();
        let rules = ClientRules {
            host_lost_after: Duration::from_millis(400),
            ..quick_client()
        };
        let opening = Opening {
            host: host.id(),
            app: "editor.desktop".into(),
            title: "Editor".into(),
            kind: StreamKind::AppWindow,
            wish: ViewerWish::default(),
        };
        let asked = Instant::now();
        let outcome = open_window(
            link,
            &viewer,
            &opening,
            &AtomicBool::new(false),
            &rules,
            &mut |_| {},
        );
        stand_in.join().unwrap();
        match outcome {
            Ok(Outcome::HostLost(why)) => assert!(why.contains("has not answered"), "{why}"),
            other => panic!("unexpected: {other:?}"),
        }
        // Not at once: the host gets the time it gives a client.
        assert!(asked.elapsed() >= Duration::from_millis(400));
        // The window had been shown without a size from the host: with the
        // one the viewer asked for. It is closed again.
        assert_eq!(
            journal.all(),
            ["viewer start", "viewer show", "viewer stop"]
        );
        assert!(matches!(
            &viewer.told()[1],
            ViewerCommand::Show { display, .. } if *display == FakeViewer::DISPLAY
        ));
    }

    /// The whole way with programs on both sides: the stand-ins of
    /// `scripts/`, which make and show no picture, and an application that
    /// is a real process.
    #[cfg(unix)]
    #[test]
    fn with_programs_on_both_sides_a_session_starts_and_ends() {
        use std::collections::BTreeMap;
        use std::sync::Mutex;

        use istesso_catalog::launch::LaunchPlan;
        use istesso_proto::AppEntry;
        use rustix::process::{test_kill_process, Pid};

        use super::super::{CommandRunner, DesktopLauncher, Launcher, Rules, RunningApp};

        /// Starts a program that writes down its process id and what the
        /// runner gave it, and then stays.
        struct Stays {
            note: PathBuf,
        }
        impl Launcher for Stays {
            fn plan(&self, _app: &AppEntry) -> Result<LaunchPlan, String> {
                Ok(LaunchPlan {
                    program: "sh".into(),
                    args: vec![
                        "-c".into(),
                        format!(
                            "echo \"$$ $ISTESSO_FAKE_SESSION\" > '{}'; exec sleep 60",
                            self.note.display()
                        ),
                    ],
                    working_dir: None,
                })
            }
            fn launch(
                &self,
                plan: &LaunchPlan,
                env: &BTreeMap<String, String>,
            ) -> Result<Box<dyn RunningApp>, String> {
                DesktopLauncher.launch(plan, env)
            }
        }
        let script = |name: &str| {
            PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts")).join(name)
        };
        let within = |what: &str, done: &mut dyn FnMut() -> bool| {
            let deadline = Instant::now() + Duration::from_secs(15);
            while !done() {
                assert!(Instant::now() < deadline, "waited in vain for {what}");
                std::thread::sleep(Duration::from_millis(10));
            }
        };

        let laptop = Device::new("laptop");
        let host = Device::new("host");
        laptop.trust(&host, Permission::defaults());
        host.trust(&laptop, Permission::defaults());
        let said = Arc::new(Mutex::new(Vec::<String>::new()));
        let log: Log = {
            let said = said.clone();
            Arc::new(move |line| said.lock().unwrap().push(line))
        };
        let logged = |part: &str| said.lock().unwrap().iter().any(|line| line.contains(part));

        let note = host.paths().config.join("application.txt");
        let sessions = Sessions::with(
            host.paths(),
            log.clone(),
            Some(Arc::new(CommandRunner::new(
                script("fake-session-runner.sh"),
                log.clone(),
            ))),
            Arc::new(Stays { note: note.clone() }),
            Rules::default(),
        );
        let options = AgentOptions {
            bind: Ipv4Addr::LOCALHOST.into(),
            port: 0,
            discovery: false,
            refresh_peers: None,
            verbose: false,
        };
        let agent = Agent::bind(host.paths(), &options, log.clone())
            .unwrap()
            .with_sessions(sessions, vec![app("editor.desktop", "Editor")]);
        let addr = agent.local_addr().unwrap();
        std::thread::spawn(move || agent.run());
        cache::suggest_address(&laptop.paths(), &host.id(), host.name, &addr.to_string()).unwrap();

        let interrupted = Arc::new(AtomicBool::new(false));
        let window = {
            let link = link_to(&laptop.paths(), &host.id(), Duration::from_secs(5)).unwrap();
            let viewer = CommandViewer::new(script("fake-viewer.sh"), log.clone());
            let opening = Opening {
                host: host.id(),
                app: "editor.desktop".into(),
                title: "Editor".into(),
                kind: StreamKind::AppWindow,
                wish: ViewerWish {
                    width: Some(1720),
                    height: Some(720),
                    ..ViewerWish::default()
                },
            };
            let interrupted = interrupted.clone();
            std::thread::spawn(move || {
                open_window(
                    link,
                    &viewer,
                    &opening,
                    &interrupted,
                    &quick_client(),
                    &mut |_| {},
                )
            })
        };

        // The application runs, with what the runner gave it; the viewer
        // shows what the runner said it makes, which is what the viewer
        // wished for.
        within("the application", &mut || {
            std::fs::read_to_string(&note).is_ok_and(|text| text.ends_with('\n'))
        });
        let text = std::fs::read_to_string(&note).unwrap();
        let (pid, session) = text.trim().split_once(' ').unwrap();
        assert!(SessionId(session.to_string()).is_well_formed(), "{text}");
        let pid = Pid::from_raw(pid.parse().unwrap()).unwrap();
        assert_eq!(test_kill_process(pid), Ok(()));
        within("the viewer", &mut || logged("fake viewer: showing session"));
        assert!(logged(&format!(
            r#"showing session {session}, picture {{"width":1720,"height":720,"fps":60}} from 127.0.0.1"#
        )));
        assert!(logged(&format!(
            r#"runner: fake runner: session {session}, picture {{"width":1720,"height":720,"fps":60}} for 127.0.0.1, input at "#
        )));

        // The person stops the command: the application and both programs
        // are gone, each asked the agreed way.
        interrupted.store(true, Ordering::SeqCst);
        assert_eq!(window.join().unwrap(), Ok(Outcome::Interrupted));
        within("the application to end", &mut || {
            test_kill_process(pid).is_err()
        });
        within("the runner to be asked", &mut || {
            logged("fake runner: asked to stop")
        });
        within("the viewer to be asked", &mut || {
            logged("fake viewer: asked to close")
        });
        within("the session to be reported", &mut || {
            logged("ended: Stopped")
        });
    }

    #[test]
    fn a_viewer_that_says_nothing_opens_no_window() {
        struct Mute;
        impl Viewer for Mute {
            fn start(&self) -> Result<Box<dyn RunningViewer>, String> {
                Ok(Box::new(Mute))
            }
        }
        impl RunningViewer for Mute {
            fn tell(&mut self, _command: &ViewerCommand) -> Result<(), String> {
                Ok(())
            }
            fn poll(&mut self) -> Option<ViewerSaid> {
                None
            }
            fn stop(&mut self, _grace: Duration) {}
        }
        let rules = ClientRules {
            offer_within: Duration::from_millis(100),
            ..quick_client()
        };
        let never: Link =
            Arc::new(|| -> Result<Client, String> { panic!("the host is not asked") });
        let opening = Opening {
            host: "1a2b-3c4d-5e6f-7a8b".into(),
            app: "editor.desktop".into(),
            title: "Editor".into(),
            kind: StreamKind::AppWindow,
            wish: ViewerWish::default(),
        };
        let outcome = open_window(
            never,
            &Mute,
            &opening,
            &AtomicBool::new(false),
            &rules,
            &mut |_| {},
        );
        assert!(
            matches!(&outcome, Err(OpenError::Viewer(why)) if why.contains("did not say")),
            "{outcome:?}"
        );
    }
}
