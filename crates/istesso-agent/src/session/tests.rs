//! The host's table of sessions, with a runner and a launcher that start
//! nothing.

use istesso_pair::{Hello, Identity};

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
        ["runner close", "runner close", "app stop", "runner stop"]
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
    assert_eq!(end.message, "the application closed its last window");
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
    assert_eq!(host.since_running(), ["app stop", "runner stop"]);
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
    // already gone there is nothing left to do.
    assert_eq!(host.since_running(), ["app stop", "runner stop"]);

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
    assert_eq!(
        end.message,
        "the host's side of the stream ended (signal 9)"
    );
    assert_eq!(host.since_running(), ["app stop", "runner stop"]);
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
    PeerStore::open(host.paths.peers())
        .unwrap()
        .add(record)
        .unwrap();
    let info = host.follow_until(&id, SessionState::Detached);
    assert_eq!(info.end, None);
    assert_eq!(host.since_running(), ["runner detach"]);
    // It still sees its session, and may ask it to close or end it ...
    assert_eq!(host.list(&host.laptop).len(), 1);
    // ... but it is not shown it again, and starts nothing new.
    let (code, _) = refusal(host.attach(&id));
    assert_eq!(code, ErrorCode::Forbidden);
    let (code, _) = refusal(host.start_as(&host.laptop, "editor.desktop"));
    assert_eq!(code, ErrorCode::Forbidden);

    // The pairing is removed: the device gets nothing at all, and the
    // application is still not ended.
    PeerStore::open(host.paths.peers())
        .unwrap()
        .remove(&host.laptop.id)
        .unwrap();
    for request in [
        Request::SessionList,
        Request::SessionWait {
            session: id.clone(),
            after: 0,
            timeout_ms: 0,
            attachment: 0,
        },
        Request::SessionStop {
            session: id.clone(),
            force: true,
        },
    ] {
        let (code, message) = refusal(host.ask(&host.laptop, request));
        assert_eq!(code, ErrorCode::Forbidden);
        assert!(message.contains("no longer paired"), "{message}");
    }
    assert_eq!(
        host.sessions.look(&id).unwrap().state,
        SessionState::Detached
    );
    assert_eq!(host.since_running(), ["runner detach"]);
}

#[test]
fn a_session_is_only_there_for_the_device_that_started_it() {
    let host = host();
    let id = host.running().id;

    let tablet = pair(&host.paths, "tablet", Permission::defaults());
    assert!(host.list(&tablet).is_empty());
    for request in [
        Request::SessionWait {
            session: id.clone(),
            after: 0,
            timeout_ms: 0,
            attachment: 1,
        },
        Request::SessionStop {
            session: id.clone(),
            force: true,
        },
        Request::SessionDetach {
            session: id.clone(),
            attachment: 1,
        },
        Request::SessionAttach {
            session: id.clone(),
            display: mode(),
            offer: serde_json::Value::Null,
        },
    ] {
        let (code, _) = refusal(host.ask(&tablet, request));
        assert_eq!(code, ErrorCode::NotFound);
    }
    // The session is untouched.
    assert_eq!(host.look(&id).state, SessionState::Running);
    assert_eq!(host.journal.all().len(), 2);

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
    // Another device has one as well; it counts, and it is not named.
    let tablet = pair(&host.paths, "tablet", Permission::defaults());
    let third = session(host.start_as(&tablet, "editor.desktop")).id;
    assert_ne!(first, third);

    // Full. Sessions that nobody is shown count like the others, and
    // nothing is ended to make room.
    let (code, message) = refusal(host.start_as(&host.laptop, "editor.desktop"));
    assert_eq!(code, ErrorCode::Unavailable);
    assert_eq!(
        message,
        "this host already runs 3 sessions, as many as it takes"
    );
    host.detach(&first, 1);
    host.detach(&second, 1);
    let (code, message) = refusal(host.start_as(&host.laptop, "editor.desktop"));
    assert_eq!(code, ErrorCode::Unavailable);
    assert_eq!(
        message,
        format!(
            "this host already runs 3 sessions, as many as it takes; of yours, nobody is shown: \
             Editor ({first}), Paint ({second})"
        )
    );
    assert_eq!(host.list(&host.laptop).len(), 2);
    assert_eq!(host.journal.count("app stop"), 0);

    // One that is over makes room.
    session(host.stop(&first, true));
    host.follow_until(&first, SessionState::Ended);
    host.start();
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
