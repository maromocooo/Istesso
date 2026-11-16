//! Opening a window, against a real agent on this machine whose runner and
//! launcher start nothing; and once all the way, with programs.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};
use std::sync::Mutex;

use istesso_link::{ErrorCode, Request, Response, Session};
use istesso_pair::{Hello, Peer, Permission};
use istesso_proto::RunnerCommand;

use super::super::testing::{app, quick_rules, FakeLauncher, FakeRunner, FakeViewer, Journal};
use super::super::{Rules, Sessions};
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

/// A laptop and a host that are paired, the host's agent with a runner and
/// a launcher that start nothing, and the laptop's viewer likewise.
struct Pair {
    laptop: Device,
    host: Device,
    addr: SocketAddr,
    journal: Journal,
    runner: Arc<FakeRunner>,
    launcher: Arc<FakeLauncher>,
    viewer: Arc<FakeViewer>,
    /// The host's table, as the person at the host sees it.
    sessions: Sessions,
}

/// What the window's thread hands back: how it ended, and what the person
/// was told on the way.
type Opened = std::thread::JoinHandle<(Result<Outcome, OpenError>, Vec<String>)>;

fn pair_on(bind: std::net::IpAddr, rights: Vec<Permission>, rules: Rules) -> Option<Pair> {
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
        rules,
    );
    let agent = Agent::bind(host.paths(), &options, Arc::new(|_| {}))
        .ok()?
        .with_sessions(sessions.clone(), vec![app("editor.desktop", "Editor")]);
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
        sessions,
    })
}

fn pair() -> Pair {
    pair_on(
        Ipv4Addr::LOCALHOST.into(),
        Permission::defaults(),
        quick_rules(),
    )
    .unwrap()
}

fn quick_client() -> ClientRules {
    ClientRules {
        offer_within: Duration::from_secs(2),
        hold: Duration::from_millis(300),
        answer_within: Duration::from_secs(5),
        attach_within: Duration::from_secs(8),
        host_lost_after: Duration::from_secs(5),
        retry_every: Duration::from_millis(50),
        tick: Duration::from_millis(10),
        viewer_stop_grace: Duration::from_millis(50),
    }
}

