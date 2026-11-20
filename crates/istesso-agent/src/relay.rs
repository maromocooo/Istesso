//! The sending end of keyboard and pointer input: one conversation with the
//! device that shows an application, kept up for as long as input flows,
//! and made again when it is cut.
//!
//! A connection can end while both ends are alive: a network may stop
//! delivering on it without a word. The sender does not notice by sending,
//! because nothing comes back for what a person types. So it asks the other
//! device to answer the empty message that says "still here", says that
//! twice a second, and takes a second without an answer as a cut. Then it
//! opens a new conversation in the ordinary way; nothing of the channel's
//! procedure changes for it (ADR 0007).
//!
//! What was typed while there was no conversation is not sent later: a key
//! that arrives a second late is typed into a picture that has changed
//! since, and what was sent just before the cut may have arrived, so sending
//! it again could type it twice. When the conversation is back, the
//! modifiers that are held are pressed again, and nothing else. The daily
//! trial first releases any keys and buttons left down on the old channel.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use istesso_link::{Client, ClipboardAnswer, Error as LinkError, ErrorCode, InputAnswer};
use istesso_proto::InputEvent;

use crate::survey::ConnectFailure;

/// How often "still here" is said to a device that answers it.
pub const ALIVE_EVERY: Duration = Duration::from_millis(500);
/// How often it is said to a device that does not (an earlier version),
/// which waits five seconds for it.
pub const ALIVE_EVERY_UNANSWERED: Duration = Duration::from_secs(2);
/// How long a device that answers may say nothing: two answers are missing
/// then. The same second as for the picture and for sound.
pub const SILENT_AFTER: Duration = Duration::from_secs(1);
/// How long a "still here" may go unanswered. It is said half a second
/// after the last one, so an answer that is this late is a second after
/// the answer before it: [`SILENT_AFTER`], counted from this side's own
/// question, so that a sender that did not ask for a while does not take
/// its own pause for the other side's silence.
pub const UNANSWERED_FOR: Duration = Duration::from_millis(500);
/// How often what the device has answered is looked at.
pub const LISTEN_EVERY: Duration = Duration::from_millis(100);
/// How often "still here" is said, and the answers looked at, while a copy or a paste waits for
/// its answer (trial only): the device writes the answer when it next hears from this side.
pub const CLIPBOARD_EVERY: Duration = Duration::from_millis(50);
/// How long the answer to a copy or a paste is waited for.
pub const CLIPBOARD_WITHIN: Duration = Duration::from_secs(2);
/// How long after a failed attempt the next one is made while the cut is
/// fresh, and once the device has been away for [`SOON_FOR`].
pub const RETRY: Duration = Duration::from_millis(250);
pub const RETRY_SLOWLY: Duration = Duration::from_secs(1);
pub const SOON_FOR: Duration = Duration::from_secs(10);
/// What connecting, the handshake and the answer to the request may take
/// together when the conversation is made again, and what a write may take.
pub const AGAIN_WITHIN: Duration = Duration::from_secs(1);
/// What the answer to the first request may take. A host that is busy
/// answers late; that is not a host that refuses.
pub const FIRST_ANSWER_WITHIN: Duration = Duration::from_secs(3);

/// The keys that are pressed again when a conversation is back: Control,
/// Shift, Alt and Super, left and right (Linux key numbers).
const MODIFIERS: [u32; 8] = [29, 42, 54, 56, 97, 100, 125, 126];

/// What to do about a conversation that ended or could not be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Again {
    /// Open it again at once, then at short intervals: nobody said why it
    /// ended.
    Soon,
    /// Open it again at leisure: nothing takes input over there at the
    /// moment, which may pass.
    Slowly,
    /// Do not: the device refused, and would again.
    Never,
}

/// What a failure to connect asks for.
pub fn after_failure(failure: &ConnectFailure) -> Again {
    match failure {
        // The list of paired devices cannot be read, or has nobody of that name in
        // it any more: something a person has to look at.
        ConnectFailure::Here(_) => Again::Never,
        ConnectFailure::Link(
            LinkError::UnknownPeer
            | LinkError::NotTrusted
            | LinkError::WrongPeer { .. }
            | LinkError::Version { .. },
        ) => Again::Never,
        ConnectFailure::Link(_) => Again::Soon,
    }
}

/// What a refusal to take input asks for.
pub fn after_refusal(code: ErrorCode) -> Again {
    match code {
        // The program that shows the application is not there: it may be on its way.
        ErrorCode::Internal => Again::Slowly,
        ErrorCode::Forbidden | ErrorCode::Unsupported | ErrorCode::Other => Again::Never,
    }
}

/// Opens a conversation with the device, within this time.
pub type Connect = Box<dyn FnMut(Duration) -> Result<Client, ConnectFailure> + Send>;

/// Something that happened to the conversation, for whoever runs the relay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Happened {
    /// It was cut; it is being opened again.
    Cut,
    /// It is back, this long after it was cut.
    Back { after: Duration },
}

/// What became of events handed to [`InputLine::pass`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Passed {
    /// They were sent. Otherwise there is no conversation at the moment,
    /// and they are gone.
    pub sent: bool,
    pub happened: Option<Happened>,
}

/// Keys and buttons the daily relay may have left down. The old agent
/// normally releases them. A restarted agent has no record of them.
#[derive(Default)]
struct HeldInput {
    keys: BTreeSet<u32>,
    buttons: BTreeSet<u32>,
}

impl HeldInput {
    fn record(&mut self, events: &[InputEvent], sent: bool) {
        for event in events {
            let (held, code, pressed) = match *event {
                InputEvent::Key { code, pressed } => (&mut self.keys, code, pressed),
                InputEvent::Button { code, pressed } => (&mut self.buttons, code, pressed),
                _ => continue,
            };
            if pressed {
                held.insert(code);
            } else if sent {
                // A failed write may have lost this release. Keep it for the next channel.
                held.remove(&code);
            }
        }
    }

    fn releases(&self) -> Vec<InputEvent> {
        self.keys
            .iter()
            .map(|&code| InputEvent::Key {
                code,
                pressed: false,
            })
            .chain(self.buttons.iter().map(|&code| InputEvent::Button {
                code,
                pressed: false,
            }))
            .collect()
    }
}

