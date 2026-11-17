//! The input's connection, inside the application.
//!
//! The window's thread hands over what the person did ([`Input::send`]) and
//! goes on at once; it never touches the connection, so a write that has to
//! wait never holds up the window. A thread of this module takes the
//! events, sends them, and looks after the conversation as the relay did
//! (`istesso_agent::relay::InputLine`): it says "still here", notices
//! silence, opens the conversation again after a cut, and does not send
//! later what was typed while there was none.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use istesso_agent::passing::Mark;
use istesso_agent::relay::{after_failure, Again, Connect, Happened, InputLine, LISTEN_EVERY};
use istesso_agent::PassLog;
use istesso_proto::InputEvent;

/// How long the thread waits for events while there is no conversation:
/// an attempt to open one may be due at once.
const WHILE_CLOSED: Duration = Duration::from_millis(5);
/// How long after a failed first attempt the next one is made.
const FIRST_RETRY: Duration = Duration::from_millis(500);
/// What connecting and the handshake may take the first time.
const FIRST_WITHIN: Duration = Duration::from_secs(3);

/// Something that happened to the conversation, for the application's log
/// and its window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Told {
    /// The first conversation is open.
    Open,
    /// It was cut; it is being opened again.
    Cut,
    /// It is back, this long after it was cut.
    Back { after: Duration },
    /// The host does not take input from here (any more), and said why.
    /// Nothing is tried after this.
    Over { why: String },
}

/// How the conversation stands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    /// How often the conversation was cut since it was first open.
    pub cuts: u64,
    /// There is a conversation at the moment.
    pub open: bool,
    /// The host answers "still here", so that silence is noticed.
    pub answered: bool,
    /// Why no input is taken any more, once that is so.
    pub over: Option<String>,
}

struct Batch {
    events: Vec<InputEvent>,
    /// When the events were in hand, on the thread that handed them over.
    mark: Mark,
}

struct Shared {
    state: Mutex<State>,
    stop: AtomicBool,
}

/// Makes the function that opens a conversation with the host. Called for
/// the first conversation, and again for the line that keeps it up.
pub type MakeConnect = Box<dyn Fn() -> Connect + Send>;

/// Is told what happens to the conversation, and how often it has been cut
/// so far. Called on the input's thread.
pub type Tell = Box<dyn Fn(&Told, u64) + Send>;

/// The input's connection and the thread that owns it.
pub struct Input {
    events: mpsc::Sender<Batch>,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Input {
    /// Starts the thread. It opens the conversation and keeps trying until
    /// it has one or is refused. `passing`: a log of when input was passed
    /// on, for measuring. `interactive`: the thread asks to be scheduled
    /// as work a person is waiting for. `tell` is called on the thread for
    /// everything that happens to the conversation.
    pub fn start(
        make_connect: MakeConnect,
        passing: Option<PassLog>,
        interactive: bool,
        tell: Tell,
    ) -> Input {
        let (events, taken) = mpsc::channel();
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            stop: AtomicBool::new(false),
        });
        let thread = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("istesso input".into())
                .spawn(move || {
                    if interactive {
                        ask_to_be_interactive();
                    }
                    run(taken, make_connect, &shared, passing.as_ref(), &*tell);
                })
                .ok()
        };
        Input {
            events,
            shared,
            thread,
        }
    }

    /// Hands over what the person did. Never waits. Without a conversation
    /// the events are gone: they are not sent later.
    pub fn send(&self, events: Vec<InputEvent>) {
        if events.is_empty() {
            return;
        }
        let _ = self.events.send(Batch {
            events,
            mark: PassLog::mark(),
        });
    }

    pub fn state(&self) -> State {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            // It looks up at least every tenth of a second, or as soon as an attempt to
            // connect is over. The conversation is closed with the thread, and the host
            // lets go of what it had down.
            let _ = thread.join();
        }
    }
}

/// Adds an event to those that leave together. Of pointer positions in a
/// row only the last one matters.
fn take(events: &mut Vec<InputEvent>, event: InputEvent) {
    match (events.last_mut(), &event) {
        (Some(last @ InputEvent::PointerAt { .. }), InputEvent::PointerAt { .. }) => *last = event,
        _ => events.push(event),
    }
}

