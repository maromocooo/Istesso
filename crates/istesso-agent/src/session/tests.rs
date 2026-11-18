//! The host's table of sessions, with a runner and a launcher that start
//! nothing.

use istesso_pair::{Hello, Identity};

use super::local::HostSession;
use super::testing::{app, quick_rules, FakeLauncher, FakeRunner, Journal, OnAttach};
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
    host_named("session", rules)
}

fn host_named(tag: &str, rules: Rules) -> Host {
    let dir = TempDir::new(tag);
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
    fn ask_from(&self, peer: &Peer, from: &str, request: Request) -> Response {
        self.sessions
            .answer(&request, peer, from.parse().unwrap(), || {
                vec![
                    app("editor.desktop", "Editor"),
                    app("paint.desktop", "Paint"),
                ]
            })
    }

    fn ask(&self, peer: &Peer, request: Request) -> Response {
        self.ask_from(peer, LAPTOP, request)
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

    /// Starts the editor and follows it, as its client does, until it runs.
    fn running(&self) -> SessionInfo {
        let started = self.start();
        self.follow_until(&started.id, SessionState::Running)
    }

    /// Asks as the client that is shown the session.
    fn wait_as(&self, id: &SessionId, after: u64, timeout_ms: u32, attachment: u64) -> Response {
        self.ask(
            &self.laptop,
            Request::SessionWait {
                session: id.clone(),
                after,
                timeout_ms,
                attachment,
            },
        )
    }

    /// Asks without being anyone's client.
    fn look(&self, id: &SessionId) -> SessionInfo {
        session(self.wait_as(id, 0, 0, 0))
    }

    /// Asks until the session is in `state`. The questions speak for the
    /// attachment that is shown the session at the time.
    fn follow_until(&self, id: &SessionId, state: SessionState) -> SessionInfo {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut after = 0;
        loop {
            let attachment = self.sessions.look(id).map_or(0, |info| info.attachment);
            let info = session(self.wait_as(id, after, 200, attachment));
            if info.state == state {
                return info;
            }
            assert!(Instant::now() < deadline, "still {:?}", info.state);
            after = info.revision;
        }
    }

    fn stop(&self, id: &SessionId, force: bool) -> Response {
        self.ask(
            &self.laptop,
            Request::SessionStop {
                session: id.clone(),
                force,
            },
        )
    }

    fn attach(&self, id: &SessionId) -> Response {
        self.ask_from(
            &self.laptop,
            "192.0.2.77:50000",
            Request::SessionAttach {
                session: id.clone(),
                display: DisplayMode {
                    width: 800,
                    height: 600,
                    fps: 30,
                },
                offer: serde_json::json!({"port": 50010}),
            },
        )
    }

    fn detach(&self, id: &SessionId, attachment: u64) -> SessionInfo {
        session(self.ask(
            &self.laptop,
            Request::SessionDetach {
                session: id.clone(),
                attachment,
            },
        ))
    }

    fn list(&self, peer: &Peer) -> Vec<SessionInfo> {
        match self.ask(peer, Request::SessionList) {
            Response::Sessions { sessions } => sessions,
            other => panic!("not a list: {other:?}"),
        }
    }

    /// What was done to applications and streams after the session ran.
    fn since_running(&self) -> Vec<String> {
        self.journal.all().split_off(2)
    }

    /// Looks at the session the way the person at the host does, without
    /// asking as any device, until it is as `what` says.
    fn settles(&self, id: &SessionId, what: impl Fn(&HostSession) -> bool) -> HostSession {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let seen = self.sessions.host_look(id).expect("the session is known");
            if what(&seen) {
                return seen;
            }
            assert!(Instant::now() < deadline, "still {seen:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn unpair(&self, peer: &Peer) {
        let mut peers = PeerStore::open(self.paths.peers()).unwrap();
        assert!(peers.remove(&peer.id).unwrap().is_some());
    }

    fn pair_again(&self, peer: &Peer) {
        let mut peers = PeerStore::open(self.paths.peers()).unwrap();
        peers.add(peer.clone()).unwrap();
    }

    /// Every request about a session, as `peer` would make it.
    fn everything_about(&self, id: &SessionId) -> Vec<Request> {
        vec![
            Request::SessionList,
            Request::SessionWait {
                session: id.clone(),
                after: 0,
                timeout_ms: 0,
                attachment: 0,
            },
            Request::SessionAttach {
                session: id.clone(),
                display: mode(),
                offer: serde_json::json!({"port": 50010}),
            },
            Request::SessionDetach {
                session: id.clone(),
                attachment: 1,
            },
            Request::SessionShow {
                session: id.clone(),
                attachment: 1,
                visible: true,
            },
            Request::SessionStop {
                session: id.clone(),
                force: false,
            },
            Request::SessionStop {
                session: id.clone(),
                force: true,
            },
        ]
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
    assert_eq!((started.revision, started.attachment), (1, 1));
    assert_eq!(
        (&started.display, &started.answer, &started.end),
        (&None, &None, &None)
    );

    let running = host.follow_until(&started.id, SessionState::Running);
    assert_eq!((running.revision, running.attachment), (2, 1));
    // What the runner said it produces, and its answer, are passed on.
    assert_eq!(running.display, Some(host.runner.display()));
    assert_eq!(running.answer, Some(serde_json::json!({"fake": "answer"})));
    assert_eq!(running.end, None);

    // The runner was told who the picture is for and where input comes
    // from. The address is the one the conversation came from, not the
    // device's own word.
    match host.runner.told().pop().unwrap() {
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
            // The device's key goes with its id: a runner keeps the
            // picture to the device that has both.
            assert_eq!(
                peer.public_key.as_deref(),
                Some(host.laptop.public_key.as_str())
            );
            assert_eq!(peer.address, "192.0.2.10");
            assert_eq!(offer, serde_json::json!({"port": 50004}));
            let socket = std::path::PathBuf::from(input_socket);
            assert_eq!(socket, host.paths.session_input_socket(&started.id));
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
        other => panic!("unexpected: {other:?}"),
    }
    // The application got the runner's environment, after the runner was
    // ready.
    assert_eq!(
        host.journal.all(),
        [
            "runner start",
            "app launch editor WAYLAND_DISPLAY=wayland-fake"
        ]
    );
}

#[test]
fn a_client_that_goes_away_leaves_the_application_running() {
    let host = host_with(Rules {
        client_lost_after: Duration::from_millis(400),
        ..quick_rules()
    });
    let id = host.start().id;
    // While the client keeps asking, the picture keeps going, however long.
    let asked = Instant::now();
    let mut after = 0;
    while asked.elapsed() < Duration::from_millis(1200) {
        let info = session(host.wait_as(&id, after, 150, 1));
        assert!(
            matches!(info.state, SessionState::Starting | SessionState::Running),
            "{:?}",
            info.state
        );
        after = info.revision;
    }
    // Somebody else asking does not speak for the client.
    let asked = Instant::now();
    while asked.elapsed() < Duration::from_millis(900) {
        host.look(&id);
        std::thread::sleep(Duration::from_millis(50));
    }
    // The lid is closed: the picture stops, and that is all.
    let info = host.look(&id);
    assert_eq!(info.state, SessionState::Detached);
    assert_eq!(info.answer, None);
    assert_eq!((info.end, info.attachment), (None, 1));
    assert_eq!(host.since_running(), ["runner detach"]);
    // Hours later it is still there.
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(host.look(&id).state, SessionState::Detached);
    assert_eq!(host.since_running(), ["runner detach"]);
}

#[test]
fn a_client_comes_back_to_a_session_it_left() {
    let host = host();
    let running = host.running();
    let id = running.id.clone();

    // It says that it leaves; nothing has to time out.
    let left = host.detach(&id, 1);
    assert_eq!(left.state, SessionState::Detached);
    assert_eq!(left.answer, None);
    assert_eq!(host.since_running(), ["runner detach"]);

    // It comes back, from another address and with another window.
    let back = session(host.attach(&id));
    assert_eq!(back.state, SessionState::Running);
    assert_eq!(back.attachment, 2);
    assert_eq!(
        back.display,
        Some(DisplayMode {
            width: 800,
            height: 600,
            fps: 30
        })
    );
    assert_eq!(
        back.answer,
        Some(serde_json::json!({"fake": "answer", "attach": 2}))
    );
    match host.runner.told().pop().unwrap() {
        RunnerCommand::Attach {
            peer,
            display,
            offer,
        } => {
            assert_eq!(peer.id, host.laptop.id);
            // The device's key goes with its id: a runner keeps the
            // picture to the device that has both.
            assert_eq!(
                peer.public_key.as_deref(),
                Some(host.laptop.public_key.as_str())
            );
            assert_eq!(peer.address, "192.0.2.77");
            assert_eq!(display.width, 800);
            assert_eq!(offer, serde_json::json!({"port": 50010}));
        }
        other => panic!("unexpected: {other:?}"),
    }
    // The same application all along.
    assert_eq!(host.since_running(), ["runner detach", "runner attach"]);
    assert_eq!(
        host.journal
            .count("app launch editor WAYLAND_DISPLAY=wayland-fake"),
        1
    );
}

#[test]
fn a_new_window_takes_the_session_from_the_old_one() {
    let host = host_with(Rules {
        client_lost_after: Duration::from_millis(400),
        ..quick_rules()
    });
    let id = host.running().id;
    let taken = session(host.attach(&id));
    assert_eq!((taken.state, taken.attachment), (SessionState::Running, 2));
    // No detach in between: the runner switches.
    assert_eq!(host.since_running(), ["runner attach"]);

    // The old window sees that it has been replaced. Its questions no
    // longer keep the picture going, and its leaving changes nothing.
    let seen_by_old = session(host.wait_as(&id, 0, 0, 1));
    assert_eq!(seen_by_old.attachment, 2);
    assert_eq!(host.detach(&id, 1).state, SessionState::Running);
    let asked = Instant::now();
    while asked.elapsed() < Duration::from_millis(900) {
        session(host.wait_as(&id, 0, 100, 1));
    }
    assert_eq!(host.look(&id).state, SessionState::Detached);
    assert_eq!(host.since_running(), ["runner attach", "runner detach"]);
}

#[test]
fn a_client_that_cannot_be_attached_is_told_and_the_application_stays() {
    // The runner says that it cannot.
    let host = host();
    let id = host.running().id;
    host.detach(&id, 1);
    host.runner
        .on_attach(OnAttach::Refuse("the encoder is busy".into()));
    let (code, message) = refusal(host.attach(&id));
    assert_eq!(code, ErrorCode::Unavailable);
    assert_eq!(message, "the encoder is busy");
    let info = host.look(&id);
    assert_eq!((info.state, info.attachment), (SessionState::Detached, 1));
    // The next try works.
    host.runner.on_attach(OnAttach::Attach);
    assert_eq!(session(host.attach(&id)).attachment, 2);

    // The runner says nothing.
    let host = host_with(Rules {
        attach_within: Duration::from_millis(200),
        ..quick_rules()
    });
    let id = host.running().id;
    host.runner.on_attach(OnAttach::Ignore);
    let asked = Instant::now();
    let (code, message) = refusal(host.attach(&id));
    assert_eq!(code, ErrorCode::Unavailable);
    assert!(message.contains("did not take the client"), "{message}");
    assert!(asked.elapsed() >= Duration::from_millis(200));
    // The window that was shown it has lost it; the application has not
    // been touched.
    assert_eq!(host.look(&id).state, SessionState::Detached);
    assert_eq!(host.since_running(), ["runner attach", "runner detach"]);

    // A runner that stops sending by itself is believed.
    let host = self::host();
    let id = host.running().id;
    host.runner.say(StreamEvent::Said(RunnerEvent::Detached {
        message: "the client does not take the picture".into(),
    }));
    assert_eq!(host.follow_until(&id, SessionState::Detached).answer, None);

    // What cannot be attached to.
    let host = self::host();
    host.runner.instead_of_ready(Vec::new());
    let id = host.start().id;
    let (code, message) = refusal(host.attach(&id));
    assert_eq!(code, ErrorCode::Unavailable);
    assert!(message.contains("still starting"), "{message}");
    session(host.stop(&id, true));
    host.follow_until(&id, SessionState::Ended);
    let (code, message) = refusal(host.attach(&id));
    assert_eq!(code, ErrorCode::Unavailable);
    assert!(message.contains("ending"), "{message}");
}

#[test]
fn asking_an_application_to_close_leaves_the_answer_to_it() {
    let host = host();
    let id = host.running().id;

    // The answer comes at once and nothing has changed: asking is all.
    let asked = session(host.stop(&id, false));
    assert_eq!((asked.state, asked.end), (SessionState::Running, None));
    host.journal.wait_for("runner close", 1);
    // An application with unsaved work has a question, and stays. So does
    // the session, and its client.
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(host.look(&id).state, SessionState::Running);
    assert_eq!(host.since_running(), ["runner close"]);

    // Asked again, it goes.
    session(host.stop(&id, false));
    host.journal.wait_for("runner close", 2);
    host.launcher.end_app("exit status 0");
    let ended = host.follow_until(&id, SessionState::Ended);
    let end = ended.end.unwrap();
    assert_eq!(end.reason, EndReason::AppExited);
    assert_eq!(end.message, "the application ended (exit status 0)");
    assert_eq!(
        host.since_running(),
        ["runner close", "runner close", "runner stop"]
    );
    // Asking something that is over to close does nothing.
    assert_eq!(session(host.stop(&id, false)).state, SessionState::Ended);
    assert_eq!(host.journal.count("runner close"), 2);
}

#[test]
fn a_session_nobody_is_shown_can_be_asked_to_close_too() {
    let host = host();
    let id = host.running().id;
    host.detach(&id, 1);
    session(host.stop(&id, false));
    host.journal.wait_for("runner close", 1);
    // The runner says when the last window is gone.
    host.runner.say(StreamEvent::Said(RunnerEvent::Empty));
    let end = host.follow_until(&id, SessionState::Ended).end.unwrap();
    assert_eq!(end.reason, EndReason::AppExited);
    // Its processes were still there: asked to end, they did.
    assert_eq!(
        end.message,
        "the application closed its last window; it ended when asked to (signal 15)"
    );
    assert_eq!(
        host.since_running(),
        [
            "runner detach",
            "runner close",
            "app asked to end",
            "runner stop"
        ]
    );
}

#[test]
fn ending_by_force_stops_the_application_before_the_stream() {
    let host = host();
    let id = host.running().id;

    let answer = session(host.stop(&id, true));
    assert!(matches!(
        answer.state,
        SessionState::Stopping | SessionState::Ended
    ));
    assert_eq!(answer.end.as_ref().unwrap().reason, EndReason::Stopped);
    let ended = host.follow_until(&id, SessionState::Ended);
    assert_eq!(ended.end.unwrap().reason, EndReason::Stopped);
    assert_eq!(ended.answer, None);
    // The application is not asked to close first: the two ways are kept
    // apart.
    assert_eq!(host.since_running(), ["app ended by force", "runner stop"]);
    // Asking again does no harm and says the same.
    let again = session(host.stop(&id, true));
    assert_eq!(again.state, SessionState::Ended);
    assert_eq!(host.journal.all().len(), 4);
}

#[test]
fn an_application_that_ends_takes_its_session_with_it() {
    let host = host();
    let running = host.running();
    let id = running.id.clone();

    // The client is waiting with a long timeout; the answer comes as soon
    // as the application is gone, not when the time is up.
    let waiter = {
        let (sessions, laptop, id) = (host.sessions.clone(), host.laptop.clone(), id.clone());
        std::thread::spawn(move || {
            let asked = Instant::now();
            let request = Request::SessionWait {
                session: id,
                after: running.revision,
                timeout_ms: 3000,
                attachment: 1,
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
    // already gone there is nothing left to do: nothing is sent to it.
    assert_eq!(host.since_running(), ["runner stop"]);

    // The same when nobody is shown it.
    let id = host.running().id;
    host.detach(&id, 1);
    host.launcher.end_app("exit status 0");
    assert_eq!(
        host.follow_until(&id, SessionState::Ended)
            .end
            .unwrap()
            .reason,
        EndReason::AppExited
    );
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
    // The display is gone with it; there is nothing the application could
    // be shown on.
    let host = host();
    let id = host.running().id;
    host.runner.say(StreamEvent::Exited("signal 9".into()));
    let end = host.follow_until(&id, SessionState::Ended).end.unwrap();
    assert_eq!(end.reason, EndReason::StreamFailed);
    // Without its display the application is asked to end, never forced.
    assert_eq!(
        end.message,
        "the host's side of the stream ended (signal 9); it ended when asked to (signal 15)"
    );
    assert_eq!(host.since_running(), ["app asked to end", "runner stop"]);
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
fn a_client_that_leaves_while_the_session_starts_is_not_shown_it() {
    let host = host();
    host.runner.instead_of_ready(Vec::new());
    let id = host.start().id;
    assert_eq!(host.detach(&id, 1).state, SessionState::Starting);
    // The runner gets there in the end; the application starts, for
    // nobody to see yet.
    std::thread::sleep(Duration::from_millis(100));
    host.runner.say(StreamEvent::Said(RunnerEvent::Ready {
        env: BTreeMap::new(),
        display: mode(),
        answer: serde_json::Value::Null,
    }));
    let info = host.follow_until(&id, SessionState::Detached);
    assert_eq!(info.display, Some(mode()));
    assert_eq!(
        host.journal.all(),
        [
            "runner start",
            "app launch editor WAYLAND_DISPLAY=-",
            "runner detach"
        ]
    );
}

#[test]
fn taking_the_right_away_stops_the_picture_not_the_application() {
    let host = host();
    let id = host.running().id;

    // The laptop may still look, but no longer control.
    let mut record = host.laptop.clone();
    record.permissions = vec![Permission::View];
    host.pair_again(&record);
    let seen = host.settles(&id, |s| s.info.state == SessionState::Detached);
    assert!(seen.revoked);
    assert_eq!(seen.info.end, None);
    assert_eq!(host.since_running(), ["runner detach"]);
    // What starting takes, everything about the session takes: the laptop
    // is not shown it again, and does not list, follow or end it either.
    for request in host.everything_about(&id) {
        let (code, message) = refusal(host.ask(&host.laptop, request));
        assert_eq!(code, ErrorCode::Forbidden);
        assert!(message.contains("may not use applications"), "{message}");
    }
    let (code, message) = refusal(host.start_as(&host.laptop, "editor.desktop"));
    assert_eq!(code, ErrorCode::Forbidden);
    assert!(message.contains("may not start"), "{message}");
    // None of that reached the application or the stream.
    assert_eq!(host.since_running(), ["runner detach"]);
    assert_eq!(
        host.sessions.look(&id).unwrap().state,
        SessionState::Detached
    );
}

#[test]
fn a_session_nobody_is_shown_is_closed_to_a_device_that_was_unpaired() {
    let host = host();
    let id = host.running().id;
    assert_eq!(host.detach(&id, 1).state, SessionState::Detached);
    assert_eq!(host.since_running(), ["runner detach"]);
    assert!(!host.sessions.host_look(&id).unwrap().revoked);

    // Nobody asks anything: the session's own round finds out.
    let unpaired = Instant::now();
    host.unpair(&host.laptop);
    let seen = host.settles(&id, |s| s.revoked);
    assert!(unpaired.elapsed() < Duration::from_secs(2));
    assert_eq!(seen.info.state, SessionState::Detached);
    // It is kept, and says from now on who opened it.
    assert_eq!(seen.info.left_by.as_deref(), Some("laptop"));
    assert_eq!(
        (seen.started_by.name.as_str(), &seen.started_by.id),
        ("laptop", &host.laptop.id)
    );

    // The device gets nothing: not the list, not the picture, and no way
    // to end the application.
    for request in host.everything_about(&id) {
        let (code, message) = refusal(host.ask(&host.laptop, request));
        assert_eq!(code, ErrorCode::Forbidden);
        assert!(message.contains("no longer paired"), "{message}");
    }
    // The application runs on, with its place in the table.
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(host.since_running(), ["runner detach"]);
    let seen = host.sessions.host_look(&id).unwrap();
    assert_eq!(
        (seen.info.state, &seen.info.end),
        (SessionState::Detached, &None)
    );

    // The person at the host sees it, whoever started it, and can ask it
    // to close ...
    let all = host.sessions.host_list();
    assert_eq!(all.len(), 1);
    assert_eq!((&all[0].info.id, all[0].revoked), (&id, true));
    assert_eq!(all[0].app_name, "Editor");
    let asked = host.sessions.host_stop(&id, false).unwrap();
    assert_eq!(asked.info.state, SessionState::Detached);
    host.journal.wait_for("runner close", 1);
    assert_eq!(host.journal.count("app asked to end"), 0);
    // ... or end it.
    host.sessions.host_stop(&id, true).unwrap();
    let over = host.settles(&id, |s| s.info.state == SessionState::Ended);
    assert_eq!(over.info.end.unwrap().reason, EndReason::Stopped);
    assert_eq!(
        host.since_running(),
        [
            "runner detach",
            "runner close",
            "app ended by force",
            "runner stop"
        ]
    );
    assert!(host.sessions.host_list().is_empty());
    assert_eq!(
        host.sessions
            .host_stop(&SessionId("0000000000000000".into()), true)
            .unwrap_err()
            .0,
        ErrorCode::NotFound
    );
}

#[test]
fn unpairing_stops_the_picture_and_pairing_again_gives_the_session_back() {
    let host = host();
    let id = host.running().id;
    // What the media path agreed on is not part of what the host's person
    // is told.
    let shown = host.sessions.host_look(&id).unwrap();
    assert_eq!(
        (shown.info.state, &shown.info.answer),
        (SessionState::Running, &None)
    );
    assert!(host.sessions.look(&id).unwrap().answer.is_some());

    host.unpair(&host.laptop);
    let seen = host.settles(&id, |s| s.info.state == SessionState::Detached);
    assert!(seen.revoked);
    assert_eq!(host.since_running(), ["runner detach"]);

    // The same device, paired again, finds its application as it left it.
    host.pair_again(&host.laptop);
    host.settles(&id, |s| !s.revoked);
    let listed = host.list(&host.laptop);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].left_by.as_deref(), Some("laptop"));
    let back = session(host.attach(&id));
    assert_eq!((back.state, back.attachment), (SessionState::Running, 2));
    assert_eq!(host.since_running(), ["runner detach", "runner attach"]);
}

#[test]
fn nothing_is_started_for_a_device_that_was_unpaired_meanwhile() {
    // While the runner is still getting ready ...
    let host = host();
    host.runner.instead_of_ready(Vec::new());
    let id = host.start().id;
    host.unpair(&host.laptop);
    let over = host.settles(&id, |s| s.info.state == SessionState::Ended);
    let end = over.info.end.unwrap();
    assert_eq!(end.reason, EndReason::Stopped);
    assert!(end.message.contains("may no longer start"), "{end:?}");
    assert_eq!(host.journal.all(), ["runner start", "runner stop"]);

    // ... or at the moment it is.
    let host = host_with(Rules {
        recheck_rights_every: Duration::from_secs(3600),
        ..quick_rules()
    });
    host.runner.instead_of_ready(Vec::new());
    let id = host.start().id;
    host.journal.wait_for("runner start", 1);
    // The round has read the rights once, and will not again for an hour.
    std::thread::sleep(Duration::from_millis(100));
    host.unpair(&host.laptop);
    host.runner.say(StreamEvent::Said(RunnerEvent::Ready {
        env: BTreeMap::new(),
        display: mode(),
        answer: serde_json::Value::Null,
    }));
    host.settles(&id, |s| s.info.state == SessionState::Ended);
    assert_eq!(host.journal.all(), ["runner start", "runner stop"]);
}

#[test]
fn a_session_left_by_an_unpaired_device_is_open_to_devices_with_the_rights() {
    let host = host();
    let id = host.running().id;
    let tablet = pair(&host.paths, "tablet", Permission::defaults());
    let phone = pair(&host.paths, "phone", vec![Permission::View]);
    // It carries no mark while its device is paired.
    assert_eq!(host.list(&tablet)[0].left_by, None);

    host.unpair(&host.laptop);
    let kept = host.settles(&id, |s| {
        s.info.left_by.is_some() && s.info.state == SessionState::Detached
    });
    assert_eq!(kept.info.left_by.as_deref(), Some("laptop"));
    assert_eq!(host.since_running(), ["runner detach"]);

    // The tablet finds it in its list, marked as opened by a device that
    // was unpaired ...
    let listed = host.list(&tablet);
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (&listed[0].id, listed[0].left_by.as_deref(), listed[0].state),
        (&id, Some("laptop"), SessionState::Detached)
    );
    // ... and is shown it: the picture goes to the tablet from now on.
    let attach = Request::SessionAttach {
        session: id.clone(),
        display: mode(),
        offer: serde_json::json!({"port": 50020}),
    };
    let shown = session(host.ask_from(&tablet, "192.0.2.88:50000", attach.clone()));
    assert_eq!((shown.state, shown.attachment), (SessionState::Running, 2));
    match host.runner.told().pop().unwrap() {
        RunnerCommand::Attach { peer, .. } => {
            assert_eq!(peer.id, tablet.id);
            assert_eq!(peer.address, "192.0.2.88");
        }
        other => panic!("unexpected: {other:?}"),
    }
    // The laptop is still unpaired. That is nothing to the tablet's
    // picture, however often the list of paired devices is read.
    let as_tablet = |request| host.ask_from(&tablet, "192.0.2.88:50000", request);
    let look = Request::SessionWait {
        session: id.clone(),
        after: 0,
        timeout_ms: 0,
        attachment: 2,
    };
    std::thread::sleep(Duration::from_millis(200));
    let seen = session(as_tablet(look.clone()));
    assert_eq!(seen.state, SessionState::Running);
    assert_eq!(host.since_running(), ["runner detach", "runner attach"]);
    // What the media path agreed on with the tablet is told to the window
    // that is shown the session, and to nobody else that may use it.
    assert!(seen.answer.is_some());
    assert_eq!(host.list(&tablet)[0].answer, None);
    let asking = Request::SessionWait {
        session: id.clone(),
        after: 0,
        timeout_ms: 0,
        attachment: 0,
    };
    assert_eq!(session(as_tablet(asking)).answer, None);

    // "Every device that has the rights", and no other. A paired device
    // that may only look is refused whatever it asks, about this session
    // as about one that does not exist, so it does not learn which is
    // there. The laptop, unpaired, is refused likewise.
    let nothing = SessionId("0000000000000000".to_string());
    for (device, why) in [
        (&phone, "may not use applications"),
        (&host.laptop, "no longer paired"),
    ] {
        for about in [&id, &nothing] {
            for request in host.everything_about(about) {
                let (code, message) = refusal(host.ask(device, request));
                assert_eq!(code, ErrorCode::Forbidden);
                assert!(message.contains(why), "{message}");
            }
        }
    }
    // None of that reached the application or its stream.
    assert_eq!(host.since_running(), ["runner detach", "runner attach"]);
    assert_eq!(
        session(as_tablet(look.clone())).state,
        SessionState::Running
    );

    // The picture stops for the device that is shown the session when
    // that device loses a right, whoever started the session.
    let mut record = tablet.clone();
    record.permissions = vec![Permission::View];
    host.pair_again(&record);
    host.settles(&id, |s| s.info.state == SessionState::Detached);
    assert_eq!(
        host.since_running(),
        ["runner detach", "runner attach", "runner detach"]
    );
    host.pair_again(&tablet);

    // Paired again, the laptop is one device among the others: the
    // session stays marked, and open to all of them.
    host.pair_again(&host.laptop);
    host.settles(&id, |s| !s.revoked);
    assert_eq!(
        host.list(&host.laptop)[0].left_by.as_deref(),
        Some("laptop")
    );
    assert_eq!(host.list(&tablet).len(), 1);
    // The same machine paired anew, with a key it did not have before, is
    // a device like the tablet: it finds the session as one that a device
    // called "laptop" left.
    let renewed = pair(&host.paths, "laptop", Permission::defaults());
    assert_ne!(renewed.id, host.laptop.id);
    let listed = host.list(&renewed);
    assert_eq!(
        (listed.len(), listed[0].left_by.as_deref()),
        (1, Some("laptop"))
    );
    // Any of them can end it.
    session(as_tablet(Request::SessionStop {
        session: id.clone(),
        force: true,
    }));
    let over = host.settles(&id, |s| s.info.state == SessionState::Ended);
    assert_eq!(over.info.end.unwrap().reason, EndReason::Stopped);
}

#[test]
fn nobody_uses_a_session_without_the_rights() {
    // Every device that has what starting takes ...
    assert!(policy::may_use(&Permission::defaults()));
    assert!(policy::may_use(&[
        Permission::Audio,
        Permission::Control,
        Permission::View
    ]));
    // ... and nobody with less, whoever started the session.
    for lacking in [
        vec![],
        vec![Permission::View],
        vec![Permission::Control],
        vec![Permission::View, Permission::Audio],
    ] {
        assert!(!policy::may_use(&lacking), "{lacking:?}");
    }
}

#[test]
fn a_list_of_paired_devices_that_cannot_be_read_is_not_an_unpairing() {
    let host = host();
    let tablet = pair(&host.paths, "tablet", Permission::defaults());
    let id = host.running().id;
    let list = host.paths.peers();
    let good = std::fs::read(&list).unwrap();

    for broken in ["missing", "half-written", "empty"] {
        match broken {
            "missing" => std::fs::remove_file(&list).unwrap(),
            "half-written" => std::fs::write(&list, &good[..good.len() / 2]).unwrap(),
            _ => std::fs::write(&list, "").unwrap(),
        }
        // Nothing is known now, so nothing is allowed: the picture that
        // was being sent stops within a round or two.
        let broke = Instant::now();
        let seen = host.settles(&id, |s| s.info.state == SessionState::Detached);
        assert!(broke.elapsed() < Duration::from_secs(2), "{broken}");
        // Nothing is concluded either. The session has no mark, and its
        // application runs on.
        assert_eq!(
            (seen.revoked, &seen.info.left_by),
            (false, &None),
            "{broken}"
        );
        assert_eq!(host.journal.count("app asked to end"), 0, "{broken}");
        // Whoever asks is refused, the device that started the session
        // included, whatever it asks: to start, to list, to follow, to be
        // shown, to leave, to stop. Nothing is widened.
        for device in [&host.laptop, &tablet] {
            for request in host.everything_about(&id) {
                let (code, message) = refusal(host.ask(device, request));
                assert_eq!(code, ErrorCode::Unavailable, "{broken}");
                assert!(
                    message.contains("cannot read its list of paired devices"),
                    "{message}"
                );
            }
            let (code, _) = refusal(host.start_as(device, "paint.desktop"));
            assert_eq!(code, ErrorCode::Unavailable, "{broken}");
        }
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(
            host.sessions.host_look(&id).unwrap().info.state,
            SessionState::Detached,
            "{broken}"
        );
        // The list is back. The session is there as before, without a
        // mark, and can be shown again.
        std::fs::write(&list, &good).unwrap();
        assert_eq!(host.look(&id).state, SessionState::Detached);
        assert_eq!(host.list(&tablet)[0].left_by, None);
        assert_eq!(session(host.attach(&id)).state, SessionState::Running);
    }
    // The stream was told to stop three times and to send again three
    // times. The application was never touched.
    assert_eq!(
        (
            host.journal.count("runner detach"),
            host.journal.count("runner attach"),
            host.journal.count("app asked to end"),
            host.journal.count("runner stop")
        ),
        (3, 3, 0, 0)
    );
    assert_eq!(host.sessions.host_look(&id).unwrap().info.left_by, None);

    // A list that was read and does not hold the device is another matter.
    host.unpair(&host.laptop);
    let left = host.settles(&id, |s| s.info.left_by.is_some());
    assert_eq!(left.info.state, SessionState::Detached);
    assert_eq!(host.list(&tablet).len(), 1);
}

#[test]
fn an_attachment_belongs_to_the_device_it_was_given_to() {
    // The laptop is shown the session as attachment 1. The tablet may use
    // every session, and the number is easy to guess.
    let host = host_with(Rules {
        client_lost_after: Duration::from_millis(400),
        ..quick_rules()
    });
    let id = host.running().id;
    let tablet = pair(&host.paths, "tablet", Permission::defaults());
    let as_first = |device: &Peer, request: Request| session(host.ask(device, request));
    let wait = Request::SessionWait {
        session: id.clone(),
        after: 0,
        timeout_ms: 0,
        attachment: 1,
    };
    let hide = Request::SessionShow {
        session: id.clone(),
        attachment: 1,
        visible: false,
    };
    let leave = Request::SessionDetach {
        session: id.clone(),
        attachment: 1,
    };

    // Naming the laptop's number, the tablet is not told what the media
    // path agreed on with the laptop, does not hide the laptop's window,
    // and does not leave for it.
    assert_eq!(as_first(&tablet, wait.clone()).answer, None);
    assert_eq!(as_first(&tablet, hide.clone()).paused, None);
    assert_eq!(
        as_first(&tablet, leave.clone()).state,
        SessionState::Running
    );
    std::thread::sleep(Duration::from_millis(100));
    assert!(host.since_running().is_empty());
    // The laptop is told, with the same number.
    assert!(as_first(&host.laptop, wait.clone()).answer.is_some());

    // The tablet's questions do not stand in for the laptop's either: the
    // laptop falls silent, the tablet keeps asking in its name, and the
    // picture stops when the laptop has been gone for long enough.
    let silent = Instant::now();
    while silent.elapsed() < Duration::from_millis(900) {
        if as_first(&tablet, wait.clone()).state == SessionState::Detached {
            break;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    assert_eq!(
        host.sessions.host_look(&id).unwrap().info.state,
        SessionState::Detached
    );
    assert_eq!(host.since_running(), ["runner detach"]);

    // Its own questions keep it there for as long as it asks, and it can
    // hide its own window and leave.
    let back = session(host.attach(&id));
    assert_eq!((back.state, back.attachment), (SessionState::Running, 2));
    let mine = |request: fn(SessionId) -> Request| as_first(&host.laptop, request(id.clone()));
    let asking = Instant::now();
    while asking.elapsed() < Duration::from_millis(900) {
        let seen = mine(|session| Request::SessionWait {
            session,
            after: 0,
            timeout_ms: 0,
            attachment: 2,
        });
        assert_eq!(seen.state, SessionState::Running);
        std::thread::sleep(Duration::from_millis(40));
    }
    let hidden = mine(|session| Request::SessionShow {
        session,
        attachment: 2,
        visible: false,
    });
    assert_eq!(hidden.paused, Some(PauseReason::Hidden));
    mine(|session| Request::SessionDetach {
        session,
        attachment: 2,
    });
    host.settles(&id, |s| s.info.state == SessionState::Detached);
}

#[test]
fn input_for_a_session_is_taken_from_the_device_that_is_shown_it() {
    let host = host();
    let id = host.running().id;
    let tablet = pair(&host.paths, "tablet", Permission::defaults());
    // The laptop is shown the session: its input goes to the session's own
    // socket. The tablet may use the session, and is not shown it.
    assert_eq!(
        host.sessions.input_socket_of(&id, &host.laptop),
        Some(host.paths.session_input_socket(&id))
    );
    assert_eq!(host.sessions.input_socket_of(&id, &tablet), None);
    // Nobody is shown it: nobody's input is taken.
    host.detach(&id, 1);
    host.settles(&id, |s| s.info.state == SessionState::Detached);
    assert!(!host.sessions.shown_to(&id, &host.laptop));
    // The tablet attaches: now it is the tablet's, and not the laptop's.
    session(host.ask_from(
        &tablet,
        "192.0.2.88:50000",
        Request::SessionAttach {
            session: id.clone(),
            display: mode(),
            offer: serde_json::Value::Null,
        },
    ));
    assert!(host.sessions.shown_to(&id, &tablet));
    assert!(!host.sessions.shown_to(&id, &host.laptop));
    assert!(!host
        .sessions
        .shown_to(&SessionId("0000000000000000".into()), &tablet));
}

#[test]
fn an_encoder_that_is_refused_stops_the_picture_of_a_window_nobody_looks_at() {
    // Eight slots by the setting, and three windows: room enough on paper.
    let host = host_with(Rules {
        encoder_retry_after: Duration::from_millis(700),
        ..quick_rules()
    });
    let paused = |id: &SessionId| host.sessions.host_look(id).unwrap().info.paused;
    let first = host.running().id;
    let second = session(host.start_as(&host.laptop, "paint.desktop")).id;
    host.follow_until(&second, SessionState::Running);
    let third = session(host.start_as(&host.laptop, "editor.desktop")).id;
    host.follow_until(&third, SessionState::Running);
    let before = host.journal.all().len();

    // The card refuses the third window's encoder in the middle of the
    // run: another program on the machine took one.
    host.runner.say(StreamEvent::Said(RunnerEvent::NoEncoder {
        message: "the card gives no more".to_string(),
    }));
    // Nothing crashes and nothing ends. The window that was turned to
    // longest ago gives its picture up, and its report says why, for its
    // client to tell the person.
    host.settles(&first, |s| s.info.paused == Some(PauseReason::NoSlot));
    let told = session(host.wait_as(&first, 0, 0, 1));
    assert_eq!(
        (told.state, told.paused),
        (SessionState::Running, Some(PauseReason::NoSlot))
    );
    // Its runner gives the encoder back first; then the one that was
    // refused is told to try again.
    host.journal.wait_for("runner pause", 1);
    host.journal.wait_for("runner resume", 1);
    assert_eq!(
        host.journal.all()[before..],
        ["runner pause", "runner resume"]
    );
    assert_eq!((paused(&second), paused(&third)), (None, None));
    for id in [&first, &second, &third] {
        assert_eq!(
            host.sessions.host_look(id).unwrap().info.state,
            SessionState::Running
        );
    }
    // A window that is opened meanwhile is not refused either: it takes
    // the place of the one that was turned to longest ago.
    let fourth = session(host.start_as(&host.laptop, "paint.desktop")).id;
    host.follow_until(&fourth, SessionState::Running);
    host.settles(&second, |s| s.info.paused == Some(PauseReason::NoSlot));
    assert_eq!(paused(&fourth), None);

    // After a while the host tries what it is set to again.
    host.settles(&first, |s| s.info.paused.is_none());
    host.settles(&second, |s| s.info.paused.is_none());
    assert_eq!(host.journal.count("app asked to end"), 0);
    assert_eq!(host.journal.count("runner stop"), 0);
}

#[test]
fn a_single_window_whose_encoder_is_refused_waits_and_tries_again() {
    let host = host_with(Rules {
        encoder_retry_after: Duration::from_millis(400),
        ..quick_rules()
    });
    let id = host.running().id;
    host.runner.say(StreamEvent::Said(RunnerEvent::NoEncoder {
        message: String::new(),
    }));
    // The session is as it was, attached and running, with no picture and
    // the reason for it. Its runner is told nothing: it has no encoder to
    // give back.
    let seen = host.settles(&id, |s| s.info.paused == Some(PauseReason::NoSlot));
    assert_eq!(
        (seen.info.state, seen.info.attachment),
        (SessionState::Running, 1)
    );
    std::thread::sleep(Duration::from_millis(100));
    assert!(host.since_running().is_empty());
    // Then it is told to try again, and the picture is back.
    host.settles(&id, |s| s.info.paused.is_none());
    host.journal.wait_for("runner resume", 1);
    assert_eq!(host.since_running(), ["runner resume"]);
    // Refused once more, it waits once more.
    host.runner.say(StreamEvent::Said(RunnerEvent::NoEncoder {
        message: String::new(),
    }));
    host.settles(&id, |s| s.info.paused == Some(PauseReason::NoSlot));
    assert_eq!(host.journal.count("app asked to end"), 0);
}

#[test]
fn a_refusal_from_a_window_that_holds_no_encoder_changes_nothing() {
    let host = host();
    let first = host.running().id;
    let second = session(host.start_as(&host.laptop, "paint.desktop")).id;
    host.follow_until(&second, SessionState::Running);
    // The second window cannot be seen: it holds no encoder.
    let hidden = session(host.ask(
        &host.laptop,
        Request::SessionShow {
            session: second.clone(),
            attachment: 1,
            visible: false,
        },
    ));
    assert_eq!(hidden.paused, Some(PauseReason::Hidden));
    host.journal.wait_for("runner pause", 1);
    // Its runner says, late, that the encoder was refused. The host does
    // not count fewer pictures for that: the first window keeps its own.
    host.runner.say(StreamEvent::Said(RunnerEvent::NoEncoder {
        message: String::new(),
    }));
    std::thread::sleep(Duration::from_millis(150));
    let paused = |id: &SessionId| host.sessions.host_look(id).unwrap().info.paused;
    assert_eq!(
        (paused(&first), paused(&second)),
        (None, Some(PauseReason::Hidden))
    );
    assert_eq!(host.journal.count("runner pause"), 1);
}

#[test]
fn a_session_can_be_there_and_hold_no_encoder() {
    let host = host();
    let id = host.running().id;
    let seen = |visible: bool, attachment: u64| {
        session(host.ask(
            &host.laptop,
            Request::SessionShow {
                session: id.clone(),
                attachment,
                visible,
            },
        ))
    };

    // The window cannot be seen: no picture is made for it. The session
    // is as it was otherwise: running, attached, its application alive.
    let hidden = seen(false, 1);
    assert_eq!(
        (hidden.state, hidden.paused, hidden.attachment),
        (SessionState::Running, Some(PauseReason::Hidden), 1)
    );
    host.journal.wait_for("runner pause", 1);
    // A window that was replaced has no say in it.
    assert_eq!(seen(true, 7).paused, Some(PauseReason::Hidden));
    // Its client still asks about it, so it is not taken for gone.
    assert_eq!(host.look(&id).state, SessionState::Running);

    // Seen again: the picture is made again. Nothing was agreed anew.
    assert_eq!(seen(true, 1).paused, None);
    host.journal.wait_for("runner resume", 1);
    assert_eq!(host.since_running(), ["runner pause", "runner resume"]);

    // Hidden, then left: a session nobody is shown has no picture to stop
    // and none to bring back, and comes back with one when it is shown
    // again.
    seen(false, 1);
    host.journal.wait_for("runner pause", 2);
    assert_eq!(host.detach(&id, 1).paused, None);
    std::thread::sleep(Duration::from_millis(100));
    let back = session(host.attach(&id));
    assert_eq!((back.state, back.paused), (SessionState::Running, None));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        host.since_running()[2..],
        ["runner pause", "runner detach", "runner attach"]
    );
    assert_eq!(host.journal.count("app asked to end"), 0);
}

#[test]
fn a_window_beyond_the_encoder_slots_is_opened_with_its_picture_stopped() {
    let host = host_with(Rules {
        encoder_slots: 2,
        ..quick_rules()
    });
    let paused = |id: &SessionId| host.sessions.host_look(id).unwrap().info.paused;
    let first = host.running().id;
    let second = session(host.start_as(&host.laptop, "paint.desktop")).id;
    host.follow_until(&second, SessionState::Running);
    assert_eq!((paused(&first), paused(&second)), (None, None));

    // A third window. It is not refused: it opens and gets its picture,
    // and the window that was turned to longest ago gives its slot up. It
    // stays open and attached, and says why it shows nothing new.
    let third = session(host.start_as(&host.laptop, "editor.desktop")).id;
    host.follow_until(&third, SessionState::Running);
    host.journal.wait_for("runner pause", 1);
    assert_eq!(
        (paused(&first), paused(&second), paused(&third)),
        (Some(PauseReason::NoSlot), None, None)
    );
    let listed = host.list(&host.laptop);
    assert_eq!(listed.len(), 3);
    assert_eq!(
        (listed[0].state, listed[0].paused),
        (SessionState::Running, Some(PauseReason::NoSlot))
    );

    // The person turns to the first window. It gets a slot, and the one
    // that was turned to longest ago now is the second.
    session(host.ask(
        &host.laptop,
        Request::SessionShow {
            session: first.clone(),
            attachment: 1,
            visible: true,
        },
    ));
    host.journal.wait_for("runner resume", 1);
    host.journal.wait_for("runner pause", 2);
    assert_eq!(
        (paused(&first), paused(&second), paused(&third)),
        (None, Some(PauseReason::NoSlot), None)
    );

    // A window that cannot be seen needs no slot, so the second gets it.
    session(host.ask(
        &host.laptop,
        Request::SessionShow {
            session: third.clone(),
            attachment: 1,
            visible: false,
        },
    ));
    host.journal.wait_for("runner resume", 2);
    assert_eq!(
        (paused(&first), paused(&second), paused(&third)),
        (None, None, Some(PauseReason::Hidden))
    );
    // Neither does a session nobody is shown, or one that is over.
    session(host.ask(
        &host.laptop,
        Request::SessionShow {
            session: third.clone(),
            attachment: 1,
            visible: true,
        },
    ));
    host.settles(&second, |s| s.info.paused == Some(PauseReason::NoSlot));
    host.detach(&third, 1);
    host.settles(&second, |s| s.info.paused.is_none());
    assert_eq!(host.journal.count("app asked to end"), 0);
    assert_eq!(Rules::default().encoder_slots, 8);
}

/// Asks on another thread, as a device whose question the host holds back.
fn asking(host: &Host, request: Request) -> std::thread::JoinHandle<Response> {
    let (sessions, laptop) = (host.sessions.clone(), host.laptop.clone());
    std::thread::spawn(move || {
        sessions.answer(&request, &laptop, LAPTOP.parse().unwrap(), Vec::new)
    })
}

fn cannot_read(response: Response) {
    let (code, message) = refusal(response);
    assert_eq!(code, ErrorCode::Unavailable);
    assert!(
        message.contains("cannot read its list of paired devices"),
        "{message}"
    );
}

#[test]
fn a_question_that_is_held_back_is_not_answered_once_the_list_cannot_be_read() {
    let host = host();
    let running = host.running();
    let id = running.id.clone();
    let list = host.paths.peers();
    let good = std::fs::read(&list).unwrap();

    // The laptop's window waits for news of its session, and was let in
    // when it asked. The list goes while the host holds the answer back.
    let held = asking(
        &host,
        Request::SessionWait {
            session: id.clone(),
            after: running.revision,
            timeout_ms: 450,
            attachment: 1,
        },
    );
    std::thread::sleep(Duration::from_millis(100));
    std::fs::remove_file(&list).unwrap();
    cannot_read(held.join().unwrap());
    host.settles(&id, |s| s.info.state == SessionState::Detached);
    std::fs::write(&list, &good).unwrap();

    // The same for a device that asked to be shown the session and is
    // waiting for the host's side of the stream, which takes its time.
    host.runner.on_attach(OnAttach::Ignore);
    let held = asking(
        &host,
        Request::SessionAttach {
            session: id.clone(),
            display: mode(),
            offer: serde_json::json!({"port": 50010}),
        },
    );
    host.journal.wait_for("runner attach", 1);
    std::fs::write(&list, "").unwrap();
    cannot_read(held.join().unwrap());
    // The stream was told to stop sending to it, and nobody is shown the
    // session. The application runs on, without a mark.
    assert_eq!(
        host.since_running(),
        ["runner detach", "runner attach", "runner detach"]
    );
    let seen = host.sessions.host_look(&id).unwrap();
    assert_eq!(
        (seen.info.state, seen.info.attachment, &seen.info.left_by),
        (SessionState::Detached, 1, &None)
    );
    assert_eq!(host.journal.count("app asked to end"), 0);
}

#[test]
fn nobody_is_attached_while_the_list_cannot_be_read() {
    let host = host();
    let id = host.running().id;
    host.detach(&id, 1);
    host.settles(&id, |s| s.info.state == SessionState::Detached);
    std::fs::remove_file(host.paths.peers()).unwrap();

    // An order to show the session that is already in the table when the
    // list goes: the device was let in when it asked, a moment before.
    let number = 7;
    {
        let mut table = host.sessions.inner.lock();
        let entry = table.sessions.get_mut(&id.0).unwrap();
        entry.orders.attach = Some(Attach {
            number,
            device: Device::of(&host.laptop),
            peer: SessionPeer {
                id: host.laptop.id.clone(),
                address: "192.0.2.10".to_string(),
                public_key: None,
            },
            display: mode(),
            offer: serde_json::Value::Null,
        });
    }
    host.sessions.inner.changed.notify_all();
    // The round that takes the order reads the list first, cannot, and
    // turns the order down. The host's side of the stream never hears of
    // it.
    let deadline = Instant::now() + Duration::from_secs(5);
    let outcome = loop {
        let went = {
            let table = host.sessions.inner.lock();
            let entry = &table.sessions[&id.0];
            entry
                .attached
                .iter()
                .find(|(n, _)| *n == number)
                .map(|(_, outcome)| outcome.clone())
        };
        if let Some(outcome) = went {
            break outcome;
        }
        assert!(Instant::now() < deadline, "the order was not taken");
        std::thread::sleep(Duration::from_millis(5));
    };
    let why = outcome.unwrap_err();
    assert!(
        why.contains("cannot read its list of paired devices"),
        "{why}"
    );
    assert_eq!(host.since_running(), ["runner detach"]);
    assert_eq!(
        host.sessions.host_look(&id).unwrap().info.state,
        SessionState::Detached
    );
}

#[test]
fn nothing_is_started_while_the_list_cannot_be_read() {
    let host = host();
    host.runner.instead_of_ready(Vec::new());
    let id = host.start().id;
    host.journal.wait_for("runner start", 1);
    // The session is still starting when the list goes. The host's side
    // of the stream gets ready after that.
    std::fs::write(host.paths.peers(), "{ not a list").unwrap();
    std::thread::sleep(Duration::from_millis(100));
    host.runner.say(StreamEvent::Said(RunnerEvent::Ready {
        env: BTreeMap::new(),
        display: mode(),
        answer: serde_json::Value::Null,
    }));
    // No application is started for anybody while nobody can be known to
    // be allowed, and the session says why it ended.
    let over = host.settles(&id, |s| s.info.state == SessionState::Ended);
    let end = over.info.end.unwrap();
    assert_eq!(end.reason, EndReason::AppFailed);
    assert!(
        end.message
            .contains("cannot read its list of paired devices"),
        "{end:?}"
    );
    assert_eq!(host.journal.all(), ["runner start", "runner stop"]);
    assert_eq!(over.info.left_by, None);
}

#[cfg(unix)]
#[test]
fn the_person_at_the_host_asks_on_a_socket_only_they_can_reach() {
    use std::os::unix::fs::PermissionsExt;

    use super::local::{after_unpairing, listen, serve, sessions_here, stop_here, HostLink};

    let mode_of = |path: &std::path::Path| {
        std::fs::symlink_metadata(path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    };
    let open_to_all = |path: &std::path::Path| {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    };

    // A short name: the path of a socket may only be so long.
    let host = host_named("s", quick_rules());
    let socket = host.paths.agent_socket();
    let dir = socket.parent().unwrap();
    // No agent: said, not guessed.
    let none = sessions_here(&host.paths).unwrap_err();
    assert!(none.contains("no istesso agent"), "{none}");

    // A directory that others could enter is closed before the socket is
    // opened in it.
    std::fs::create_dir_all(dir).unwrap();
    open_to_all(dir);
    let listener = listen(&host.paths).unwrap();
    assert_eq!(mode_of(dir), 0o700);
    assert_eq!(mode_of(&socket), 0o600);
    // One agent answers there, not two.
    let second = listen(&host.paths).unwrap_err();
    assert_eq!(second.kind(), std::io::ErrorKind::AddrInUse, "{second}");
    let sessions = host.sessions.clone();
    std::thread::spawn(move || {
        serve(
            listener,
            Arc::new(move |request| sessions.answer_host(request)),
        );
    });

    assert_eq!(sessions_here(&host.paths).unwrap(), []);
    let id = host.running().id;
    let listed = sessions_here(&host.paths).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (&listed[0].info.id, listed[0].started_by.name.as_str()),
        (&id, "laptop")
    );
    assert_eq!(listed[0], host.sessions.host_look(&id).unwrap());

    // Asked to close, the application may stay; ended, it is over.
    let asked = stop_here(&host.paths, &id, false, Duration::from_millis(100)).unwrap();
    assert_eq!(asked.info.state, SessionState::Running);
    assert_eq!(host.since_running(), ["runner close"]);
    let over = stop_here(&host.paths, &id, true, Duration::from_secs(5)).unwrap();
    assert_eq!(over.info.state, SessionState::Ended);
    assert_eq!(over.info.end.unwrap().reason, EndReason::Stopped);
    let unknown = SessionId("0000000000000000".into());
    let refused = stop_here(&host.paths, &unknown, true, Duration::ZERO).unwrap_err();
    assert_eq!(refused, "no such session");

    // Something that is not a request gets an answer that says so, and
    // the conversation goes on.
    {
        use std::io::{BufRead, BufReader, Write};
        let mut raw = std::os::unix::net::UnixStream::connect(&socket).unwrap();
        let mut lines = BufReader::new(raw.try_clone().unwrap());
        let mut say = |line: &str| {
            writeln!(raw, "{line}").unwrap();
            let mut answer = String::new();
            lines.read_line(&mut answer).unwrap();
            answer
        };
        let answer = say(r#"{"v":1,"ask":"shutdown"}"#);
        assert!(answer.contains(r#""code":"unsupported""#), "{answer}");
        let answer = say(r#"{"v":1,"ask":"sessions"}"#);
        assert_eq!(
            answer,
            "{\"v\":1,\"answer\":\"sessions\",\"sessions\":[]}\n"
        );
    }

    // The asking side does not believe a socket in a directory that
    // somebody else could have put it in.
    open_to_all(dir);
    let Err(doubt) = HostLink::open(&host.paths) else {
        panic!("asked through a directory that is open to others");
    };
    let doubt = doubt.to_string();
    assert!(doubt.contains("can be entered by other users"), "{doubt}");

    // A socket that an agent left behind is replaced.
    let other = host_named("s", quick_rules());
    let left = other.paths.agent_socket();
    std::fs::create_dir_all(left.parent().unwrap()).unwrap();
    drop(std::os::unix::net::UnixListener::bind(&left).unwrap());
    assert!(left.exists());
    // (A program that another test starts at this very moment holds the
    // closed listener open until it has started, so once may not do.)
    let replaced = (0..100).any(|_| {
        listen(&other.paths).is_ok() || {
            std::thread::sleep(Duration::from_millis(10));
            false
        }
    });
    assert!(replaced);
    // Something else of that name is not touched.
    let third = host_named("s", quick_rules());
    let taken = third.paths.agent_socket();
    std::fs::create_dir_all(taken.parent().unwrap()).unwrap();
    std::fs::write(&taken, "not a socket").unwrap();
    assert!(listen(&third.paths).is_err());
    assert_eq!(std::fs::read_to_string(&taken).unwrap(), "not a socket");

    // An agent that has no runtime directory, as when it is started at
    // boot, listens next to the cache. Somebody who asks from a login,
    // where there is one, finds it there.
    let boot = host_named("s", quick_rules());
    let no_place = boot.paths.config.join("not-a-directory");
    std::fs::write(&no_place, "").unwrap();
    let at_boot = Paths {
        runtime: no_place.join("run"),
        ..boot.paths.clone()
    };
    let listener = listen(&at_boot).unwrap();
    assert!(boot.paths.cache.join("run/agent.sock").exists());
    assert!(!boot.paths.agent_socket().exists());
    let sessions = boot.sessions.clone();
    std::thread::spawn(move || {
        serve(
            listener,
            Arc::new(move |request| sessions.answer_host(request)),
        );
    });
    let from_a_login = boot.paths.clone();
    std::fs::create_dir_all(&from_a_login.runtime).unwrap();
    std::fs::set_permissions(
        &from_a_login.runtime,
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let id = boot.running().id;
    assert_eq!(sessions_here(&from_a_login).unwrap()[0].info.id, id);
    // The command that removes a pairing learns what the device left.
    let left = after_unpairing(&from_a_login, &boot.laptop.id).unwrap();
    assert_eq!(left.unwrap().len(), 1);
    assert_eq!(
        after_unpairing(&from_a_login, "nobody").unwrap(),
        Some(vec![])
    );
    // Where no agent runs, nothing of that device does.
    let nobody = host_named("s", quick_rules());
    assert_eq!(after_unpairing(&nobody.paths, "nobody"), Ok(None));
}

#[test]
fn a_session_is_there_for_every_device_with_the_rights_one_at_a_time() {
    let host = host();
    let id = host.running().id;
    assert_eq!(host.look(&id).shown_on.as_deref(), Some("laptop"));

    // Another device of the same person finds the application that the
    // laptop opened, and who is shown it ...
    let tablet = pair(&host.paths, "tablet", Permission::defaults());
    let listed = host.list(&tablet);
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (
            &listed[0].id,
            listed[0].shown_on.as_deref(),
            &listed[0].left_by
        ),
        (&id, Some("laptop"), &None)
    );
    // ... and carries on with it. The picture goes to the tablet, and to
    // the tablet alone: the runner sends to one address at a time.
    let shown = session(host.ask_from(
        &tablet,
        "192.0.2.88:50000",
        Request::SessionAttach {
            session: id.clone(),
            display: mode(),
            offer: serde_json::json!({"port": 50020}),
        },
    ));
    assert_eq!(
        (shown.state, shown.attachment, shown.shown_on.as_deref()),
        (SessionState::Running, 2, Some("tablet"))
    );
    assert_eq!(host.since_running(), ["runner attach"]);
    match host.runner.told().pop().unwrap() {
        RunnerCommand::Attach { peer, .. } => {
            assert_eq!(
                (peer.id, peer.address.as_str()),
                (tablet.id.clone(), "192.0.2.88")
            );
        }
        other => panic!("unexpected: {other:?}"),
    }
    // The laptop's window, which asks as the first to be shown, learns
    // that the session moved, and where to. It is not told what the media
    // path agreed on with the tablet.
    let moved = session(host.wait_as(&id, 0, 0, 1));
    assert_eq!(
        (moved.attachment, moved.shown_on.as_deref(), &moved.answer),
        (2, Some("tablet"), &None)
    );
    // Its leaving, late, does nothing to the tablet's picture.
    assert_eq!(host.detach(&id, 1).state, SessionState::Running);
    assert_eq!(host.since_running(), ["runner attach"]);

    // A device that may only look gets nothing.
    let phone = pair(&host.paths, "phone", vec![Permission::View]);
    for request in host.everything_about(&id) {
        let (code, _) = refusal(host.ask(&phone, request));
        assert_eq!(code, ErrorCode::Forbidden);
    }
    assert_eq!(host.since_running(), ["runner attach"]);

    // Ids that are not ids, and ids of nothing.
    for bad in [
        "",
        "../../etc/passwd",
        "5B0C9E3A7F21D684",
        "0000000000000000",
    ] {
        let (code, _) = refusal(host.wait_as(&SessionId(bad.to_string()), 0, 0, 0));
        assert_eq!(code, ErrorCode::NotFound, "{bad}");
    }
}

#[test]
fn a_device_sees_its_sessions_in_the_order_it_started_them() {
    let host = host();
    assert!(host.list(&host.laptop).is_empty());
    let first = host.running().id;
    let second = session(host.start_as(&host.laptop, "paint.desktop")).id;
    host.follow_until(&second, SessionState::Running);
    host.detach(&first, 1);

    let listed = host.list(&host.laptop);
    assert_eq!(
        listed
            .iter()
            .map(|s| (s.id.clone(), s.app.as_str(), s.state))
            .collect::<Vec<_>>(),
        [
            (first.clone(), "editor.desktop", SessionState::Detached),
            (second.clone(), "paint.desktop", SessionState::Running)
        ]
    );
    // One that is over is no longer listed, though it is still remembered.
    session(host.stop(&first, true));
    host.follow_until(&first, SessionState::Ended);
    assert_eq!(host.list(&host.laptop).len(), 1);
    assert_eq!(host.look(&first).state, SessionState::Ended);
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
fn a_full_host_refuses_and_names_what_nobody_is_shown() {
    let host = host_with(Rules {
        max_sessions: 3,
        ..quick_rules()
    });
    let first = host.running().id;
    let second = session(host.start_as(&host.laptop, "paint.desktop")).id;
    host.follow_until(&second, SessionState::Running);
    // Another device has one as well.
    let tablet = pair(&host.paths, "tablet", Permission::defaults());
    let third = session(host.start_as(&tablet, "editor.desktop")).id;
    assert_ne!(first, third);

    // Full. Sessions that nobody is shown count like the others, nothing
    // is ended to make room, and the refusal says what the limit is and
    // where it is set.
    let full = "this host already keeps 3 sessions open, as many as it is set to \
                (`istesso agent --max-sessions`)";
    let (code, message) = refusal(host.start_as(&host.laptop, "editor.desktop"));
    assert_eq!(code, ErrorCode::Unavailable);
    assert_eq!(message, full);
    host.detach(&first, 1);
    host.detach(&second, 1);
    let (code, message) = refusal(host.start_as(&host.laptop, "editor.desktop"));
    assert_eq!(code, ErrorCode::Unavailable);
    assert_eq!(
        message,
        format!("{full}; nobody is shown: Editor ({first}), Paint ({second})")
    );
    assert_eq!(host.journal.count("app asked to end"), 0);

    // What an unpaired device left behind keeps its place, and says whose
    // it was. Another device can end one to make room.
    host.unpair(&host.laptop);
    host.settles(&first, |s| s.info.left_by.is_some());
    host.settles(&second, |s| s.info.left_by.is_some());
    let (_, message) = refusal(host.start_as(&tablet, "paint.desktop"));
    assert_eq!(
        message,
        format!(
            "{full}; nobody is shown: Editor ({first}, opened by laptop), \
             Paint ({second}, opened by laptop)"
        )
    );
    assert_eq!(host.journal.count("app asked to end"), 0);
    session(host.ask(
        &tablet,
        Request::SessionStop {
            session: first.clone(),
            force: true,
        },
    ));
    host.settles(&first, |s| s.info.state == SessionState::Ended);
    session(host.start_as(&tablet, "paint.desktop"));

    // The limit is a safety value, far above what a person opens.
    assert_eq!(Rules::default().max_sessions, 64);
}

#[test]
fn a_question_is_answered_at_once_or_held_back() {
    let host = host();
    let running = host.running();
    let id = running.id.clone();

    // Something the caller has not seen: at once.
    let asked = Instant::now();
    assert_eq!(session(host.wait_as(&id, 0, 3000, 1)), running);
    assert!(asked.elapsed() < Duration::from_millis(300));
    // Nothing new and no patience: at once.
    let asked = Instant::now();
    assert_eq!(session(host.wait_as(&id, running.revision, 0, 1)), running);
    assert!(asked.elapsed() < Duration::from_millis(300));
    // Nothing new: for as long as the caller said ...
    let asked = Instant::now();
    assert_eq!(
        session(host.wait_as(&id, running.revision, 300, 1)),
        running
    );
    let took = asked.elapsed();
    assert!(
        took >= Duration::from_millis(280) && took < Duration::from_millis(1500),
        "{took:?}"
    );
    // ... but never longer than the host allows.
    let asked = Instant::now();
    session(host.wait_as(&id, running.revision, 60_000, 1));
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
    let id = host.running().id;
    session(host.stop(&id, true));
    let ended = host.follow_until(&id, SessionState::Ended);
    // A late question still gets the reason ...
    assert_eq!(host.look(&id), ended);
    // ... until the host has forgotten.
    std::thread::sleep(Duration::from_millis(400));
    let (code, _) = refusal(host.wait_as(&id, 0, 0, 0));
    assert_eq!(code, ErrorCode::NotFound);
}

// ---------------------------------------------------------------------------
// Ending an application: asked, never forced but on a person's word
// (ADR 0010, 3; the owner's decisions of 2026-11-17)
// ---------------------------------------------------------------------------

/// One way a session can come to its end, done to a running session.
type Ending = fn(&Host, &SessionId);

#[test]
fn no_way_of_ending_but_a_persons_force_ends_an_application_by_force() {
    // Every way a session ends, each with an application that is still
    // there when it does.
    let ways: [(&str, Ending); 6] = [
        ("its last window closed", |host, _| {
            host.runner.say(StreamEvent::Said(RunnerEvent::Empty))
        }),
        ("no window came", |host, _| {
            host.runner
                .say(StreamEvent::Said(RunnerEvent::NoWindowYet { after_s: 60 }))
        }),
        ("the stream died", |host, _| {
            host.runner.say(StreamEvent::Exited("signal 9".into()))
        }),
        ("the runner failed", |host, _| {
            host.runner.say(StreamEvent::Said(RunnerEvent::Error {
                message: "no display".into(),
            }))
        }),
        ("asked to close, then empty", |host, id| {
            session(host.stop(id, false));
            host.journal.wait_for("runner close", 1);
            host.runner.say(StreamEvent::Said(RunnerEvent::Empty))
        }),
        ("left, then empty", |host, id| {
            host.detach(id, 1);
            host.runner.say(StreamEvent::Said(RunnerEvent::Empty))
        }),
    ];
    for (way, end) in ways {
        for deaf in [false, true] {
            let host = host();
            host.launcher.deaf_to_asking(deaf);
            let id = host.running().id;
            end(&host, &id);
            let state = if deaf {
                SessionState::LeftOver
            } else {
                SessionState::Ended
            };
            host.follow_until(&id, state);
            assert_eq!(
                host.journal.count("app ended by force"),
                0,
                "{way}, deaf {deaf}"
            );
            assert_eq!(
                host.journal.count("app asked to end"),
                1,
                "{way}, deaf {deaf}"
            );
        }
    }
    // The person's word is the one way.
    let host = host();
    host.launcher.deaf_to_asking(true);
    let id = host.running().id;
    session(host.stop(&id, true));
    host.follow_until(&id, SessionState::Ended);
    assert_eq!(host.journal.count("app ended by force"), 1);
    assert_eq!(host.journal.count("app asked to end"), 0);
}

#[test]
fn an_application_that_stays_when_asked_is_left_over_for_a_person_to_see() {
    let host = host();
    host.launcher.deaf_to_asking(true);
    let id = host.running().id;
    host.runner.say(StreamEvent::Said(RunnerEvent::Empty));
    let left = host.follow_until(&id, SessionState::LeftOver);
    assert_eq!(left.left_over, Some(4242));
    let end = left.end.unwrap();
    assert_eq!(end.reason, EndReason::AppExited);
    assert!(
        end.message.contains("process group 4242") && end.message.contains("--force"),
        "{}",
        end.message
    );
    // The runner is gone; the session is listed, for the device and for the
    // person at the host, and cannot be shown any more.
    host.journal.wait_for("runner stop", 1);
    assert_eq!(host.list(&host.laptop)[0].state, SessionState::LeftOver);
    assert_eq!(host.sessions.host_list()[0].info.left_over, Some(4242));
    let (code, _) = refusal(host.attach(&id));
    assert_eq!(code, ErrorCode::Unavailable);
    // Asking again asks again; nothing more.
    assert_eq!(session(host.stop(&id, false)).state, SessionState::LeftOver);
    host.journal.wait_for("app asked to end", 2);
    assert_eq!(host.look(&id).state, SessionState::LeftOver);
    assert_eq!(host.journal.count("app ended by force"), 0);
    // A person's force ends it.
    host.sessions.host_stop(&id, true).unwrap();
    let over = host.follow_until(&id, SessionState::Ended);
    assert_eq!(over.left_over, None);
    let end = over.end.unwrap();
    assert_eq!(end.reason, EndReason::Stopped);
    assert!(
        end.message.ends_with("a person ended them by force"),
        "{}",
        end.message
    );
    assert_eq!(host.journal.count("app ended by force"), 1);
    assert!(host.list(&host.laptop).is_empty());
}

#[test]
fn what_was_left_over_ends_the_session_when_it_goes() {
    let host = host();
    host.launcher.deaf_to_asking(true);
    let id = host.running().id;
    host.runner.say(StreamEvent::Said(RunnerEvent::Empty));
    host.follow_until(&id, SessionState::LeftOver);
    // It listens this time.
    host.launcher.deaf_to_asking(false);
    session(host.stop(&id, false));
    let end = host.follow_until(&id, SessionState::Ended).end.unwrap();
    assert_eq!(end.reason, EndReason::AppExited);
    assert!(
        end.message.ends_with("they ended later (signal 15)"),
        "{}",
        end.message
    );
    assert_eq!(host.journal.count("app ended by force"), 0);
}

#[test]
fn an_application_is_opening_until_its_first_window() {
    let host = host();
    let id = host.running().id;
    assert!(!host.look(&id).opening);
    host.runner.say(StreamEvent::Said(RunnerEvent::Opening));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !host.look(&id).opening {
        assert!(Instant::now() < deadline, "not opening");
        std::thread::sleep(Duration::from_millis(10));
    }
    // No window yet is no change; the first window is.
    host.runner
        .say(StreamEvent::Said(RunnerEvent::Windows { count: 0 }));
    host.runner
        .say(StreamEvent::Said(RunnerEvent::Windows { count: 1 }));
    while host.look(&id).opening {
        assert!(Instant::now() < deadline, "still opening");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(host.look(&id).state, SessionState::Running);
    // A session that showed no window for long is ended as after `empty`.
    host.runner
        .say(StreamEvent::Said(RunnerEvent::NoWindowYet { after_s: 60 }));
    let end = host.follow_until(&id, SessionState::Ended).end.unwrap();
    assert_eq!(end.reason, EndReason::AppExited);
    assert!(
        end.message
            .starts_with("the application showed no window within 60 seconds"),
        "{}",
        end.message
    );
}