/// The conversation input is sent on.
pub struct InputLine {
    connect: Connect,
    client: Option<Client>,
    /// The device answers "still here".
    alive: bool,
    /// The device takes committed text; without, none is sent to it.
    text: bool,
    /// The device takes composing text; without, none is sent to it.
    preedit: bool,
    /// Pieces of committed or composing text not sent because the device does not take them.
    text_dropped: u64,
    /// The level of scrolls the device takes (`istesso_proto::SCROLL_LEVEL`; 0: `dx`, `dy` only).
    scroll: u8,
    /// Said once in this conversation that the device does not take the kind of scrolls.
    scroll_told: bool,
    /// The device takes copy and paste (trial only); without, none is sent to it.
    clipboard: bool,
    /// Copies and pastes not sent because the device does not take them.
    clipboard_dropped: u64,
    /// The device takes the application's other windows (trial only, ADR 0027 proposal):
    /// window numbers, `close` and `windows`. Without, none of them is sent to it
    /// ([`istesso_proto::needs_windows`]).
    windows: bool,
    /// Events not sent because they were for the other windows and the device does not take
    /// them.
    windows_dropped: u64,
    /// Said once in this conversation that the device does not take them.
    windows_told: bool,
    /// The copies and pastes sent on this conversation that wait for their answer, and until
    /// when.
    awaiting: BTreeMap<u32, Instant>,
    /// Answers that came and are not handed on yet.
    answers: Vec<ClipboardAnswer>,
    /// When "still here" was last said.
    said: Instant,
    /// When it was first said without an answer having come since. Silence
    /// is counted from here, not from the last answer: what counts is a
    /// question that went unanswered, not how long this side did not ask.
    unanswered_since: Option<Instant>,
    listened: Instant,
    /// The modifiers that are down on this side, as far as the events say.
    modifiers: BTreeSet<u32>,
    daily_held: Option<HeldInput>,
    /// Where the pointer is, and in which window of the application, as far as the events say.
    pointer: Option<(i32, i32, Option<u32>)>,
    /// Since when there has been no conversation, and when the next attempt
    /// is due.
    without_since: Option<Instant>,
    due: Instant,
    made_at: Instant,
    /// How often the conversation was cut.
    cuts: u64,
}

impl InputLine {
    /// Asks on a conversation that has just been opened whether input is
    /// taken there. `connect` opens the next one when this one is cut.
    /// `Err` says why there is no line, and what to do about it: a device
    /// that refuses is not asked again ([`Again::Never`]); one that did not
    /// answer in time may be, with a new conversation.
    pub fn open(mut client: Client, connect: Connect) -> Result<Self, (Again, String)> {
        let taken = ask(&mut client, FIRST_ANSWER_WITHIN)?;
        let mut line = Self::waiting(connect);
        line.client = Some(client);
        line.alive = taken.alive;
        line.text = taken.text;
        line.preedit = taken.preedit;
        line.scroll = taken.scroll;
        line.clipboard = taken.clipboard;
        line.windows = taken.windows;
        Ok(line)
    }

    /// A daily relay waits for the first input socket as well as later ones.
    /// It releases old keys and buttons before restoring held modifiers.
    pub fn for_daily_window(connect: Connect) -> Self {
        let mut line = Self::waiting(connect);
        line.daily_held = Some(HeldInput::default());
        line
    }

    fn waiting(connect: Connect) -> Self {
        let now = Instant::now();
        InputLine {
            connect,
            client: None,
            alive: false,
            text: false,
            preedit: false,
            text_dropped: 0,
            scroll: 0,
            scroll_told: false,
            clipboard: false,
            clipboard_dropped: 0,
            windows: false,
            windows_dropped: 0,
            windows_told: false,
            awaiting: BTreeMap::new(),
            answers: Vec::new(),
            said: now,
            unanswered_since: None,
            listened: now,
            modifiers: BTreeSet::new(),
            daily_held: None,
            pointer: None,
            without_since: None,
            due: now,
            made_at: now,
            cuts: 0,
        }
    }

    /// How often the conversation was cut since this line was opened,
    /// whether or not it is back. A measurement that is to say something
    /// about an undisturbed line wants this to be 0.
    pub fn cuts(&self) -> u64 {
        self.cuts
    }

    /// Whether there is a conversation at the moment.
    pub fn is_open(&self) -> bool {
        self.client.is_some()
    }

    /// How long a disconnected relay can wait for input before its next attempt.
    pub fn retry_in(&self, now: Instant) -> Duration {
        self.due.saturating_duration_since(now).min(LISTEN_EVERY)
    }

    /// Whether the device takes committed text, as it said when the
    /// conversation was last opened.
    pub fn takes_text(&self) -> bool {
        self.text
    }

    /// The level of scrolls the device takes, as it said when the conversation was opened.
    pub fn scroll_level(&self) -> u8 {
        self.scroll
    }

    /// Whether the device takes composing text, as it said when the
    /// conversation was last opened.
    pub fn takes_preedit(&self) -> bool {
        self.preedit
    }

    /// How many pieces of committed or composing text were not sent because the device
    /// does not take them.
    pub fn text_dropped(&self) -> u64 {
        self.text_dropped
    }

    /// Whether the device takes copy and paste, as it said when the conversation was last
    /// opened (trial only).
    pub fn takes_clipboard(&self) -> bool {
        self.clipboard
    }

    /// How many copies and pastes were not sent because the device does not take them.
    pub fn clipboard_dropped(&self) -> u64 {
        self.clipboard_dropped
    }

    /// Whether the device takes the application's other windows (window numbers, `close` and
    /// `windows`), as it said when the conversation was last opened (trial only).
    pub fn takes_windows(&self) -> bool {
        self.windows
    }

    /// How many events for the other windows were not sent because the device does not take
    /// them.
    pub fn windows_dropped(&self) -> u64 {
        self.windows_dropped
    }

    /// The answers to copy and paste that came since this was last asked, in their order.
    pub fn take_clipboard_answers(&mut self) -> Vec<ClipboardAnswer> {
        std::mem::take(&mut self.answers)
    }