fn run(
    taken: mpsc::Receiver<Batch>,
    make_connect: MakeConnect,
    shared: &Shared,
    passing: Option<&PassLog>,
    tell: &dyn Fn(&Told, u64),
) {
    let set = |change: &dyn Fn(&mut State)| {
        change(&mut shared.state.lock().unwrap_or_else(|e| e.into_inner()));
    };
    let mut line: Option<InputLine> = None;
    let mut first = make_connect();
    let mut due = Instant::now();
    let mut over = false;
    while !shared.stop.load(Ordering::SeqCst) {
        let open = line.as_ref().is_some_and(InputLine::is_open);
        // While a conversation is being opened, the attempt may be due at once. Once
        // it is over there is nothing to look after: what is handed over is taken and
        // dropped.
        let wait = if open || over {
            LISTEN_EVERY
        } else {
            WHILE_CLOSED
        };
        let mut events = Vec::new();
        let mut mark = None;
        match taken.recv_timeout(wait) {
            Ok(batch) => {
                mark = Some(batch.mark);
                batch.events.into_iter().for_each(|e| take(&mut events, e));
                // Whatever else has been handed over meanwhile leaves in the same message.
                while let Ok(more) = taken.try_recv() {
                    more.events.into_iter().for_each(|e| take(&mut events, e));
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if over {
            continue;
        }
        let now = Instant::now();
        let Some(open_line) = &mut line else {
            // No conversation yet: what was handed over is gone, and the first one is
            // tried when it is due.
            if now < due {
                continue;
            }
            let opened = first(FIRST_WITHIN)
                .map_err(|failure| (after_failure(&failure), failure.to_string()))
                .and_then(|client| {
                    InputLine::open(client, make_connect()).map_err(|why| (Again::Never, why))
                });
            match opened {
                Ok(opened) => {
                    let answered = opened.is_answered();
                    line = Some(opened);
                    set(&|state| {
                        state.open = true;
                        state.answered = answered;
                    });
                    if let Some(passing) = passing {
                        passing.reconnection("open", 0);
                    }
                    tell(&Told::Open, 0);
                }
                Err((Again::Never, why)) => {
                    over = true;
                    set(&|state| state.over = Some(why.clone()));
                    tell(&Told::Over { why }, 0);
                }
                Err(_) => due = Instant::now() + FIRST_RETRY,
            }
            continue;
        };
        let mut happened = None;
        if !events.is_empty() {
            let passed = open_line.pass(&events, now);
            happened = passed.happened;
            if let (Some(passing), Some(mark), true) = (passing, mark, passed.sent) {
                passing.passed(mark, events.len());
            }
        }
        if happened.is_none() {
            match open_line.turn(now) {
                Ok(turned) => happened = turned,
                Err(why) => {
                    over = true;
                    let cuts = open_line.cuts();
                    set(&|state| {
                        state.open = false;
                        state.over = Some(why.clone());
                    });
                    tell(&Told::Over { why }, cuts);
                    continue;
                }
            }
        }
        let (cuts, is_open, answered) = (
            open_line.cuts(),
            open_line.is_open(),
            open_line.is_answered(),
        );
        match happened {
            Some(Happened::Cut) => {
                set(&|state| {
                    state.cuts = cuts;
                    state.open = false;
                });
                if let Some(passing) = passing {
                    passing.reconnection("cut", cuts);
                }
                tell(&Told::Cut, cuts);
            }
            Some(Happened::Back { after }) => {
                set(&|state| {
                    state.open = is_open;
                    state.answered = answered;
                });
                if let Some(passing) = passing {
                    passing.reconnection("back", cuts);
                }
                tell(&Told::Back { after }, cuts);
            }
            None => {}
        }
    }
}

/// Asks the system to schedule this thread as work a person is waiting
/// for (macOS: the user-interactive class). Not a privileged operation;
/// elsewhere it does nothing.
fn ask_to_be_interactive() {
    #[cfg(target_os = "macos")]
    // SAFETY: changes the scheduling class of the calling thread only.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
}
