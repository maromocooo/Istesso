//! The input's connection, inside the application.
//!
//! The window's thread hands over what the person did ([`Input::send`]) and
//! goes on at once; it never touches the connection, so a write that has to
//! wait never holds up the window. A thread of this module takes the
//! events, sends them, and looks after the conversation as the relay did
//! (`istesso_agent::relay::InputLine`): it says "still here", notices
//! silence, opens the conversation again after a cut, and does not send
//! later what was typed while there was none.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use istesso_agent::passing::Mark;
use istesso_agent::relay::{
    after_failure, Again, Connect, Happened, InputLine, LISTEN_EVERY, MODIFIERS, RETRY,
    RETRY_SLOWLY, SOON_FOR,
};
use istesso_agent::{ConnectFailure, PassLog};
use istesso_proto::InputEvent;

/// How long closing waits for the thread. It looks up at once when it is
/// told to stop, unless it is in the middle of an attempt to reach the host
/// (seconds, when the host is not there): closing does not wait for that.
const CLOSING_WAITS: Duration = Duration::from_millis(200);
/// What connecting and the handshake may take the first time.
const FIRST_WITHIN: Duration = Duration::from_secs(3);

/// Something that happened to the conversation, for the application's log
/// and its window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Told {
    /// The first conversation is open.
    Open,
    /// There is no conversation yet, and the host is asked again: it could
    /// not be reached, or said that nothing on it takes input at the moment
    /// (the application was opened before the host's side was up). Told
    /// once, with the reason of the first attempt that failed.
    NotYet { why: String },
    /// It was cut; it is being opened again.
    Cut,
    /// It is back, this long after it was cut.
    Back { after: Duration },
    /// The host does not take input from here (any more), and said why: the
    /// refusal is about rights (not paired, not allowed to control) or about
    /// a request it does not know. Nothing is tried after this.
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
    /// Whether the thread is scheduled as work a person is waiting for:
    /// `None` when that was not asked for, otherwise whether it took effect.
    pub interactive: Option<bool>,
}

struct Batch {
    events: Vec<InputEvent>,
    /// When the events were in hand, on the thread that handed them over.
    mark: Mark,
}

/// What reaches the thread.
enum Handed {
    Events(Batch),
    /// Look up now: it is time to stop.
    Stop,
}

struct Shared {
    state: Mutex<State>,
    stop: AtomicBool,
    /// Whether the caller's function may still be called. Held while it is
    /// called, so that closing returns only between two calls.
    may_tell: Mutex<bool>,
}

/// Makes the function that opens a conversation with the host. Called for
/// the first conversation, and again for the line that keeps it up.
pub type MakeConnect = Box<dyn Fn() -> Connect + Send>;

/// Is told what happens to the conversation, and how often it has been cut
/// so far. Called on the input's thread.
pub type Tell = Box<dyn Fn(&Told, u64) + Send>;

