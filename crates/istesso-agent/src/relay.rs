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
//! Opening it again is done with tries that overlap
//! ([`istesso_link::again`]): a host that does not answer makes each try
//! wait its deadline out, and the next try does not wait for that. The
//! request to send input, `input_open`, goes on the first connection whose
//! handshake completes, and on no other: the host ends the conversation
//! before, and lets go of its keys, only when that request arrives.
//!
//! What was typed while there was no conversation is not sent later: a key
//! that arrives a second late is typed into a picture that has changed
//! since, and what was sent just before the cut may have arrived, so sending
//! it again could type it twice. When the conversation is back, the
//! modifiers that are held are pressed again, and nothing else.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub use istesso_link::Again;
use istesso_link::{Client, Error as LinkError, ErrorCode, InputAnswer, Pace, Racer, Turn};
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
/// What the answer to `input_open` may take when the conversation is made
/// again ([`Pace::answer_within`]), and what a write may take. How often a
/// try at making it again is started, and what a try may take, is
/// [`Pace`]: the same numbers as for the picture and for sound.
pub const AGAIN_WITHIN: Duration = Duration::from_secs(1);
/// What the answer to the first request may take. A host that is busy
/// answers late; that is not a host that refuses.
pub const FIRST_ANSWER_WITHIN: Duration = Duration::from_secs(3);

/// The keys that are pressed again when a conversation is back: Control,
/// Shift, Alt and Super, left and right (Linux key numbers).
const MODIFIERS: [u32; 8] = [29, 42, 54, 56, 97, 100, 125, 126];

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

/// One try at opening a conversation with the device: looking a name up,
/// connecting and the handshake, within this time, all of it together
/// ([`crate::Reaching::open`], which also sees to it that a try is one
/// connection, or two at most). Several tries run at once, each on a
/// thread of its own, and each makes its own keys.
pub type Connect = Arc<dyn Fn(Duration) -> Result<Client, ConnectFailure> + Send + Sync>;

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
    /// The tries at opening the conversation again, and how often it was
    /// cut.
    racer: Racer<Client, ConnectFailure>,
}

impl InputLine {
    /// Asks on a conversation that has just been opened whether input is
    /// taken there. `connect` is one try at opening the next one when this
    /// one is cut. `Err` says why there is no line, and what to do about
    /// it: a device that refuses is not asked again ([`Again::Never`]); one
    /// that did not answer in time may be, with a new conversation.
    pub fn open(mut client: Client, connect: Connect) -> Result<Self, (Again, String)> {
        let alive = ask(&mut client, FIRST_ANSWER_WITHIN)?;
        let now = Instant::now();
        let mut racer = Racer::new(Pace::default());
        racer.tries_mut().made(now);
        Ok(InputLine {
            connect,
            client: Some(client),
            alive,
            said: now,
            unanswered_since: None,
            listened: now,
            modifiers: BTreeSet::new(),
            pointer: None,
            racer,
        })
    }

    /// The same line with other numbers for its tries. For tests, which
    /// cannot wait two seconds for every try that gets no answer.
    pub fn paced(mut self, pace: Pace) -> Self {
        self.racer = Racer::new(pace);
        self.racer.tries_mut().made(Instant::now());
        self
    }

    /// How often the conversation was cut since this line was opened,
    /// whether or not it is back. One cut is one, however many tries it
    /// took to come back. A measurement that is to say something about an
    /// undisturbed line wants this to be 0.
    pub fn cuts(&self) -> u64 {
        self.racer.tries().cuts()
    }

