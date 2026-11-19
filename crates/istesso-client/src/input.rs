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
use istesso_media::Failure;
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
    /// with the reason of the attempt that failed, when the first attempt
    /// fails and again whenever a later one fails in another way.
    /// `failure` is that way in a word that names no address (the reason's
    /// sentence may name one, and is not for a log).
    NotYet { why: String, failure: Failure },
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
    /// While there is no conversation: in what way the latest attempt to
    /// reach the host failed.
    pub failure: Option<Failure>,
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

/// Linux key codes of the keys committed text stands in for (input-event-codes.h).
const KEY_TAB: u32 = 15;
const KEY_ENTER: u32 = 28;

/// Committed text from the device's own input method as the events that carry it (trial only,
/// docs/adr/0007-control-channel.md, "確定した文字"): pieces of at most
/// [`istesso_proto::TEXT_MOST_CHARS`] characters and [`istesso_proto::TEXT_MOST_BYTES`] bytes, cut
/// between characters; a newline (or carriage return) as the Enter key and a tab as the Tab key,
/// pressed and let go; other control characters left out. Never logged.
pub fn text_events(text: &str) -> Vec<InputEvent> {
    let mut events = Vec::new();
    let mut piece = String::new();
    let mut chars = 0;
    let flush = |piece: &mut String, chars: &mut usize, events: &mut Vec<InputEvent>| {
        if !piece.is_empty() {
            events.push(InputEvent::Text {
                text: std::mem::take(piece),
            });
        }
        *chars = 0;
    };
    let mut last_was_cr = false;
    for c in text.chars() {
        let key = match c {
            '\n' if last_was_cr => {
                last_was_cr = false;
                continue; // "\r\n" is one Enter
            }
            '\r' | '\n' => Some(KEY_ENTER),
            '\t' => Some(KEY_TAB),
            _ => None,
        };
        last_was_cr = c == '\r';
        if let Some(code) = key {
            flush(&mut piece, &mut chars, &mut events);
            events.push(InputEvent::Key {
                code,
                pressed: true,
            });
            events.push(InputEvent::Key {
                code,
                pressed: false,
            });
            continue;
        }
        if c.is_control() {
            continue;
        }
        if chars + 1 > istesso_proto::TEXT_MOST_CHARS
            || piece.len() + c.len_utf8() > istesso_proto::TEXT_MOST_BYTES
        {
            flush(&mut piece, &mut chars, &mut events);
        }
        piece.push(c);
        chars += 1;
    }
    flush(&mut piece, &mut chars, &mut events);
    events
}

/// Adds an event to those that leave together. Of pointer positions in a
/// row only the last one matters.
pub(crate) fn take(events: &mut Vec<InputEvent>, event: InputEvent) {
    match (events.last_mut(), &event) {
        (Some(last @ InputEvent::PointerAt { .. }), InputEvent::PointerAt { .. }) => *last = event,
        _ => events.push(event),
    }
}

/// In what way an attempt to reach the host failed, in a word without an
/// address.
fn way_of(failure: &ConnectFailure) -> Failure {
    match failure {
        ConnectFailure::Link(err) => Failure::of_link(err),
        ConnectFailure::Here(_) => Failure::Other,
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
            let client = connect(within).inspect_err(|failure| {
                // Kept for whoever asks how the input stands, also while the line
                // itself is trying again after a cut.
                shared
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .failure = Some(way_of(failure));
            })?;
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
    // Since when the first conversation has been asked for in vain, and in what way the
    // attempt failed that was last told of.
    let mut in_vain_since: Option<Instant> = None;
    let mut told_way: Option<Failure> = None;
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
                .map_err(|failure| {
                    (
                        after_failure(&failure),
                        failure.to_string(),
                        way_of(&failure),
                    )
                })
                .and_then(|client| {
                    // The host was reached and greeted. Either it answered that nothing
                    // takes input there at the moment, or the question got no answer.
                    InputLine::open(client, make_connect()).map_err(|(again, why)| {
                        let way = match again {
                            Again::Slowly => Failure::NotReady,
                            _ => Failure::NoAnswer,
                        };
                        (again, why, way)
                    })
                });
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
                        state.failure = None;
                    });
                    if let Some(passing) = passing {
                        passing.reconnection("open", 0);
                    }
                    tell(&Told::Open, 0);
                }
                Err((Again::Never, why, _)) => {
                    over = true;
                    set(&|state| state.over = Some(why.clone()));
                    tell(&Told::Over { why }, 0);
                }
                // A state that passes: asked again, at the pace the line uses for
                // coming back after a cut.
                Err((again, why, way)) => {
                    let now = Instant::now();
                    let since = *in_vain_since.get_or_insert(now);
                    set(&|state| state.failure = Some(way));
                    if told_way != Some(way) {
                        told_way = Some(way);
                        tell(&Told::NotYet { why, failure: way }, 0);
                    }
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
                    state.failure = None;
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

#[cfg(test)]
mod text_tests {
    use super::*;

    fn text(t: &str) -> InputEvent {
        InputEvent::Text { text: t.into() }
    }

    fn key(code: u32, pressed: bool) -> InputEvent {
        InputEvent::Key { code, pressed }
    }

    #[test]
    fn committed_text_is_cut_into_pieces_within_the_limits() {
        assert_eq!(text_events("日本語"), [text("日本語")]);
        let long = "あ".repeat(300);
        let pieces = text_events(&long);
        assert_eq!(pieces, [text(&"あ".repeat(256)), text(&"あ".repeat(44))]);
        // Four bytes a character: the byte limit comes first.
        let wide = "😀".repeat(300);
        for piece in text_events(&wide) {
            let InputEvent::Text { text } = piece else {
                panic!()
            };
            assert!(istesso_proto::text_is_acceptable(&text));
            assert_eq!(text.len(), 1024.min(text.len()));
        }
        assert!(text_events("").is_empty());
    }

    #[test]
    fn newline_and_tab_go_as_keys_and_other_control_characters_not_at_all() {
        assert_eq!(
            text_events("a\tb\r\nc\u{7}"),
            [
                text("a"),
                key(KEY_TAB, true),
                key(KEY_TAB, false),
                text("b"),
                key(KEY_ENTER, true),
                key(KEY_ENTER, false),
                text("c")
            ]
        );
        assert_eq!(
            text_events("\n"),
            [key(KEY_ENTER, true), key(KEY_ENTER, false)]
        );
    }
}