/// The input's connection and the thread that owns it.
pub struct Input {
    events: mpsc::Sender<Handed>,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    /// Closed by the thread when it has ended.
    ended: mpsc::Receiver<()>,
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
        let (ending, ended) = mpsc::channel::<()>();
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            stop: AtomicBool::new(false),
            may_tell: Mutex::new(true),
        });
        let thread = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("istesso input".into())
                .spawn(move || {
                    if interactive {
                        let took = ask_to_be_interactive();
                        shared
                            .state
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .interactive = Some(took);
                    }
                    let tell = |what: &Told, cuts: u64| {
                        let may = shared.may_tell.lock().unwrap_or_else(|e| e.into_inner());
                        if *may {
                            tell(what, cuts);
                        }
                    };
                    run(taken, make_connect, &shared, passing.as_ref(), &tell);
                    // The conversation is closed by now: whoever waits may go on.
                    drop(ending);
                })
                .ok()
        };
        Input {
            events,
            shared,
            thread,
            ended,
        }
    }

    /// Hands over what the person did. Never waits. Without a conversation
    /// the events are gone: they are not sent later.
    pub fn send(&self, events: Vec<InputEvent>) {
        if events.is_empty() {
            return;
        }
        let _ = self.events.send(Handed::Events(Batch {
            events,
            mark: PassLog::mark(),
        }));
    }

    /// For the measurement's way in: hands over events as [`Input::send`]
    /// does, with the time they were in hand as the caller took it.
    #[cfg(any(test, feature = "measure-keys"))]
    pub fn hand(&self) -> crate::measure::Hand {
        let events = self.events.clone();
        Box::new(move |batch, mark| {
            if !batch.is_empty() {
                let _ = events.send(Handed::Events(Batch {
                    events: batch,
                    mark,
                }));
            }
        })
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
        let _ = self.events.send(Handed::Stop);
        // From here on the caller's function is not called any more (a call that is
        // being made is waited for; it is short).
        *self
            .shared
            .may_tell
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = false;
        if let Some(thread) = self.thread.take() {
            // It looks up at once and closes the conversation, and the host lets go of
            // what it had down. Only when it is in the middle of an attempt to reach a
            // host that does not answer, that is not waited for: there is no
            // conversation then, and the thread ends by itself when the attempt does.
            match self.ended.recv_timeout(CLOSING_WAITS) {
                Err(RecvTimeoutError::Timeout) => drop(thread),
                _ => {
                    let _ = thread.join();
                }
            }
        }
    }
}

/// Adds an event to those that leave together. Of pointer positions in a
/// row only the last one matters.
pub(crate) fn take(events: &mut Vec<InputEvent>, event: InputEvent) {
    match (events.last_mut(), &event) {
        (Some(last @ InputEvent::PointerAt { .. }), InputEvent::PointerAt { .. }) => *last = event,
        _ => events.push(event),
    }
}

/// Keeps what is held and where the pointer is, as far as the events say,
/// while there is no line to keep it yet.
fn keep(events: &[InputEvent], held: &mut BTreeSet<u32>, pointer: &mut Option<(i32, i32)>) {
    for event in events {
        match *event {
            InputEvent::Key { code, pressed } if MODIFIERS.contains(&code) => {
                if pressed {
                    held.insert(code);
                } else {
                    held.remove(&code);
                }
            }
            InputEvent::PointerAt { x, y } => *pointer = Some((x, y)),
            _ => {}
        }
    }
}

