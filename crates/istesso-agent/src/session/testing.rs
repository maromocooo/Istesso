//! Stand-ins for the tests of this crate: a runner, a launcher and a viewer
//! that start nothing, write down what they were asked to do, and do what
//! the test tells them.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use istesso_catalog::launch::LaunchPlan;
use istesso_proto::{
    AppEntry, DisplayMode, Launch, OsFamily, RunnerCommand, RunnerEvent, ViewerCommand, ViewerEvent,
};

use super::client::{RunningViewer, Viewer, ViewerSaid};
use super::{Launcher, Rules, Runner, RunningApp, RunningStream, StreamEvent};

/// What the stand-ins were asked to do, in the order it happened.
#[derive(Clone, Default)]
pub struct Journal(Arc<Mutex<Vec<String>>>);

impl Journal {
    pub fn note(&self, what: impl Into<String>) {
        self.0.lock().unwrap().push(what.into());
    }

    pub fn all(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }

    /// How often `what` has been written down.
    pub fn count(&self, what: &str) -> usize {
        self.all().iter().filter(|line| *line == what).count()
    }

    /// Waits until `what` has been written down `times` times.
    pub fn wait_for(&self, what: &str, times: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.count(what) < times {
            assert!(
                Instant::now() < deadline,
                "no '{what}' ({times}x) in {:?}",
                self.all()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// The host's rules with waits short enough for tests. A client is given
/// plenty of time, so that only the tests about lost clients lose one.
pub fn quick_rules() -> Rules {
    Rules {
        client_lost_after: Duration::from_secs(30),
        runner_ready_within: Duration::from_secs(5),
        attach_within: Duration::from_secs(5),
        app_stop_grace: Duration::from_millis(50),
        runner_stop_grace: Duration::from_millis(50),
        remember_ended_for: Duration::from_secs(60),
        wait_at_most: Duration::from_millis(500),
        max_sessions: 8,
        tick: Duration::from_millis(10),
        recheck_rights_every: Duration::from_millis(50),
    }
}

/// An entry of a Linux host's catalogue.
pub fn app(id: &str, name: &str) -> AppEntry {
    AppEntry {
        id: id.to_string(),
        name: name.to_string(),
        platform: OsFamily::Linux,
        version: None,
        launch: Launch::DesktopEntry {
            desktop_id: id.to_string(),
            exec: name.to_lowercase(),
        },
        icon: None,
        categories: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

/// How a stand-in stream answers an `attach`.
#[derive(Clone)]
pub enum OnAttach {
    /// Sends the picture to the new client, and says so.
    Attach,
    /// Cannot, and says why.
    Refuse(String),
    /// Says nothing.
    Ignore,
}

/// A runner whose streams say they are ready at once, and take every
/// client, unless told otherwise.
pub struct FakeRunner {
    journal: Journal,
    refusal: Mutex<Option<String>>,
    /// What a stream says as soon as it is started.
    opening: Mutex<Vec<StreamEvent>>,
    shared: Arc<Shared>,
}

/// What a stream says next.
type Said = Arc<Mutex<VecDeque<StreamEvent>>>;

/// What a runner and its streams have in common.
struct Shared {
    /// One list for each stream, in the order they were started.
    streams: Mutex<Vec<Said>>,
    /// Every line the runner and its streams were given.
    told: Mutex<Vec<RunnerCommand>>,
    on_attach: Mutex<OnAttach>,
    attachments: Mutex<u64>,
}

impl FakeRunner {
    pub fn new(journal: &Journal) -> Self {
        let ready = StreamEvent::Said(RunnerEvent::Ready {
            env: BTreeMap::from([("WAYLAND_DISPLAY".to_string(), "wayland-fake".to_string())]),
            display: Self::DISPLAY,
            answer: serde_json::json!({"fake": "answer"}),
        });
        FakeRunner {
            journal: journal.clone(),
            refusal: Mutex::new(None),
            opening: Mutex::new(vec![ready]),
            shared: Arc::new(Shared {
                streams: Mutex::default(),
                told: Mutex::default(),
                on_attach: Mutex::new(OnAttach::Attach),
                attachments: Mutex::new(1),
            }),
        }
    }

    /// Not what any test asks for, so that it shows whose word is passed on.
    const DISPLAY: DisplayMode = DisplayMode {
        width: 1272,
        height: 716,
        fps: 59,
    };

    /// What its streams say they produce for the first client.
    pub fn display(&self) -> DisplayMode {
        Self::DISPLAY
    }

    /// Every line it and its streams were given, the `start` lines
    /// included.
    pub fn told(&self) -> Vec<RunnerCommand> {
        self.shared.told.lock().unwrap().clone()
    }

    /// From now on it cannot start a stream.
    pub fn refuse(&self, why: &str) {
        *self.refusal.lock().unwrap() = Some(why.to_string());
    }

    /// What its streams say after they start, in place of "ready".
    pub fn instead_of_ready(&self, events: Vec<StreamEvent>) {
        *self.opening.lock().unwrap() = events;
    }

    /// Something the stream that was started last does.
    pub fn say(&self, event: StreamEvent) {
        let streams = self.shared.streams.lock().unwrap();
        let said = streams.last().expect("a stream was started");
        said.lock().unwrap().push_back(event);
    }

    /// How its streams answer an `attach` from now on.
    pub fn on_attach(&self, answer: OnAttach) {
        *self.shared.on_attach.lock().unwrap() = answer;
    }
}

impl Runner for FakeRunner {
    fn start(&self, start: &RunnerCommand) -> Result<Box<dyn RunningStream>, String> {
        if let Some(why) = self.refusal.lock().unwrap().clone() {
            return Err(why);
        }
        self.journal.note("runner start");
        self.shared.told.lock().unwrap().push(start.clone());
        let said: Said = Arc::new(Mutex::new(
            self.opening.lock().unwrap().iter().cloned().collect(),
        ));
        self.shared.streams.lock().unwrap().push(said.clone());
        Ok(Box::new(FakeStream {
            journal: self.journal.clone(),
            shared: self.shared.clone(),
            said,
        }))
    }
}

struct FakeStream {
    journal: Journal,
    shared: Arc<Shared>,
    said: Said,
}

impl RunningStream for FakeStream {
    fn poll(&mut self) -> Option<StreamEvent> {
        self.said.lock().unwrap().pop_front()
    }

    fn tell(&mut self, command: &RunnerCommand) -> Result<(), String> {
        self.shared.told.lock().unwrap().push(command.clone());
        match command {
            RunnerCommand::Attach { display, .. } => {
                self.journal.note("runner attach");
                let event = match self.shared.on_attach.lock().unwrap().clone() {
                    OnAttach::Attach => {
                        let mut attachments = self.shared.attachments.lock().unwrap();
                        *attachments += 1;
                        // It makes what this client asked for.
                        Some(RunnerEvent::Attached {
                            display: *display,
                            answer: serde_json::json!({"fake": "answer", "attach": *attachments}),
                        })
                    }
                    OnAttach::Refuse(message) => Some(RunnerEvent::Detached { message }),
                    OnAttach::Ignore => None,
                };
                if let Some(event) = event {
                    self.said
                        .lock()
                        .unwrap()
                        .push_back(StreamEvent::Said(event));
                }
            }
            RunnerCommand::Application { .. } => {}
            RunnerCommand::Detach => self.journal.note("runner detach"),
            RunnerCommand::Close => self.journal.note("runner close"),
            RunnerCommand::Start { .. } => self.journal.note("runner start again"),
        }
        Ok(())
    }

    fn stop(&mut self, _grace: Duration) {
        self.journal.note("runner stop");
    }
}

// ---------------------------------------------------------------------------
// Launcher
// ---------------------------------------------------------------------------

/// A launcher whose applications run until the test ends them.
pub struct FakeLauncher {
    journal: Journal,
    plan_refusal: Mutex<Option<String>>,
    launch_failure: Mutex<Option<String>>,
    /// How the applications end. One list for all of them.
    ends: Arc<Mutex<VecDeque<String>>>,
}

impl FakeLauncher {
    pub fn new(journal: &Journal) -> Self {
        FakeLauncher {
            journal: journal.clone(),
            plan_refusal: Mutex::new(None),
            launch_failure: Mutex::new(None),
            ends: Arc::default(),
        }
    }

    /// From now on no application can be shown as a window.
    pub fn refuse_plan(&self, why: &str) {
        *self.plan_refusal.lock().unwrap() = Some(why.to_string());
    }

    pub fn allow_plan(&self) {
        *self.plan_refusal.lock().unwrap() = None;
    }

    /// From now on applications do not start.
    pub fn fail_launch(&self, why: &str) {
        *self.launch_failure.lock().unwrap() = Some(why.to_string());
    }

    /// A running application ends by itself.
    pub fn end_app(&self, how: &str) {
        self.ends.lock().unwrap().push_back(how.to_string());
    }
}

impl Launcher for FakeLauncher {
    fn plan(&self, app: &AppEntry) -> Result<LaunchPlan, String> {
        match self.plan_refusal.lock().unwrap().clone() {
            Some(why) => Err(why),
            None => Ok(LaunchPlan {
                program: app.name.to_lowercase(),
                args: Vec::new(),
                working_dir: None,
            }),
        }
    }

    fn launch(
        &self,
        plan: &LaunchPlan,
        env: &BTreeMap<String, String>,
    ) -> Result<Box<dyn RunningApp>, String> {
        if let Some(why) = self.launch_failure.lock().unwrap().clone() {
            return Err(why);
        }
        self.journal.note(format!(
            "app launch {} WAYLAND_DISPLAY={}",
            plan.program,
            env.get("WAYLAND_DISPLAY").map_or("-", String::as_str)
        ));
        Ok(Box::new(FakeApp {
            journal: self.journal.clone(),
            ends: self.ends.clone(),
        }))
    }
}

struct FakeApp {
    journal: Journal,
    ends: Arc<Mutex<VecDeque<String>>>,
}

impl RunningApp for FakeApp {
    fn poll(&mut self) -> Option<String> {
        self.ends.lock().unwrap().pop_front()
    }

    fn stop(&mut self, _grace: Duration) {
        self.journal.note("app stop");
    }
}

// ---------------------------------------------------------------------------
// Viewer
// ---------------------------------------------------------------------------

/// A viewer that offers at once and stays open until the test closes it.
pub struct FakeViewer {
    journal: Journal,
    said: Arc<Mutex<VecDeque<ViewerSaid>>>,
    told: Arc<Mutex<Vec<ViewerCommand>>>,
}

impl FakeViewer {
    pub fn new(journal: &Journal) -> Self {
        FakeViewer {
            journal: journal.clone(),
            said: Arc::default(),
            told: Arc::default(),
        }
    }

    /// What it asks the host for.
    pub const DISPLAY: DisplayMode = DisplayMode {
        width: 1280,
        height: 720,
        fps: 60,
    };

    /// Everything it was told.
    pub fn told(&self) -> Vec<ViewerCommand> {
        self.told.lock().unwrap().clone()
    }

    /// The person clicks the window's close button.
    pub fn ask_to_close(&self) {
        self.said
            .lock()
            .unwrap()
            .push_back(ViewerSaid::Said(ViewerEvent::Close));
    }

    /// The person quits the viewer.
    pub fn quit(&self) {
        self.said.lock().unwrap().push_back(ViewerSaid::Exited {
            ok: true,
            how: "exit status 0".to_string(),
        });
    }

    /// The viewer breaks.
    pub fn crash(&self) {
        self.said.lock().unwrap().push_back(ViewerSaid::Exited {
            ok: false,
            how: "signal 11".to_string(),
        });
    }
}

impl Viewer for FakeViewer {
    fn start(&self) -> Result<Box<dyn RunningViewer>, String> {
        self.journal.note("viewer start");
        Ok(Box::new(FakeWindow {
            journal: self.journal.clone(),
            said: self.said.clone(),
            told: self.told.clone(),
        }))
    }
}

struct FakeWindow {
    journal: Journal,
    said: Arc<Mutex<VecDeque<ViewerSaid>>>,
    told: Arc<Mutex<Vec<ViewerCommand>>>,
}

impl RunningViewer for FakeWindow {
    fn tell(&mut self, command: &ViewerCommand) -> Result<(), String> {
        self.told.lock().unwrap().push(command.clone());
        match command {
            ViewerCommand::Prepare { .. } => {
                self.said
                    .lock()
                    .unwrap()
                    .push_back(ViewerSaid::Said(ViewerEvent::Offer {
                        display: FakeViewer::DISPLAY,
                        offer: serde_json::json!({"fake": "offer"}),
                    }));
            }
            ViewerCommand::Show { .. } => self.journal.note("viewer show"),
        }
        Ok(())
    }

    fn poll(&mut self) -> Option<ViewerSaid> {
        self.said.lock().unwrap().pop_front()
    }

    fn stop(&mut self, _grace: Duration) {
        self.journal.note("viewer stop");
    }
}