    /// How long to wait for something before looking after an open conversation again:
    /// shorter while a copy or a paste waits for its answer.
    pub fn listen_in(&self) -> Duration {
        if self.awaiting.is_empty() {
            LISTEN_EVERY
        } else {
            CLIPBOARD_EVERY
        }
    }

    /// Whether the device answers "still here", so that silence is noticed.
    pub fn is_answered(&self) -> bool {
        self.alive
    }

    fn observe_input(&mut self, events: &[InputEvent]) {
        for event in events {
            match *event {
                InputEvent::Key { code, pressed } if MODIFIERS.contains(&code) => {
                    if pressed {
                        self.modifiers.insert(code);
                    } else {
                        self.modifiers.remove(&code);
                    }
                }
                InputEvent::PointerAt { x, y, window } if self.daily_held.is_none() => {
                    self.pointer = Some((x, y, window));
                }
                _ => {}
            }
        }
    }

    /// Sends what a person did. Without a conversation it is not sent, now
    /// or later.
    pub fn pass(&mut self, events: &[InputEvent], now: Instant) -> Passed {
        // A device that did not say it takes committed text cannot read a message with
        // it, keys and all: none goes to it.
        let kept: Vec<InputEvent>;
        // The same for copy and paste. And for what is meant for the application's other
        // windows (trial only): a device that does not know window numbers would take a
        // pointer meant for another window for one on the first.
        let not_taken = |e: &InputEvent| match e {
            InputEvent::Text { .. } => !self.text,
            InputEvent::Preedit { .. } => !self.preedit,
            InputEvent::Copy { .. } | InputEvent::Paste { .. } => !self.clipboard,
            _ => !self.windows && istesso_proto::needs_windows(e),
        };
        let events = if events.iter().any(not_taken) {
            kept = events.iter().filter(|e| !not_taken(e)).cloned().collect();
            let mut windows_dropped = false;
            for event in events.iter().filter(|e| not_taken(e)) {
                match event {
                    InputEvent::Text { .. } | InputEvent::Preedit { .. } => self.text_dropped += 1,
                    InputEvent::Copy { .. } | InputEvent::Paste { .. } => {
                        self.clipboard_dropped += 1
                    }
                    _ => {
                        self.windows_dropped += 1;
                        windows_dropped = true;
                    }
                }
            }
            // Said once a conversation, in a plain line, as for the kind of scrolls.
            if windows_dropped && !self.windows_told && self.client.is_some() {
                self.windows_told = true;
                eprintln!(
                    "note: the host's agent does not take: the application's other windows (window numbers, close, windows); what is meant for them is not sent"
                );
            }
            &kept[..]
        } else {
            events
        };
        // A device that does not take the kind of scrolls uses their dx and dy only: one whose
        // dx and dy are both 0 (a stage of a gesture) would only be an empty scroll there. Said
        // once a conversation, in a plain line, instead of the scrolls being less without a word.
        let fewer: Vec<InputEvent>;
        let events = if self.scroll < istesso_proto::SCROLL_LEVEL
            && events.iter().any(istesso_proto::scroll_has_kind)
        {
            if !self.scroll_told && self.client.is_some() {
                self.scroll_told = true;
                eprintln!(
                    "note: the host's agent does not take: scroll kinds (wheel notches, finger pixels, stages); scrolls go as dx and dy only"
                );
            }
            fewer = events
                .iter()
                .filter(|e| !matches!(e, InputEvent::Scroll { dx: 0, dy: 0, .. }))
                .cloned()
                .collect();
            &fewer[..]
        } else {
            events
        };
        self.observe_input(events);
        let Some(client) = &mut self.client else {
            return Passed {
                sent: false,
                happened: None,
            };
        };
        // What a person types is not answered, so it says nothing about whether the
        // conversation is alive: "still here" goes on being said beside it.
        let sent = client.send_input(events).is_ok();
        if let Some(held) = &mut self.daily_held {
            held.record(events, sent);
        }
        if sent {
            for event in events {
                if let InputEvent::Copy { id, .. } | InputEvent::Paste { id, .. } = event {
                    self.awaiting.insert(*id, now + CLIPBOARD_WITHIN);
                    // Said again at once, so that the device has a turn to answer soon.
                    self.said = now.checked_sub(ALIVE_EVERY).unwrap_or(self.said);
                }
            }
        }
        Passed {
            sent,
            happened: (!sent).then(|| self.cut(now)),
        }
    }

    /// Looks after the conversation: says "still here", hears the answers,
    /// notices silence, and opens the conversation again when it was cut.
    /// To be called every [`LISTEN_EVERY`] or so. `Err` says why the device
    /// does not take input from here any more; nothing is tried after that.
    pub fn turn(&mut self, now: Instant) -> Result<Option<Happened>, String> {
        self.turn_with_pending(now, Vec::new)
    }

    /// On a daily reconnect, drain local input received during the handshake.
    /// Only the resulting modifier state is used on the new channel.
    pub fn turn_with_pending(
        &mut self,
        now: Instant,
        pending: impl FnOnce() -> Vec<InputEvent>,
    ) -> Result<Option<Happened>, String> {
        self.awaiting.retain(|_, until| *until > now);
        let waiting = !self.awaiting.is_empty();
        let Some(client) = &mut self.client else {
            return self.open_again(now, pending);
        };
        let silent = self
            .unanswered_since
            .is_some_and(|since| now.saturating_duration_since(since) >= UNANSWERED_FOR);
        let listen_every = if waiting {
            CLIPBOARD_EVERY
        } else {
            LISTEN_EVERY
        };
        if self.alive && (silent || now.saturating_duration_since(self.listened) >= listen_every) {
            // Before silence is believed, what has arrived is looked at: an answer
            // that came while nobody listened is an answer, and so is an answer to a copy.
            self.listened = now;
            match client.heard() {
                Ok(heard) => {
                    for answer in &heard.clipboard {
                        self.awaiting.remove(&answer.id);
                    }
                    self.answers.extend(heard.clipboard);
                    if heard.alive {
                        self.unanswered_since = None;
                    } else if silent {
                        return Ok(Some(self.cut(now)));
                    }
                }
                Err(_) => return Ok(Some(self.cut(now))),
            }
        }
        let every = if self.alive && !self.awaiting.is_empty() {
            CLIPBOARD_EVERY
        } else if self.alive {
            ALIVE_EVERY
        } else {
            ALIVE_EVERY_UNANSWERED
        };
        if now.saturating_duration_since(self.said) >= every {
            self.said = now;
            if client.send_input(&[]).is_err() {
                return Ok(Some(self.cut(now)));
            }
            if self.alive {
                self.unanswered_since.get_or_insert(now);
            }
        }
        Ok(None)
    }