    /// How many tries at opening the conversation again were started, over
    /// all cuts.
    pub fn tries(&self) -> u64 {
        self.racer.tries().started()
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
        if self.client.is_none() {
            return self.open_again();
        }
        // Tries that were dropped when the conversation came back end later;
        // what they bring is closed here.
        let _ = self
            .racer
            .turn(Duration::ZERO, &self.connect, after_failure);
        let Some(client) = &mut self.client else {
            return Ok(None);
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
        self.client = None;
        // The first try at once, unless the conversation had only just been
        // opened: a device that takes one and lets go of it straight away is not
        // asked faster than any other.
        self.racer.tries_mut().cut(now);
        Happened::Cut
    }

    /// The request on the chosen connection was not granted, or the
    /// connection did not last: the tries go on, at leisure if that is what
    /// the answer asks for, or it is over.
    fn unanswered(&mut self, what: Again, why: String) -> Result<Option<Happened>, String> {
        self.racer.tries_mut().unanswered(what, Instant::now());
        match what {
            Again::Never => Err(why),
            Again::Slowly | Again::Soon => Ok(None),
        }
    }

    /// Starts the tries that are due, without waiting for any, and asks on
    /// the first connection whose handshake has completed whether input is
    /// taken there. Only that asking waits for the device, and only for
    /// [`AGAIN_WITHIN`].
    fn open_again(&mut self) -> Result<Option<Happened>, String> {
        // A connection that the host has closed since its handshake (it holds
        // only so many of a device that have not asked for anything yet) is not
        // one that was opened: it counts as a try that failed.
        let still_open = |client: &Client| !client.closed_by_peer();
        let turn = self
            .racer
            .turn_if(Duration::ZERO, &self.connect, after_failure, still_open);
        let mut client = match turn {
            Turn::Nothing => return Ok(None),
            Turn::Stopped(failure) => return Err(failure.to_string()),
            Turn::Opened(client) => client,
        };
        // This is the connection that is used: the request goes on it and on no
        // other. The host ends the conversation before when it arrives.
        let alive = match ask(&mut client, self.racer.tries().pace().answer_within) {
            Ok(alive) => alive,
            Err((what, why)) => return self.unanswered(what, why),
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
            return self.unanswered(Again::Soon, "the conversation ended at once".into());
        }
        let now_open = Instant::now();
        let after = self.racer.tries_mut().answered(now_open);
        self.client = Some(client);
        self.alive = alive;
        self.unanswered_since = None;
        (self.said, self.listened) = (now_open, now_open);
        Ok(Some(Happened::Back { after }))
    }
}

/// Asks whether input is taken, and to be answered when saying "still
/// here"; the answer has `within` this time, all of it together, however
/// slowly it comes. `Ok` says whether the device answers; `Err` what to do
/// about its refusal, and the refusal in a sentence. Afterwards a write
/// may take [`AGAIN_WITHIN`] at most.
fn ask(client: &mut Client, within: Duration) -> Result<bool, (Again, String)> {
    let link = |err: LinkError| {
        let why = err.to_string();
        (after_failure(&ConnectFailure::Link(err)), why)
    };
    let by = Instant::now() + within;
    let answer = client.open_input_alive_by(by, AGAIN_WITHIN).map_err(link)?;
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

    /// The numbers of the product, but for the quick tries of the first
    /// seconds: a second try is half a second away. On the loopback the
    /// first try has long ended by then, so a test that counts connections
    /// counts the same on a machine that is busy with other things. For
    /// the same reason there are no early tries: one 10 ms after the first
    /// would be a second connection whenever the first took that long.
    fn unhurried() -> Pace {
        Pace {
            quick: Duration::from_millis(500),
            early: &[],
            ..Pace::default()
        }
    }

    /// A line of the device to the host's agent by way of `addr`.
    fn line_through(pair: &std::sync::Arc<InputPair>, addr: std::net::SocketAddr) -> InputLine {
        line_paced(pair, addr, unhurried())
    }

    fn line_paced(
        pair: &std::sync::Arc<InputPair>,
        addr: std::net::SocketAddr,
        pace: Pace,
    ) -> InputLine {
        let client = pair.connect_to(addr, Duration::from_secs(3)).unwrap();
        let again = pair.clone();
        InputLine::open(client, Arc::new(move |within| again.try_to(addr, within)))
            .unwrap()
            .paced(pace)
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

    /// Test 11 of ADR 0007, the device's side: several tries are under way
    /// at once and all of them complete. The request goes on the first, the
    /// others are closed, and the host sees one device come back once.
    #[test]
    fn of_several_tries_that_complete_one_asks_and_the_others_are_closed() {
        let pair = Arc::new(InputPair::new("rg"));
        let between = Between::new(pair.addr);
        let client = pair
            .connect_to(between.addr, Duration::from_secs(3))
            .unwrap();
        // Every try takes a while to complete, as over a long way, so that the
        // tries that follow it are under way when the first one is used.
        let slowly: Connect = {
            let (pair, addr) = (pair.clone(), between.addr);
            Arc::new(move |within| {
                let client = pair.try_to(addr, within)?;
                std::thread::sleep(Duration::from_millis(400));
                Ok(client)
            })
        };
        let mut line = InputLine::open(client, slowly).unwrap();
        // Shift is held, and the pointer is somewhere.
        let held = [key(42, true), InputEvent::PointerAt { x: 7, y: 8 }];
        assert!(line.pass(&held, Instant::now()).sent);
        assert_eq!(pair.all_handed_over(A_MOMENT).len(), 2);

        between.fall_silent();
        assert_eq!(
            turn_until_something(&mut line, Duration::from_secs(3)),
            Ok(Some(Happened::Cut))
        );
        assert!(matches!(
            turn_until_something(&mut line, Duration::from_secs(3)),
            Ok(Some(Happened::Back { .. }))
        ));
        // Four tries were started a tenth of a second apart while the first was
        // on its way (fewer, if this machine was busy with something else and
        // woke the line late; more than one, or nothing was under way at once).
        let tries = line.tries();
        assert!((2..=4).contains(&tries), "{tries} tries");
        assert_eq!(between.connections(), 1 + tries);
        assert_eq!(line.cuts(), 1);

        // The host let go of Shift once, when the request came, and then Shift
        // was pressed again and the pointer put where it is.
        let came_back = [key(42, false), key(42, true), held[1]];
        assert_eq!(pair.all_handed_over(A_MOMENT), came_back);
        // The other tries complete, and are closed without having asked for
        // anything. Nothing more happens at the host for them: no key comes up
        // a second time, and nothing is said.
        assert_eq!(
            turn_until_something(&mut line, Duration::from_millis(900)),
            Ok(None)
        );
        pair.until(|| {
            assert_eq!(line.turn(Instant::now()), Ok(None));
            line.racer.tries().running() == 0 && pair.waiting.waiting() == 0
        });
        assert!(pair.all_handed_over(A_MOMENT).is_empty());
        assert!(pair.said.lock().unwrap().is_empty());
        assert_eq!(line.tries(), tries, "tries went on after it was back");
        // And the line carries input.
        assert!(line.is_open());
        assert!(line.pass(&[key(31, true)], Instant::now()).sent);
        assert_eq!(
            pair.handed_over(Duration::from_secs(3)),
            Some(key(31, true))
        );
    }

    /// Test 15: while the host is away, the tries overlap: each waits its
    /// time out, and the next does not wait for it. When the host is back,
    /// the next try reaches it. It was one cut.
    #[test]
    fn a_host_that_was_away_is_reached_by_the_next_try_after_it_is_back() {
        let pair = Arc::new(InputPair::new("rh"));
        let between = Between::new(pair.addr);
        // Shorter times than the product's, in the same proportion: a try may
        // take four times as long as the tries are apart.
        let pace = Pace {
            every: Duration::from_millis(250),
            quick: Duration::from_millis(50),
            try_within: Duration::from_secs(1),
            ..Pace::default()
        };
        let mut line = line_paced(&pair, between.addr, pace);

        between.host_is_away(true);
        assert_eq!(
            turn_until_something(&mut line, Duration::from_secs(3)),
            Ok(Some(Happened::Cut))
        );
        let away_from = Instant::now();
        let mut most = 0;
        while away_from.elapsed() < Duration::from_millis(2500) {
            assert_eq!(line.turn(Instant::now()), Ok(None));
            most = most.max(line.racer.tries().running());
            std::thread::sleep(Duration::from_millis(20));
        }
        // Several at once, and never more than may be. (Eight at most, and
        // ten or more tries in that time, on a machine that is not busy; the
        // numbers leave room for one that is.)
        assert!((3..=8).contains(&most), "{most} tries at once");
        let tried = line.tries();
        assert!(tried >= 6, "{tried} tries in two and a half seconds");

        between.host_is_away(false);
        let back_from = Instant::now();
        assert!(matches!(
            turn_until_something(&mut line, Duration::from_secs(3)),
            Ok(Some(Happened::Back { .. }))
        ));
        // The next try was a quarter of a second away at most.
        let took = back_from.elapsed();
        assert!(took < Duration::from_millis(1500), "back after {took:?}");
        assert!(
            line.tries() - tried <= 2,
            "{} more tries",
            line.tries() - tried
        );
        assert_eq!(line.cuts(), 1);
        assert!(line.pass(&[key(31, true)], Instant::now()).sent);
        assert_eq!(
            pair.handed_over(Duration::from_secs(3)),
            Some(key(31, true))
        );
    }

    /// Test 16: a host of the version before, which knows no limit on the
    /// connections that wait and gives each five seconds, is reached by
    /// tries that overlap as by any other: it, too, takes the input over
    /// only when it is asked to.
    #[test]
    fn a_host_of_the_version_before_is_come_back_to_with_tries_that_overlap() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        use istesso_link::{LocalKeys, Request, Response, Session};
        let pair = Arc::new(InputPair::new("ri"));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let paths = pair.host.paths();
        let asked = Arc::new(AtomicUsize::new(0));
        let counted = asked.clone();
        std::thread::spawn(move || {
            let identity = istesso_pair::Identity::load_or_create(&paths.identity()).unwrap();
            let keys = Arc::new(LocalKeys::new(&identity).unwrap());
            for stream in listener.incoming() {
                let (stream, keys, paths) = (stream.unwrap(), keys.clone(), paths.clone());
                let counted = counted.clone();
                std::thread::spawn(move || {
                    let peers = istesso_pair::PeerStore::open(paths.peers()).unwrap();
                    let Ok(mut session) =
                        Session::accept(stream, &keys, &peers, Duration::from_secs(5))
                    else {
                        return;
                    };
                    let mut answers = false;
                    while let Ok(Some(request)) = session.next_request() {
                        let response = match request {
                            Request::InputOpen { alive } => {
                                counted.fetch_add(1, Ordering::SeqCst);
                                answers = alive;
                                Response::InputOpen { alive }
                            }
                            Request::Input { events } if events.is_empty() && answers => {
                                Response::InputAlive
                            }
                            _ => continue,
                        };
                        if session.respond(&response).is_err() {
                            return;
                        }
                    }
                });
            }
        });
        let between = Between::new(addr);
        let client = pair
            .connect_to(between.addr, Duration::from_secs(3))
            .unwrap();
        // As above: tries that take a while, so that several are under way.
        let slowly: Connect = {
            let (pair, addr) = (pair.clone(), between.addr);
            Arc::new(move |within| {
                let client = pair.try_to(addr, within)?;
                std::thread::sleep(Duration::from_millis(300));
                Ok(client)
            })
        };
        let mut line = InputLine::open(client, slowly).unwrap();
        assert!(line.is_answered());
        assert_eq!(asked.load(Ordering::SeqCst), 1);

        between.fall_silent();
        assert_eq!(
            turn_until_something(&mut line, Duration::from_secs(3)),
            Ok(Some(Happened::Cut))
        );
        assert!(matches!(
            turn_until_something(&mut line, Duration::from_secs(3)),
            Ok(Some(Happened::Back { .. }))
        ));
        assert!(line.tries() >= 2, "{} tries", line.tries());
        // Asked once more, on one connection, however many tries there were.
        assert_eq!(turn_until_something(&mut line, SILENT_AFTER * 2), Ok(None));
        assert_eq!(asked.load(Ordering::SeqCst), 2);
        assert_eq!(line.cuts(), 1);
        assert!(line.is_open());
    }

    /// Quitting while tries are under way: every try ends, what it opened
    /// is closed, and the host is left with nothing that waits and with
    /// nothing to say.
    #[test]
    fn a_line_that_is_dropped_while_tries_are_under_way_leaves_nothing_behind() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let pair = Arc::new(InputPair::new("rj"));
        let between = Between::new(pair.addr);
        let client = pair
            .connect_to(between.addr, Duration::from_secs(3))
            .unwrap();
        let under_way = Arc::new(AtomicUsize::new(0));
        // Tries that complete their handshake and take a while to say so.
        let slowly: Connect = {
            let (pair, addr, under_way) = (pair.clone(), between.addr, under_way.clone());
            Arc::new(move |within| {
                under_way.fetch_add(1, Ordering::SeqCst);
                let client = pair.try_to(addr, within);
                std::thread::sleep(Duration::from_millis(400));
                under_way.fetch_sub(1, Ordering::SeqCst);
                client
            })
        };
        let mut line = InputLine::open(client, slowly).unwrap();
        between.fall_silent();
        assert_eq!(
            turn_until_something(&mut line, Duration::from_secs(3)),
            Ok(Some(Happened::Cut))
        );
        assert_eq!(
            turn_until_something(&mut line, Duration::from_millis(150)),
            Ok(None)
        );
        // At least one try has got as far as the host, and none is back.
        pair.until(|| pair.waiting.waiting() >= 1);
        assert!(line.racer.tries().running() >= 1);
        assert!(!line.is_open());

        drop(line);
        pair.until(|| under_way.load(Ordering::SeqCst) == 0);
        pair.until(|| pair.waiting.waiting() == 0);
        // Nothing was asked of the host, and it did nothing.
        assert!(pair.all_handed_over(A_MOMENT).is_empty());
        assert!(pair.said.lock().unwrap().is_empty());
    }