/// What a person is told while a window is open, as short words.
fn told(progress: Progress<'_>) -> String {
    match progress {
        Progress::Session(info) => format!("{:?}", info.state).to_lowercase(),
        Progress::AskedToClose => "asked to close".to_string(),
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

    fn opening(&self, session: Option<SessionId>) -> Opening {
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
            session,
        }
    }

    /// Opens a window on another thread, with this viewer, and waits until
    /// it has been shown the session `shows` times in all.
    fn open_with(
        &self,
        viewer: Arc<FakeViewer>,
        session: Option<SessionId>,
        interrupted: Arc<AtomicBool>,
        shows: usize,
    ) -> Opened {
        let (link, opening) = (self.link(), self.opening(session));
        let handle = std::thread::spawn(move || {
            let mut heard = Vec::new();
            let outcome = open_window(
                link,
                viewer.as_ref(),
                &opening,
                &interrupted,
                &quick_client(),
                &mut |progress| heard.push(told(progress)),
            );
            (outcome, heard)
        });
        self.journal.wait_for("viewer show", shows);
        handle
    }

    /// Starts the editor and opens its window.
    fn open(&self, interrupted: Arc<AtomicBool>) -> Opened {
        self.open_with(self.viewer.clone(), None, interrupted, 1)
    }

    /// The one session the laptop has on the host.
    fn session(&self) -> SessionInfo {
        let mut sessions = sessions_on(&self.link(), &quick_client()).unwrap();
        assert_eq!(sessions.len(), 1, "{sessions:?}");
        sessions.remove(0)
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
    } = pair.runner.told().remove(0)
    else {
        panic!("the runner was not started");
    };
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
            wish: pair.opening(None).wish,
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
    let (outcome, heard) = window.join().unwrap();
    assert_eq!(
        outcome,
        Ok(Outcome::Ended(SessionEnd {
            reason: EndReason::AppExited,
            message: "the application ended (exit status 0)".into(),
        }))
    );
    // The last report is the one that says it is over; the stand-ins stop
    // so fast that it may already say "ended".
    assert_eq!(heard[..2], ["starting", "running"]);
    assert!(
        matches!(heard[2].as_str(), "stopping" | "ended"),
        "{heard:?}"
    );
    pair.journal.wait_for("viewer stop", 1);
    pair.journal.wait_for("runner stop", 1);
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
fn stopping_the_command_leaves_the_application_running() {
    let pair = pair();
    let interrupted = Arc::new(AtomicBool::new(false));
    let window = pair.open(interrupted.clone());
    let id = pair.session().id;

    // Ctrl-C: the command ends, the window closes, and that is all.
    interrupted.store(true, Ordering::SeqCst);
    let (outcome, _) = window.join().unwrap();
    assert_eq!(outcome, Ok(Outcome::Left(id.clone())));
    pair.journal.wait_for("viewer stop", 1);
    pair.journal.wait_for("runner detach", 1);
    let left = pair.session();
    assert_eq!((left.state, left.attachment), (SessionState::Detached, 1));
    assert_eq!(
        pair.on_host()[2..],
        ["runner detach"],
        "the application was not touched"
    );

    // The same command again comes back to it: another window, another
    // answer from the host's side, the same application.
    let again = Arc::new(FakeViewer::new(&pair.journal));
    let window = pair.open_with(again.clone(), Some(id.clone()), Arc::default(), 2);
    let shown = pair.session();
    assert_eq!((shown.state, shown.attachment), (SessionState::Running, 2));
    assert!(matches!(
        &again.told()[1],
        ViewerCommand::Show { session, display, answer, .. }
            if *session == id
                && *display == FakeViewer::DISPLAY
                && *answer == serde_json::json!({"fake": "answer", "attach": 2})
    ));
    assert_eq!(pair.on_host()[2..], ["runner detach", "runner attach"]);

    pair.launcher.end_app("exit status 0");
    let (outcome, heard) = window.join().unwrap();
    assert!(matches!(outcome, Ok(Outcome::Ended(_))), "{outcome:?}");
    // Coming back, there is nothing to wait for: it runs at once.
    assert_eq!(heard[0], "running");
}

#[test]
fn closing_the_window_asks_the_application_and_waits_for_it() {
    let pair = pair();
    let window = pair.open(Arc::default());

    // The person clicks the close button. The application is asked through
    // the host's side of the stream, and nothing else happens: it has
    // unsaved work, and a question.
    pair.viewer.ask_to_close();
    pair.journal.wait_for("runner close", 1);
    std::thread::sleep(Duration::from_millis(300));
    assert!(!window.is_finished());
    assert_eq!(pair.session().state, SessionState::Running);
    assert_eq!(pair.journal.count("viewer stop"), 0);
    assert_eq!(pair.on_host()[2..], ["runner close"]);

    // The person answers the question; the application ends, and only now
    // does the window close.
    pair.launcher.end_app("exit status 0");
    let (outcome, heard) = window.join().unwrap();
    assert!(
        matches!(&outcome, Ok(Outcome::Ended(end)) if end.reason == EndReason::AppExited),
        "{outcome:?}"
    );
    assert_eq!(heard[..3], ["starting", "running", "asked to close"]);
    pair.journal.wait_for("runner stop", 1);
    assert_eq!(
        pair.on_host()[2..],
        ["runner close", "app stop", "runner stop"]
    );
}

#[test]
fn a_session_can_be_asked_to_close_or_ended_from_elsewhere() {
    let pair = pair();
    let window = pair.open(Arc::default());
    let id = pair.session().id;
    let link = pair.link();

    // Asked to close, the application stays (it has a question). The one
    // who asked is told that it is still there.
    let watch = Duration::from_millis(400);
    let still = stop_session(&link, &id, false, watch, &quick_client()).unwrap();
    assert_eq!((still.state, still.end), (SessionState::Running, None));
    assert_eq!(pair.on_host()[2..], ["runner close"]);
    // Watching did not make the watcher the session's client: the window
    // is still the one that is shown it.
    assert!(!window.is_finished());

    // By force it ends, and the window closes.
    let ended = stop_session(&link, &id, true, Duration::from_secs(5), &quick_client()).unwrap();
    assert_eq!(ended.state, SessionState::Ended);
    assert_eq!(ended.end.unwrap().reason, EndReason::Stopped);
    let (outcome, _) = window.join().unwrap();
    assert!(
        matches!(&outcome, Ok(Outcome::Ended(end)) if end.reason == EndReason::Stopped),
        "{outcome:?}"
    );
    assert_eq!(
        pair.on_host()[2..],
        ["runner close", "app stop", "runner stop"]
    );
    assert!(sessions_on(&link, &quick_client()).unwrap().is_empty());

    // A session that is not there.
    let nothing = SessionId("0000000000000000".into());
    let refused = stop_session(&link, &nothing, false, watch, &quick_client());
    assert!(
        matches!(&refused, Err(OpenError::Refused(refusal)) if refusal.code == ErrorCode::NotFound),
        "{refused:?}"
    );
}

#[test]
fn a_viewer_that_goes_away_leaves_the_application_running() {
    // The person quits the viewer.
    let pair = pair();
    let window = pair.open(Arc::default());
    let id = pair.session().id;
    pair.viewer.quit();
    let (outcome, _) = window.join().unwrap();
    assert_eq!(
        outcome,
        Ok(Outcome::ViewerGone {
            session: id,
            ok: true,
            how: "exit status 0".into()
        })
    );
    pair.journal.wait_for("runner detach", 1);
    assert_eq!(pair.session().state, SessionState::Detached);
    assert_eq!(pair.on_host()[2..], ["runner detach"]);

    // The viewer breaks.
    let pair = self::pair();
    let window = pair.open(Arc::default());
    pair.viewer.crash();
    let (outcome, _) = window.join().unwrap();
    assert!(
        matches!(&outcome, Ok(Outcome::ViewerGone { ok: false, how, .. }) if how == "signal 11"),
        "{outcome:?}"
    );
    pair.journal.wait_for("runner detach", 1);
    assert_eq!(pair.session().state, SessionState::Detached);
}

#[test]
fn a_second_window_takes_the_session_and_the_first_one_closes() {
    let pair = pair();
    let first = pair.open(Arc::default());
    let id = pair.session().id;

    let other = Arc::new(FakeViewer::new(&pair.journal));
    let second = pair.open_with(other, Some(id.clone()), Arc::default(), 2);
    let (outcome, _) = first.join().unwrap();
    assert_eq!(outcome, Ok(Outcome::TakenOver(id.clone())));
    // The first window had nothing to leave: the session is shown, to the
    // second one.
    std::thread::sleep(Duration::from_millis(200));
    let shown = pair.session();
    assert_eq!((shown.state, shown.attachment), (SessionState::Running, 2));
    assert_eq!(pair.on_host()[2..], ["runner attach"]);

    pair.launcher.end_app("exit status 0");
    assert!(matches!(second.join().unwrap().0, Ok(Outcome::Ended(_))));
}

#[test]
fn a_host_that_stops_showing_the_session_closes_the_window() {
    let pair = pair();
    let window = pair.open(Arc::default());
    let id = pair.session().id;

    // The host takes the laptop's right to control away.
    let mut record = PeerStore::open(pair.host.paths().peers()).unwrap().peers()[0].clone();
    record.permissions = vec![Permission::View];
    PeerStore::open(pair.host.paths().peers())
        .unwrap()
        .add(record)
        .unwrap();
    let (outcome, _) = window.join().unwrap();
    // The window closes. Whether the laptop hears of it as the session's
    // last report or as a refusal depends on which reaches it first.
    assert!(
        matches!(
            &outcome,
            Ok(Outcome::Detached(session) | Outcome::HostLost { session, .. }) if *session == id
        ),
        "{outcome:?}"
    );
    // The application runs on, with the person at the host. The laptop
    // does not see it any more, and is not shown it again.
    let kept = pair.sessions.host_look(&id).unwrap();
    assert_eq!(
        (kept.info.state, kept.revoked),
        (SessionState::Detached, true)
    );
    assert_eq!(pair.on_host()[2..], ["runner detach"]);
    let listed = sessions_on(&pair.link(), &quick_client());
    assert!(
        matches!(&listed, Err(OpenError::Refused(refusal)) if refusal.code == ErrorCode::Forbidden),
        "{listed:?}"
    );
    let refused = open_window(
        pair.link(),
        pair.viewer.as_ref(),
        &pair.opening(Some(id)),
        &AtomicBool::new(false),
        &quick_client(),
        &mut |_| {},
    );
    assert!(
        matches!(&refused, Err(OpenError::Refused(refusal)) if refusal.code == ErrorCode::Forbidden),
        "{refused:?}"
    );
}

#[test]
fn a_refusal_closes_the_viewer_and_keeps_its_code() {
    // The host lets the laptop look, not control.
    let pair = pair_on(
        Ipv4Addr::LOCALHOST.into(),
        vec![Permission::View],
        quick_rules(),
    )
    .unwrap();
    let mut reports = 0;
    let outcome = open_window(
        pair.link(),
        pair.viewer.as_ref(),
        &pair.opening(None),
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

    // An application the host does not have, and a session it does not
    // have.
    let pair = self::pair();
    for opening in [
        Opening {
            app: "nothing.desktop".into(),
            ..pair.opening(None)
        },
        pair.opening(Some(SessionId("0000000000000000".into()))),
    ] {
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
    assert!(pair.on_host().is_empty());
}

#[test]
fn over_ipv6_the_picture_is_sent_to_an_ipv6_address() {
    let Some(pair) = pair_on(
        Ipv6Addr::LOCALHOST.into(),
        Permission::defaults(),
        quick_rules(),
    ) else {
        eprintln!("skipped: this machine has no IPv6 loopback address");
        return;
    };
    assert!(pair.addr.is_ipv6());
    let interrupted = Arc::new(AtomicBool::new(false));
    let window = pair.open(interrupted.clone());
    let RunnerCommand::Start { peer, .. } = pair.runner.told().remove(0) else {
        panic!("the runner was not started");
    };
    assert_eq!(peer.address, "::1");
    match &pair.viewer.told()[1] {
        ViewerCommand::Show { host, .. } => assert_eq!(host.address, "::1"),
        other => panic!("unexpected: {other:?}"),
    }
    interrupted.store(true, Ordering::SeqCst);
    assert!(matches!(window.join().unwrap().0, Ok(Outcome::Left(_))));
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
        let mut session = Session::accept(stream, &keys, &peers, Duration::from_secs(10)).unwrap();
        let report = |state, revision| Response::Session {
            session: Box::new(SessionInfo {
                id: SessionId("5b0c9e3a7f21d684".into()),
                app: "editor.desktop".into(),
                kind: StreamKind::AppWindow,
                state,
                revision,
                attachment: 1,
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
                Some(Request::SessionStart { .. } | Request::SessionWait { attachment: 1, .. })
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
        session: None,
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
        Ok(Outcome::HostLost { why, .. }) => assert!(why.contains("has not answered"), "{why}"),
        other => panic!("unexpected: {other:?}"),
    }
    // Not at once: the host gets the time it gives a client.
    assert!(asked.elapsed() >= Duration::from_millis(400));
    // The window had been shown without a size from the host: with the one
    // the viewer asked for. It is closed again.
    assert_eq!(
        journal.all(),
        ["viewer start", "viewer show", "viewer stop"]
    );
    assert!(matches!(
        &viewer.told()[1],
        ViewerCommand::Show { display, .. } if *display == FakeViewer::DISPLAY
    ));
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
    let never: Link = Arc::new(|| -> Result<Client, String> { panic!("the host is not asked") });
    let opening = Opening {
        host: "1a2b-3c4d-5e6f-7a8b".into(),
        app: "editor.desktop".into(),
        title: "Editor".into(),
        kind: StreamKind::AppWindow,
        wish: ViewerWish::default(),
        session: None,
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

/// The whole way with programs on both sides: the stand-ins of `scripts/`,
/// which make and show no picture, and an application that is a real
/// process and behaves like one when it is asked to close.
#[cfg(unix)]
#[test]
fn with_programs_on_both_sides_a_session_is_left_come_back_to_and_closed() {
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;

    use istesso_catalog::launch::LaunchPlan;
    use istesso_proto::AppEntry;
    use rustix::process::{test_kill_process, Pid};

    use super::super::{CommandRunner, DesktopLauncher, Launcher, RunningApp};

    let script = |name: &str| {
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts")).join(name)
    };
    /// Starts the stand-in application, which writes down what happens to
    /// it.
    struct Stays {
        program: PathBuf,
        note: PathBuf,
    }
    impl Launcher for Stays {
        fn plan(&self, _app: &AppEntry) -> Result<LaunchPlan, String> {
            Ok(LaunchPlan {
                program: self.program.display().to_string(),
                args: vec![
                    "--note".into(),
                    self.note.display().to_string(),
                    "60".into(),
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
    let noted = |part: &str| std::fs::read_to_string(&note).is_ok_and(|text| text.contains(part));
    let sessions = Sessions::with(
        host.paths(),
        log.clone(),
        Some(Arc::new(CommandRunner::new(
            script("fake-session-runner.sh"),
            log.clone(),
        ))),
        Arc::new(Stays {
            program: script("fake-application.sh"),
            note: note.clone(),
        }),
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
    let link = link_to(&laptop.paths(), &host.id(), Duration::from_secs(5)).unwrap();
    // A viewer whose window the person closes two seconds after it shows.
    let closing = laptop.paths().config.join("closing-viewer.sh");
    std::fs::write(
        &closing,
        format!(
            "#!/bin/sh\nISTESSO_FAKE_VIEWER_CLOSE_AFTER=2 exec '{}'\n",
            script("fake-viewer.sh").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&closing, std::fs::Permissions::from_mode(0o755)).unwrap();

    let open = |viewer: PathBuf, session: Option<SessionId>, interrupted: Arc<AtomicBool>| {
        let (link, log) = (link.clone(), log.clone());
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
            session,
        };
        std::thread::spawn(move || {
            let viewer = CommandViewer::new(viewer, log);
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

    // The application runs, with what the runner gave it, and the viewer
    // shows what the runner said it makes.
    let interrupted = Arc::new(AtomicBool::new(false));
    let window = open(script("fake-viewer.sh"), None, interrupted.clone());
    within("the application", &mut || noted("started "));
    within("the viewer", &mut || logged("fake viewer: showing session"));
    let listed = sessions_on(&link, &quick_client()).unwrap();
    let id = listed[0].id.clone();
    let text = std::fs::read_to_string(&note).unwrap();
    let mut words = text.lines().next().unwrap().split(' ');
    let pid: i32 = words.nth(1).unwrap().parse().unwrap();
    let pid = Pid::from_raw(pid).unwrap();
    assert_eq!(words.next(), Some(id.0.as_str()), "{text}");
    assert!(logged(&format!(
        r#"showing session {id}, picture {{"width":1720,"height":720,"fps":60}} from 127.0.0.1"#
    )));
    assert!(logged(&format!(
        r#"runner: fake runner: session {id}, picture {{"width":1720,"height":720,"fps":60}} for 127.0.0.1, input at "#
    )));

    // The person stops the command. The window goes; the application and
    // the runner stay.
    interrupted.store(true, Ordering::SeqCst);
    assert_eq!(window.join().unwrap(), Ok(Outcome::Left(id.clone())));
    within("the runner to hear that the client left", &mut || {
        logged("fake runner: nobody is sent the picture now")
    });
    within("the viewer to close", &mut || {
        logged("fake viewer: asked to close its window")
    });
    assert_eq!(test_kill_process(pid), Ok(()));
    let listed = sessions_on(&link, &quick_client()).unwrap();
    assert_eq!(listed[0].state, SessionState::Detached);

    // The command again, for the same session: the same application is
    // shown. This time the person closes the window after a moment. The
    // application is asked, has nothing to save, and ends; so does the
    // session.
    let window = open(closing, Some(id.clone()), Arc::default());
    let outcome = window.join().unwrap();
    assert!(
        matches!(&outcome, Ok(Outcome::Ended(end)) if end.reason == EndReason::AppExited),
        "{outcome:?}"
    );
    assert!(logged("fake runner: picture for 127.0.0.1 again"));
    assert!(logged("fake viewer: the person closes the window"));
    assert!(logged("fake runner: asking the application to close"));
    assert!(noted("asked to close: closing"));
    within("the application to end", &mut || {
        test_kill_process(pid).is_err()
    });
    within("the runner to be stopped", &mut || {
        logged("fake runner: asked to stop")
    });
    within("the session to be reported", &mut || {
        logged("ended: AppExited")
    });
    assert!(sessions_on(&link, &quick_client()).unwrap().is_empty());
}