    fn cut(&mut self, now: Instant) -> Happened {
        self.cuts += 1;
        self.client = None;
        // Answers to what was asked on that conversation do not come on the next one.
        self.awaiting.clear();
        self.without_since = Some(now);
        // At once, unless the conversation had only just been opened: a device that
        // takes one and lets go of it straight away is not asked faster than any other.
        let short_lived = now.saturating_duration_since(self.made_at) < RETRY;
        self.due = if short_lived { now + RETRY } else { now };
        Happened::Cut
    }

    fn wait(&mut self, what: Again, why: String, now: Instant) -> Result<Option<Happened>, String> {
        let since = *self.without_since.get_or_insert(now);
        match what {
            Again::Never => Err(why),
            Again::Slowly => {
                self.due = now + RETRY_SLOWLY;
                Ok(None)
            }
            Again::Soon => {
                let fresh = now.saturating_duration_since(since) < SOON_FOR;
                self.due = now + if fresh { RETRY } else { RETRY_SLOWLY };
                Ok(None)
            }
        }
    }

    fn reconnect_events(&self) -> Vec<InputEvent> {
        let mut events = self
            .daily_held
            .as_ref()
            .map_or_else(Vec::new, HeldInput::releases);
        events.extend(self.modifiers.iter().map(|&code| InputEvent::Key {
            code,
            pressed: true,
        }));
        if let Some((x, y, window)) = self.pointer {
            events.push(InputEvent::PointerAt { x, y, window });
        }
        events
    }

    fn open_again(
        &mut self,
        now: Instant,
        pending: impl FnOnce() -> Vec<InputEvent>,
    ) -> Result<Option<Happened>, String> {
        if now < self.due {
            return Ok(None);
        }
        let within = if self.cuts == 0 {
            FIRST_ANSWER_WITHIN
        } else {
            AGAIN_WITHIN
        };
        let mut client = match (self.connect)(within) {
            Ok(client) => client,
            Err(failure) => return self.wait(after_failure(&failure), failure.to_string(), now),
        };
        let taken = match ask(&mut client, within) {
            Ok(answer) => answer,
            Err((what, why)) => return self.wait(what, why, now),
        };
        if self.daily_held.is_some() {
            self.observe_input(&pending());
        }
        let mut events = self.reconnect_events();
        // A device that does not take window numbers is not given the pointer back in another
        // window (it would put it on the first).
        if !taken.windows {
            events.retain(|e| !istesso_proto::needs_windows(e));
        }
        let sent = client.send_input(&events).is_ok();
        if let Some(held) = &mut self.daily_held {
            // A failed write may have delivered some of the restored modifiers.
            held.record(&events, sent);
        }
        if !sent {
            return self.wait(Again::Soon, "the conversation ended at once".into(), now);
        }
        let now_open = Instant::now();
        let after = now_open.saturating_duration_since(self.without_since.take().unwrap_or(now));
        self.client = Some(client);
        self.alive = taken.alive;
        self.text = taken.text;
        self.preedit = taken.preedit;
        self.scroll = taken.scroll;
        self.scroll_told = false;
        self.clipboard = taken.clipboard;
        self.windows = taken.windows;
        self.windows_told = false;
        self.unanswered_since = None;
        (self.said, self.listened, self.made_at) = (now_open, now_open, now_open);
        Ok(Some(Happened::Back { after }))
    }
}

/// What the device said when the conversation was opened.
struct Taken {
    /// It answers "still here".
    alive: bool,
    /// It takes committed text.
    text: bool,
    /// The level of scrolls it takes.
    scroll: u8,
    /// It takes copy and paste (and answers "still here", where their answers are heard).
    clipboard: bool,
    /// It takes composing text.
    preedit: bool,
    /// It takes the application's other windows (trial only).
    windows: bool,
}