    /// The known gap of ADR 0007, at the cost it has today: a request that
    /// arrives late, on a connection the device gave up long ago, makes the
    /// host take that connection and end the one in use. The device comes
    /// back once more. One more cut, and no more than that.
    #[test]
    fn a_request_that_arrives_late_costs_one_more_cut_and_no_more() {
        let pair = Arc::new(InputPair::giving_a_request("rk", Duration::from_secs(30)));
        let between = Between::new(pair.addr);
        let mut line = line_through(&pair, between.addr);
        assert!(line.pass(&[key(42, true)], Instant::now()).sent);
        assert_eq!(pair.all_handed_over(A_MOMENT), [key(42, true)]);

        // The connection whose request will be late: its handshake is done, and
        // its request is held up somewhere on the way.
        let mut late = pair.connect();
        // The line is cut meanwhile, and comes back on another connection.
        between.fall_silent();
        assert_eq!(
            turn_until_something(&mut line, Duration::from_secs(3)),
            Ok(Some(Happened::Cut))
        );
        assert!(matches!(
            turn_until_something(&mut line, Duration::from_secs(3)),
            Ok(Some(Happened::Back { .. }))
        ));
        assert_eq!(line.cuts(), 1);
        assert_eq!(
            pair.all_handed_over(A_MOMENT),
            [key(42, false), key(42, true)]
        );

        // Now the late request arrives, and behind it the closing of its
        // connection, which the device gave up when it got no answer.
        assert_eq!(
            late.open_input_alive().unwrap(),
            InputAnswer::Open { alive: true }
        );
        drop(late);
        // The host took the late connection and ended the one in use: the
        // line is cut once more, and comes back once more.
        assert_eq!(
            turn_until_something(&mut line, Duration::from_secs(3)),
            Ok(Some(Happened::Cut))
        );
        assert!(matches!(
            turn_until_something(&mut line, Duration::from_secs(3)),
            Ok(Some(Happened::Back { .. }))
        ));
        assert_eq!(line.cuts(), 2);
        // Shift came up when the late connection took over, and went down again
        // when the line was back: nothing else was typed by it.
        assert_eq!(
            pair.all_handed_over(A_MOMENT),
            [key(42, false), key(42, true)]
        );
        // And that is the end of it: the line stays.
        assert_eq!(turn_until_something(&mut line, SILENT_AFTER * 2), Ok(None));
        assert_eq!(line.cuts(), 2);
        assert!(line.is_open());
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
