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
//! modifiers that are held are pressed again, and nothing else.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use istesso_link::{Client, Error as LinkError, ErrorCode, InputAnswer};
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
        // The host cannot do it now (the session codes of ADR 0010): it may later.
        ErrorCode::Unavailable => Again::Slowly,
        ErrorCode::Forbidden | ErrorCode::Unsupported | ErrorCode::NotFound | ErrorCode::Other => {
            Again::Never
        }
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

/// The conversation input is sent on.
pub struct InputLine {
    connect: Connect,
    client: Option<Client>,
    /// The device answers "still here".
    alive: bool,
    /// When "still here" was last said.
    said: Instant,
    /// When it was first said without an answer having come since. Silence
    /// is counted from here, not from the last answer: what counts is a
    /// question that went unanswered, not how long this side did not ask.
    unanswered_since: Option<Instant>,
    listened: Instant,
    /// The modifiers that are down on this side, as far as the events say.
    modifiers: BTreeSet<u32>,
    /// Where the pointer is, as far as the events say.
    pointer: Option<(i32, i32)>,
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
        let alive = ask(&mut client, FIRST_ANSWER_WITHIN)?;
        let now = Instant::now();
        Ok(InputLine {
            connect,
            client: Some(client),
            alive,
            said: now,
            unanswered_since: None,
            listened: now,
            modifiers: BTreeSet::new(),
            pointer: None,
            without_since: None,
            due: now,
            made_at: now,
            cuts: 0,
        })
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

    /// Whether the device answers "still here", so that silence is noticed.
    pub fn is_answered(&self) -> bool {
        self.alive
    }

    /// Sends what a person did. Without a conversation it is not sent, now
    /// or later.
    pub fn pass(&mut self, events: &[InputEvent], now: Instant) -> Passed {
        for event in events {
            match *event {
                InputEvent::Key { code, pressed } if MODIFIERS.contains(&code) => {
                    if pressed {
                        self.modifiers.insert(code);
                    } else {
                        self.modifiers.remove(&code);
                    }
                }
                InputEvent::PointerAt { x, y } => self.pointer = Some((x, y)),
                _ => {}
            }
        }
        let Some(client) = &mut self.client else {
            return Passed {
                sent: false,
                happened: None,
            };
        };
        // What a person types is not answered, so it says nothing about whether the
        // conversation is alive: "still here" goes on being said beside it.
        match client.send_input(events) {
            Ok(()) => Passed {
                sent: true,
                happened: None,
            },
            Err(_) => Passed {
                sent: false,
                happened: Some(self.cut(now)),
            },
        }
    }

    /// Looks after the conversation: says "still here", hears the answers,
    /// notices silence, and opens the conversation again when it was cut.
    /// To be called every [`LISTEN_EVERY`] or so. `Err` says why the device
    /// does not take input from here any more; nothing is tried after that.
    pub fn turn(&mut self, now: Instant) -> Result<Option<Happened>, String> {
        let Some(client) = &mut self.client else {
            return self.open_again(now);
        };
        let silent = self
            .unanswered_since
            .is_some_and(|since| now.saturating_duration_since(since) >= UNANSWERED_FOR);
        if self.alive && (silent || now.saturating_duration_since(self.listened) >= LISTEN_EVERY) {
            // Before silence is believed, what has arrived is looked at: an answer
            // that came while nobody listened is an answer.
            self.listened = now;
            match client.heard_alive() {
                Ok(true) => self.unanswered_since = None,
                Ok(false) if silent => return Ok(Some(self.cut(now))),
                Ok(false) => {}
                Err(_) => return Ok(Some(self.cut(now))),
            }
        }
        let every = if self.alive {
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

    fn open_again(&mut self, now: Instant) -> Result<Option<Happened>, String> {
        if now < self.due {
            return Ok(None);
        }
        let mut client = match (self.connect)(AGAIN_WITHIN) {
            Ok(client) => client,
            Err(failure) => return self.wait(after_failure(&failure), failure.to_string(), now),
        };
        let alive = match ask(&mut client, AGAIN_WITHIN) {
            Ok(alive) => alive,
            Err((what, why)) => return self.wait(what, why, now),
        };
        // The device let go of everything the conversation before had down. What is
        // held on this side now is in line again: the modifiers go down, and the
        // pointer is where it is. Other keys and the buttons are not pressed again: a
        // letter would be typed once more, a drag would begin somewhere else.
        let mut events: Vec<InputEvent> = self
            .modifiers
            .iter()
            .map(|&code| InputEvent::Key {
                code,
                pressed: true,
            })
            .collect();
        if let Some((x, y)) = self.pointer {
            events.push(InputEvent::PointerAt { x, y });
        }
        if client.send_input(&events).is_err() {
            return self.wait(Again::Soon, "the conversation ended at once".into(), now);
        }
        let now_open = Instant::now();
        let after = now_open.saturating_duration_since(self.without_since.take().unwrap_or(now));
        self.client = Some(client);
        self.alive = alive;
        self.unanswered_since = None;
        (self.said, self.listened, self.made_at) = (now_open, now_open, now_open);
        Ok(Some(Happened::Back { after }))
    }
}

/// Asks whether input is taken, and to be answered when saying "still
/// here"; the answer has `within` this time. `Ok` says whether the device
/// answers; `Err` what to do about its refusal, and the refusal in a
/// sentence. Afterwards a write may take [`AGAIN_WITHIN`] at most.
fn ask(client: &mut Client, within: Duration) -> Result<bool, (Again, String)> {
    let link = |err: LinkError| {
        let why = err.to_string();
        (after_failure(&ConnectFailure::Link(err)), why)
    };
    client.set_timeout(within).map_err(link)?;
    let answer = client.open_input_alive().map_err(link)?;
    client.set_timeout(AGAIN_WITHIN).map_err(link)?;
    match answer {
        InputAnswer::Open { alive } => Ok(alive),
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
                &[
                    key(42, true),
                    key(30, true),
                    InputEvent::PointerAt { x: 7, y: 8 }
                ],
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
                key(42, true),
                InputEvent::PointerAt { x: 7, y: 8 }
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
    fn typing_without_a_pause_is_not_silence() {
        let pair = std::sync::Arc::new(InputPair::new("re"));
        let mut line = line_through(&pair, pair.addr);
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
                        .respond(&Response::InputOpen { alive: false })
                        .unwrap();
                }
            }
        });
        let mut line = line_through(&pair, addr);
        assert!(!line.is_answered());
        assert_eq!(turn_until_something(&mut line, SILENT_AFTER * 2), Ok(None));
        assert!(line.is_open());
        assert!(line.pass(&[key(30, true)], Instant::now()).sent);
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