/// Asks whether input is taken, and to be answered when saying "still
/// here"; the answer has `within` this time. `Ok` says whether the device
/// answers; `Err` what to do about its refusal, and the refusal in a
/// sentence. Afterwards a write may take [`AGAIN_WITHIN`] at most.
/// `Ok`: what the device said ([`Taken`]).
fn ask(client: &mut Client, within: Duration) -> Result<Taken, (Again, String)> {
    let link = |err: LinkError| {
        let why = err.to_string();
        (after_failure(&ConnectFailure::Link(err)), why)
    };
    client.set_timeout(within).map_err(link)?;
    let answer = client.open_input_alive().map_err(link)?;
    client.set_timeout(AGAIN_WITHIN).map_err(link)?;
    match answer {
        // Copy and paste only with a device that answers "still here": their answers are heard
        // where that is.
        InputAnswer::Open {
            alive,
            text,
            scroll,
            clipboard,
            preedit,
            windows,
        } => Ok(Taken {
            alive,
            text,
            scroll,
            clipboard: clipboard && alive,
            preedit,
            windows,
        }),
        InputAnswer::Refused { code, message } => Err((after_refusal(code), message)),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::testing::{Between, InputPair};

    const A_MOMENT: Duration = Duration::from_millis(300);

    fn key(code: u32, pressed: bool) -> InputEvent {
        InputEvent::Key { code, pressed }
    }

    /// A line of the device to the host's agent by way of `addr`.
    fn line_through(pair: &std::sync::Arc<InputPair>, addr: std::net::SocketAddr) -> InputLine {
        let client = pair.connect_to(addr, Duration::from_secs(3)).unwrap();
        let again = pair.clone();
        InputLine::open(
            client,
            Box::new(move |within| again.connect_to(addr, within)),
        )
        .unwrap()
    }

    /// Looks after the line as the relay does, until something happens to it.
    fn turn_until_something(
        line: &mut InputLine,
        within: Duration,
    ) -> Result<Option<Happened>, String> {
        let began = Instant::now();
        while began.elapsed() < within {
            if let Some(happened) = line.turn(Instant::now())? {
                return Ok(Some(happened));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(None)
    }

    #[test]
    fn a_line_that_goes_silent_is_opened_again_and_what_was_typed_meanwhile_is_gone() {
        let pair = std::sync::Arc::new(InputPair::new("ra"));
        let between = Between::new(pair.addr);
        let mut line = line_through(&pair, between.addr);
        assert!(line.is_answered());

        // Shift is held, a letter goes down and the pointer is somewhere.
        let now = Instant::now();
        assert!(
            line.pass(
                &[key(42, true), key(30, true), InputEvent::pointer_at(7, 8)],
                now
            )
            .sent
        );
        assert_eq!(pair.all_handed_over(A_MOMENT).len(), 3);
        // An idle line stays: "still here" is said and answered.
        assert_eq!(turn_until_something(&mut line, SILENT_AFTER * 2), Ok(None));

        // The network stops delivering, and tells nobody.
        let silent_from = Instant::now();
        between.fall_silent();
        assert_eq!(
            turn_until_something(&mut line, Duration::from_secs(3)),
            Ok(Some(Happened::Cut))
        );
        let noticed = silent_from.elapsed();
        // Half a second at most until the next "still here", and half a second for
        // its answer.
        assert!(noticed >= UNANSWERED_FOR, "{noticed:?}");
        assert!(
            noticed < SILENT_AFTER + Duration::from_millis(600),
            "{noticed:?}"
        );
        assert!(!line.is_open());

        // Typed while there is no conversation: not sent, now or later. The letter
        // that was down comes up meanwhile; Shift stays held.
        let passed = line.pass(
            &[key(48, true), key(48, false), key(30, false)],
            Instant::now(),
        );
        assert_eq!(
            passed,
            Passed {
                sent: false,
                happened: None
            }
        );

        let Ok(Some(Happened::Back { after })) =
            turn_until_something(&mut line, Duration::from_secs(3))
        else {
            panic!("the line did not come back");
        };
        assert!(after < AGAIN_WITHIN, "{after:?}");
        assert_eq!(between.connections(), 2);
        assert_eq!(line.cuts(), 1);
        // The host let go of what the first conversation had down, before the second
        // pressed anything; then Shift is down again and the pointer is where it was.
        // The letter is not pressed again, and what was typed in between never arrives.
        assert_eq!(
            pair.all_handed_over(A_MOMENT),
            [
                key(30, false),
                key(42, false),
                // No composing text stays in the application either.
                InputEvent::Preedit {
                    text: String::new(),
                    cursor: [0, 0]
                },
                key(42, true),
                InputEvent::pointer_at(7, 8)
            ]
        );
        // And the line carries input again.
        assert!(line.pass(&[key(31, true)], Instant::now()).sent);
        assert_eq!(
            pair.handed_over(Duration::from_secs(3)),
            Some(key(31, true))
        );
    }

    #[test]
    fn a_copy_is_answered_soon_and_none_is_sent_to_a_device_that_does_not_take_it() {
        use crate::clipboard::{ANSWER_SOCKET, CLIPBOARD_SOCKET};
        use std::os::unix::net::UnixDatagram;
        // A device without a compositor that answers: copy and paste are not sent.
        let without = std::sync::Arc::new(InputPair::with_clipboard("rn"));
        let mut line = line_through(&without, without.addr);
        assert!(!line.takes_clipboard());
        assert!(
            line.pass(&[InputEvent::Copy { id: 1, cut: false }], Instant::now())
                .sent
        );
        assert_eq!(line.clipboard_dropped(), 1);
        assert_eq!(line.listen_in(), LISTEN_EVERY);

        let pair = std::sync::Arc::new(InputPair::with_clipboard("rc"));
        let dir = pair.host.path().to_path_buf();
        let compositor = UnixDatagram::bind(dir.join(CLIPBOARD_SOCKET)).unwrap();
        let answers = dir.join(ANSWER_SOCKET);
        let hello = std::thread::spawn(move || {
            let mut buffer = [0u8; 256];
            let length = compositor.recv(&mut buffer).unwrap();
            compositor.send_to(&buffer[..length], answers).unwrap();
            compositor
        });
        let mut line = line_through(&pair, pair.addr);
        let _compositor = hello.join().unwrap();
        assert!(line.takes_clipboard());
        assert!(
            line.pass(&[InputEvent::Copy { id: 2, cut: true }], Instant::now())
                .sent
        );
        // While the answer is waited for, the line is looked after more often.
        assert_eq!(line.listen_in(), CLIPBOARD_EVERY);
        pair.taken.set_read_timeout(Some(A_MOMENT)).unwrap();
        let mut buffer = [0u8; 512];
        let length = pair.taken.recv(&mut buffer).unwrap();
        let mark: serde_json::Value = serde_json::from_slice(&buffer[..length]).unwrap();
        assert_eq!(mark["cut"], true);
        UnixDatagram::unbound()
            .unwrap()
            .send_to(
                &serde_json::to_vec(&serde_json::json!({
                    "kind": "answer", "nonce": mark["nonce"], "result": "refused", "why": "password_field"
                }))
                .unwrap(),
                dir.join(ANSWER_SOCKET),
            )
            .unwrap();
        let began = Instant::now();
        let mut answers = Vec::new();
        while answers.is_empty() && began.elapsed() < Duration::from_secs(1) {
            std::thread::sleep(line.listen_in());
            assert_eq!(line.turn(Instant::now()), Ok(None));
            answers = line.take_clipboard_answers();
        }
        // Within a few turns of the fast pace, not after half a second.
        assert!(
            began.elapsed() < Duration::from_millis(400),
            "{:?}",
            began.elapsed()
        );
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].id, 2);
        assert_eq!(answers[0].result, istesso_proto::ClipboardResult::Refused);
        assert_eq!(
            answers[0].why,
            Some(istesso_proto::ClipboardWhy::PasswordField)
        );
        assert_eq!(line.listen_in(), LISTEN_EVERY);
    }

    #[test]
    fn focus_and_resize_are_handed_over_and_a_size_out_of_bounds_is_not() {
        let pair = std::sync::Arc::new(InputPair::new("fr"));
        let mut line = line_through(&pair, pair.addr);
        let events = [
            InputEvent::Focus { window: None },
            InputEvent::Resize {
                width: 1280,
                height: 720,
                window: None,
            },
        ];
        assert!(line.pass(&events, Instant::now()).sent);
        assert_eq!(pair.all_handed_over(A_MOMENT), events);
        // The agent drops a size it does not take (64 to 8192 a side); the keys beside it go on.
        let beside = [
            InputEvent::Resize {
                width: 10,
                height: 720,
                window: None,
            },
            key(30, true),
            key(30, false),
        ];
        assert!(line.pass(&beside, Instant::now()).sent);
        assert_eq!(
            pair.all_handed_over(A_MOMENT),
            [key(30, true), key(30, false)]
        );
    }

    /// What is meant for the application's other windows (trial only), and a key beside it.
    fn for_the_windows() -> [InputEvent; 8] {
        [
            InputEvent::Windows { version: 1 },
            InputEvent::PointerAt {
                x: 3,
                y: 4,
                window: Some(2),
            },
            InputEvent::pointer_at(5, 6),
            InputEvent::PointerAt {
                x: 7,
                y: 8,
                window: Some(1),
            },
            InputEvent::Focus { window: Some(2) },
            InputEvent::Resize {
                width: 640,
                height: 480,
                window: Some(3),
            },
            InputEvent::Close { window: 2 },
            key(30, true),
        ]
    }

    #[test]
    fn window_numbers_close_and_windows_go_to_a_device_that_takes_them() {
        let pair = std::sync::Arc::new(InputPair::new("rw"));
        let mut line = line_through(&pair, pair.addr);
        assert!(line.takes_windows());
        let events = for_the_windows();
        assert!(line.pass(&events, Instant::now()).sent);
        assert_eq!(pair.all_handed_over(A_MOMENT), events);
        assert_eq!(line.windows_dropped(), 0);
    }

    #[test]
    fn nothing_for_another_window_goes_to_a_device_that_does_not_take_it() {
        use istesso_link::{LocalKeys, Request, Response, Session};
        let pair = std::sync::Arc::new(InputPair::new("rv"));
        // A stand-in for an agent that takes input and says nothing of windows: it keeps
        // what it is sent.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let paths = pair.host.paths();
        let (sent, received) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let identity = istesso_pair::Identity::load_or_create(&paths.identity()).unwrap();
            let keys = LocalKeys::new(&identity).unwrap();
            let (stream, _) = listener.accept().unwrap();
            let peers = istesso_pair::PeerStore::open(paths.peers()).unwrap();
            let mut session =
                Session::accept(stream, &keys, &peers, Duration::from_secs(5)).unwrap();
            while let Ok(Some(request)) = session.next_request() {
                match request {
                    Request::InputOpen { .. } => session
                        .respond(&Response::InputOpen {
                            alive: false,
                            text: true,
                            scroll: 0,
                            clipboard: false,
                            preedit: true,
                            windows: false,
                        })
                        .unwrap(),
                    Request::Input { events } => {
                        for event in events {
                            let _ = sent.send(event);
                        }
                    }
                    _ => {}
                }
            }
        });
        let mut line = line_through(&pair, addr);
        assert!(!line.takes_windows());
        assert!(line.pass(&for_the_windows(), Instant::now()).sent);
        // The pointer in the first window, said either way, and the key go; the rest is
        // counted and goes nowhere: no pointer meant for another window lands on the first.
        let mut arrived = Vec::new();
        while let Ok(event) = received.recv_timeout(A_MOMENT) {
            arrived.push(event);
        }
        assert_eq!(
            arrived,
            [
                InputEvent::pointer_at(5, 6),
                InputEvent::PointerAt {
                    x: 7,
                    y: 8,
                    window: Some(1),
                },
                key(30, true),
            ]
        );
        assert_eq!(line.windows_dropped(), 5);
        // Only such events: nothing goes, and the conversation is not taken for cut.
        assert!(
            line.pass(&[InputEvent::Close { window: 2 }], Instant::now())
                .sent
        );
        assert_eq!(line.windows_dropped(), 6);
        assert!(received.recv_timeout(A_MOMENT).is_err());
        assert_eq!(line.cuts(), 0);
    }

    #[test]
    fn daily_recovery_releases_even_keys_whose_last_write_failed() {
        let mut held = HeldInput::default();
        held.record(&[key(30, true), key(42, true)], true);
        held.record(&[key(30, false)], true);
        held.record(&[key(42, false), key(29, true)], false);
        assert_eq!(held.releases(), [key(29, false), key(42, false)]);
    }

    #[test]
    fn daily_reconnect_uses_the_last_modifier_state_and_never_retypes_a_key() {
        let mut line = InputLine::for_daily_window(Box::new(|_| unreachable!()));
        let button = InputEvent::Button {
            code: 0x110,
            pressed: true,
        };
        let before = [key(42, true), key(56, true), key(30, true), button];
        line.observe_input(&before);
        line.daily_held.as_mut().unwrap().record(&before, true);

        // Shift stays held. Alt comes up, Control goes down and a letter is typed.
        assert!(
            !line
                .pass(
                    &[key(56, false), key(29, true), key(48, true)],
                    Instant::now()
                )
                .sent
        );
        // A whole modifier tap queued during the handshake is already over.
        line.observe_input(&[key(125, true), key(125, false), key(48, false)]);
        assert_eq!(
            line.reconnect_events(),
            [
                key(30, false),
                key(42, false),
                key(56, false),
                InputEvent::Button {
                    code: 0x110,
                    pressed: false
                },
                key(29, true),
                key(42, true),
            ]
        );
    }

    #[test]
    fn daily_relay_discards_offline_input_and_backs_off_until_the_host_returns() {
        let mut line = InputLine::for_daily_window(Box::new(|_| {
            Err(ConnectFailure::Link(LinkError::Refused))
        }));
        let now = line.due;
        for elapsed in [Duration::ZERO, SOON_FOR, Duration::from_secs(60)] {
            let at = now + elapsed;
            assert_eq!(line.turn(at), Ok(None));
            assert_eq!(
                line.due,
                at + if elapsed < SOON_FOR {
                    RETRY
                } else {
                    RETRY_SLOWLY
                }
            );
            assert_eq!(line.retry_in(at), LISTEN_EVERY);
            assert_eq!(
                line.retry_in(line.due - Duration::from_millis(7)),
                Duration::from_millis(7)
            );
            assert_eq!(line.retry_in(line.due), Duration::ZERO);
            assert!(!line.pass(&[key(42, true), key(30, true)], at).sent);
        }
        assert!(line.daily_held.as_ref().unwrap().releases().is_empty());
        assert_eq!(line.cuts(), 0);
    }

    #[test]
    fn daily_input_releases_old_keys_and_buttons_then_restores_current_modifiers() {
        use std::os::unix::net::UnixDatagram;
        let pair = std::sync::Arc::new(InputPair::with_pipeline_restarts("restart", true));
        let again = pair.clone();
        let mut line = InputLine::for_daily_window(Box::new(move |within| {
            again.connect_to(again.addr, within)
        }));
        let wait = Duration::from_secs(5);
        // Before the first conversation, the line does not know whether text is taken.
        assert!(!line.takes_text());
        assert!(matches!(
            turn_until_something(&mut line, wait),
            Ok(Some(Happened::Back { .. }))
        ));
        // The daily line learns it when the conversation opens, as the first line does:
        // this agent takes committed text, and it is handed over.
        assert!(line.takes_text());
        // And scrolls with their kind: a stage that moves nothing goes too.
        assert_eq!(line.scroll_level(), istesso_proto::SCROLL_LEVEL);
        let stage = InputEvent::Scroll {
            dx: 0,
            dy: 0,
            source: Some(istesso_proto::ScrollSource::Finger),
            v120: None,
            pixels: Some([0.0, 0.0]),
            phase: Some(istesso_proto::ScrollPhase::Began),
            momentum: None,
            momentum_follows: false,
        };
        assert!(line.pass(std::slice::from_ref(&stage), Instant::now()).sent);
        assert_eq!(pair.all_handed_over(A_MOMENT), [stage]);
        let text = InputEvent::Text {
            text: "日本語".into(),
        };
        assert!(line.pass(std::slice::from_ref(&text), Instant::now()).sent);
        assert_eq!(pair.all_handed_over(A_MOMENT), [text]);
        assert_eq!(line.text_dropped(), 0);
        let button = |pressed| InputEvent::Button {
            code: 0x110,
            pressed,
        };
        let down = [key(42, true), key(30, true), button(true)];
        assert!(line.pass(&down, Instant::now()).sent);
        assert_eq!(pair.all_handed_over(A_MOMENT), down);

        let path = pair
            .taken
            .local_addr()
            .unwrap()
            .as_pathname()
            .unwrap()
            .to_owned();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            turn_until_something(&mut line, wait),
            Ok(Some(Happened::Cut))
        );
        // The agent still answers, but cannot open its pipeline's input socket.
        assert_eq!(turn_until_something(&mut line, A_MOMENT), Ok(None));
        assert!(
            !line
                .pass(
                    &[key(29, true), key(48, true), key(30, false)],
                    Instant::now()
                )
                .sent
        );

        let restarted = UnixDatagram::bind(path).unwrap();
        restarted.set_read_timeout(Some(wait)).unwrap();
        std::thread::sleep(line.due.saturating_duration_since(Instant::now()));
        // Control comes up during the handshake. Only Shift is still held.
        assert!(matches!(
            line.turn_with_pending(Instant::now(), || vec![key(29, false), key(32, true)]),
            Ok(Some(Happened::Back { .. }))
        ));
        assert!(line.pass(&[key(31, true)], Instant::now()).sent);
        let mut buffer = [0u8; 512];
        for expected in [
            key(30, false),
            key(42, false),
            button(false),
            key(42, true),
            key(31, true),
        ] {
            let len = restarted.recv(&mut buffer).unwrap();
            assert_eq!(
                serde_json::from_slice::<InputEvent>(&buffer[..len]).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn typing_without_a_pause_is_not_silence() {
        let pair = std::sync::Arc::new(InputPair::new("re"));
        let mut line = line_through(&pair, pair.addr);
        // This agent says it takes committed text, and it is handed over.
        assert!(line.takes_text());
        let text = InputEvent::Text {
            text: "日本語".into(),
        };
        assert!(line.pass(std::slice::from_ref(&text), Instant::now()).sent);
        assert_eq!(pair.all_handed_over(A_MOMENT), [text]);
        assert_eq!(line.text_dropped(), 0);
        // Keys for two seconds, twenty a second: none of them is answered, and the
        // line is alive all the same, because "still here" is said beside them.
        let began = Instant::now();
        let (mut typed, mut handed_over) = (0, 0);
        while began.elapsed() < SILENT_AFTER * 2 {
            let now = Instant::now();
            assert!(line.pass(&[key(30, true), key(30, false)], now).sent);
            typed += 2;
            assert_eq!(line.turn(now), Ok(None), "after {:?}", began.elapsed());
            // Taken as it comes, as a program that shows an application does.
            handed_over += pair.all_handed_over(Duration::from_millis(50)).len();
        }
        assert!(line.is_open());
        assert_eq!(line.cuts(), 0);
        handed_over += pair.all_handed_over(A_MOMENT).len();
        assert_eq!(handed_over, typed);
    }

    #[test]
    fn a_sender_that_stood_still_itself_does_not_take_that_for_silence() {
        let pair = std::sync::Arc::new(InputPair::new("rf"));
        let mut line = line_through(&pair, pair.addr);
        // Looked after until "still here" has just been said and not been answered yet.
        let began = Instant::now();
        while line.unanswered_since.is_none() {
            assert!(began.elapsed() < Duration::from_secs(3), "it never asked");
            assert_eq!(line.turn(Instant::now()), Ok(None));
            std::thread::sleep(Duration::from_millis(1));
        }
        // This program is not run for a while (the machine was busy, or asleep for a
        // moment): for longer than silence may last. The host lets go of a device
        // only after two seconds without a word, counted from the "still here" just
        // said, so the conversation is still there, and the answer has come
        // meanwhile and waits to be read.
        let stood_still_from = Instant::now();
        std::thread::sleep(SILENT_AFTER + Duration::from_millis(300));
        // The question is older now than an answer may be, counted by the clock. What
        // has arrived is looked at before that is believed: the line is not cut, and
        // the question it has open now, if any, is a new one.
        assert_eq!(line.turn(Instant::now()), Ok(None));
        assert!(line.is_open());
        assert!(
            line.unanswered_since
                .is_none_or(|since| since > stood_still_from),
            "the answer was not heard"
        );
        assert_eq!(line.cuts(), 0);
        // And it goes on as before.
        assert_eq!(turn_until_something(&mut line, ALIVE_EVERY * 3), Ok(None));
    }

    #[test]
    fn a_device_that_is_unpaired_is_asked_once_and_then_left_alone() {
        let pair = std::sync::Arc::new(InputPair::new("rb"));
        let between = Between::new(pair.addr);
        let mut line = line_through(&pair, between.addr);

        istesso_pair::PeerStore::open(pair.host.paths().peers())
            .unwrap()
            .remove("a laptop")
            .unwrap()
            .unwrap();
        // The host stops taking the input when it next looks (a second apart at most)
        // and closes; the line asks again and is told that it is not trusted.
        let began = Instant::now();
        let why = loop {
            assert!(began.elapsed() < Duration::from_secs(5), "it goes on");
            match line.turn(Instant::now()) {
                Err(why) => break why,
                Ok(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        };
        assert!(why.contains("does not trust"), "{why}");
        assert_eq!(between.connections(), 2, "one question, and no more");
    }

    #[test]
    fn a_device_that_may_not_control_any_more_is_asked_once_and_then_left_alone() {
        let pair = std::sync::Arc::new(InputPair::new("rc"));
        let between = Between::new(pair.addr);
        let mut line = line_through(&pair, between.addr);

        let mut peers = istesso_pair::PeerStore::open(pair.host.paths().peers()).unwrap();
        let mut laptop = peers.find("a laptop").unwrap().unwrap().clone();
        laptop.permissions = vec![istesso_pair::Permission::View];
        peers.add(laptop).unwrap();
        let began = Instant::now();
        let why = loop {
            assert!(began.elapsed() < Duration::from_secs(5), "it goes on");
            match line.turn(Instant::now()) {
                Err(why) => break why,
                Ok(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        };
        assert!(why.contains("may not control"), "{why}");
        assert_eq!(between.connections(), 2, "one question, and no more");
    }

    #[test]
    fn a_host_of_the_version_before_is_not_taken_to_be_gone_for_saying_nothing() {
        use istesso_link::{LocalKeys, Request, Response, Session};
        let pair = std::sync::Arc::new(InputPair::new("rd"));
        // A stand-in for an agent that knows nothing of "still here": it takes input,
        // says so in the old words, and never answers an empty message.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let paths = pair.host.paths();
        std::thread::spawn(move || {
            let identity = istesso_pair::Identity::load_or_create(&paths.identity()).unwrap();
            let keys = LocalKeys::new(&identity).unwrap();
            let (stream, _) = listener.accept().unwrap();
            let peers = istesso_pair::PeerStore::open(paths.peers()).unwrap();
            let mut session =
                Session::accept(stream, &keys, &peers, Duration::from_secs(5)).unwrap();
            while let Ok(Some(request)) = session.next_request() {
                if let Request::InputOpen { .. } = request {
                    session
                        .respond(&Response::InputOpen {
                            alive: false,
                            text: false,
                            scroll: 0,
                            clipboard: false,
                            preedit: false,
                            windows: false,
                        })
                        .unwrap();
                }
            }
        });
        let mut line = line_through(&pair, addr);
        assert!(!line.is_answered());
        // It says nothing of scrolls: they go as dx and dy (it reads no more of them).
        assert_eq!(line.scroll_level(), 0);
        assert_eq!(turn_until_something(&mut line, SILENT_AFTER * 2), Ok(None));
        assert!(line.is_open());
        assert!(line.pass(&[key(30, true)], Instant::now()).sent);
        // It did not say it takes committed text: none is sent (it could not read the
        // message, keys and all); the keys beside it are.
        assert!(!line.takes_text());
        let text = InputEvent::Text {
            text: "日本語".into(),
        };
        assert!(line.pass(&[key(30, false), text], Instant::now()).sent);
        assert_eq!(line.text_dropped(), 1);
        assert_eq!(turn_until_something(&mut line, A_MOMENT), Ok(None));
        assert_eq!(line.cuts(), 0);
    }

    #[test]
    fn only_a_conversation_that_ended_without_a_word_is_opened_again_soon() {
        assert_eq!(after_refusal(ErrorCode::Forbidden), Again::Never);
        assert_eq!(after_refusal(ErrorCode::Unsupported), Again::Never);
        assert_eq!(after_refusal(ErrorCode::Other), Again::Never);
        // Nothing takes input over there at the moment: that may pass.
        assert_eq!(after_refusal(ErrorCode::Internal), Again::Slowly);
        for err in [LinkError::Closed, LinkError::Timeout, LinkError::Refused] {
            assert_eq!(after_failure(&ConnectFailure::Link(err)), Again::Soon);
        }
        for err in [LinkError::NotTrusted, LinkError::UnknownPeer] {
            assert_eq!(after_failure(&ConnectFailure::Link(err)), Again::Never);
        }
        assert_eq!(
            after_failure(&ConnectFailure::Here("no paired device matches".into())),
            Again::Never
        );
    }
}