fn run(
    taken: mpsc::Receiver<Handed>,
    make_connect: MakeConnect,
    shared: &Arc<Shared>,
    passing: Option<&PassLog>,
    tell: &dyn Fn(&Told, u64),
) {
    let set = |change: &dyn Fn(&mut State)| {
        change(&mut shared.state.lock().unwrap_or_else(|e| e.into_inner()));
    };
    // A thread that has been told to stop does not ask the host for anything any more.
    // Closing does not wait for an attempt to reach the host; when such an attempt gets
    // through afterwards, the conversation is dropped as it is, without the request to
    // send input: that request would take this device's input over once more, for nobody.
    let make_connect = || -> Connect {
        let mut connect = make_connect();
        let shared = shared.clone();
        Box::new(move |within| {
            let client = connect(within)?;
            if shared.stop.load(Ordering::SeqCst) {
                return Err(ConnectFailure::Here("closed meanwhile".into()));
            }
            Ok(client)
        })
    };
    let mut line: Option<InputLine> = None;
    let mut first = make_connect();
    let mut due = Instant::now();
    let mut over = false;
    // Since when the first conversation has been asked for in vain.
    let mut in_vain_since: Option<Instant> = None;
    // Before the first conversation: the modifiers that are held and where the
    // pointer is. What is handed over then is gone, but when the conversation opens
    // the host is brought in line with these, as it is after a cut.
    let (mut held, mut pointer) = (BTreeSet::new(), None);
    // What goes out first on a conversation that has just been opened.
    let mut in_line: Vec<InputEvent> = Vec::new();
    while !shared.stop.load(Ordering::SeqCst) {
        // Waits for what is handed over, and no longer than until the conversation
        // wants looking after: every tenth of a second while it is open, and when the
        // next attempt to open one is due while it is not. Once it is over there is
        // nothing to look after: what is handed over is taken and dropped.
        let now = Instant::now();
        let wait = if over {
            Duration::from_secs(1)
        } else if !in_line.is_empty() {
            Duration::ZERO
        } else {
            match &line {
                Some(line) => line.due().map_or(LISTEN_EVERY, |due| {
                    due.saturating_duration_since(now).min(LISTEN_EVERY)
                }),
                None => due.saturating_duration_since(now).min(LISTEN_EVERY),
            }
        };
        let mut events = std::mem::take(&mut in_line);
        let mut mark = None;
        let mut hand_in = |handed: Handed, events: &mut Vec<InputEvent>| match handed {
            Handed::Events(batch) => {
                mark.get_or_insert(batch.mark);
                batch.events.into_iter().for_each(|e| take(events, e));
            }
            Handed::Stop => {}
        };
        match taken.recv_timeout(wait) {
            Ok(handed) => {
                hand_in(handed, &mut events);
                // Whatever else has been handed over meanwhile leaves in the same message.
                while let Ok(more) = taken.try_recv() {
                    hand_in(more, &mut events);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if shared.stop.load(Ordering::SeqCst) {
            break;
        }
        if over {
            continue;
        }
        let now = Instant::now();
        let Some(open_line) = &mut line else {
            // No conversation yet: what was handed over is gone, and the first one is
            // tried when it is due.
            keep(&events, &mut held, &mut pointer);
            if now < due {
                continue;
            }
            let opened = first(FIRST_WITHIN)
                .map_err(|failure| (after_failure(&failure), failure.to_string()))
                .and_then(|client| InputLine::open(client, make_connect()));
            match opened {
                Ok(opened) => {
                    let answered = opened.is_answered();
                    line = Some(opened);
                    // What was handed over while the conversation was being opened was
                    // typed before there was one: it is gone like everything before it,
                    // and only kept track of.
                    while let Ok(handed) = taken.try_recv() {
                        if let Handed::Events(batch) = handed {
                            keep(&batch.events, &mut held, &mut pointer);
                        }
                    }
                    // The host has nothing down for this device. What is held here is
                    // in line again, as after a cut: the modifiers go down and the
                    // pointer is where it is; nothing else is pressed.
                    in_line = held
                        .iter()
                        .map(|&code| InputEvent::Key {
                            code,
                            pressed: true,
                        })
                        .collect();
                    if let Some((x, y)) = pointer {
                        in_line.push(InputEvent::PointerAt { x, y });
                    }
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
                // A state that passes: asked again, at the pace the line uses for
                // coming back after a cut.
                Err((again, why)) => {
                    let now = Instant::now();
                    let since = match in_vain_since {
                        Some(since) => since,
                        None => {
                            in_vain_since = Some(now);
                            tell(&Told::NotYet { why }, 0);
                            now
                        }
                    };
                    let fresh = now.saturating_duration_since(since) < SOON_FOR;
                    due = now
                        + match again {
                            Again::Soon if fresh => RETRY,
                            _ => RETRY_SLOWLY,
                        };
                }
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
/// Asks for the calling thread to be scheduled as work a person is waiting
/// for, and says whether that is so afterwards.
pub(crate) fn ask_to_be_interactive() -> bool {
    #[cfg(target_os = "macos")]
    // SAFETY: changes, and then reads, the scheduling class of the calling thread only.
    unsafe {
        let wanted = libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE;
        if libc::pthread_set_qos_class_self_np(wanted, 0) != 0 {
            return false;
        }
        let mut class = libc::qos_class_t::QOS_CLASS_UNSPECIFIED;
        let mut relative = 0;
        libc::pthread_get_qos_class_np(libc::pthread_self(), &mut class, &mut relative) == 0
            && class as u32 == wanted as u32
    }
    #[cfg(not(target_os = "macos"))]
    false
}
