//! What the picture does when it is told to stop, to come back, or when no
//! encoder can be had.
//!
//! The encoder is the scarce thing: a graphics card opens only a few at
//! once, for the whole machine. So a picture that nobody looks at gives its
//! encoder back, and a picture that is refused an encoder goes on without
//! one: the compositor and the application never end because of it
//! (ADR 0010, rule 8; ADR 0019).
//!
//! This is only the deciding. What is decided is done by whoever implements
//! [`Works`] (the pipeline; in tests a list of what was asked), on the one
//! thread that also reads what the program is told. Nothing here is on a
//! frame's path.

/// Which paired devices are shown the picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Who {
    /// Every paired device that may look: what a measuring pipeline does.
    Paired,
    /// This one device, by its id and its public key together.
    Only { id: String, public_key: String },
    /// Nobody.
    Nobody,
}

/// What the picture is made of, as far as these decisions reach.
pub trait Works {
    /// Shuts the gate and takes the encoder out. When this returns the
    /// encoder is closed and its place on the graphics card is free.
    fn close_encoder(&mut self);
    /// Puts an encoder in and opens the gate, which lets the last frame into
    /// it. `Err` with the reason when no encoder could be had; nothing of it
    /// is left in then, and the gate is shut.
    fn open_encoder(&mut self) -> Result<(), String>;
    /// Offers the picture to devices, or stops offering it: whoever watches
    /// is cut off at once and nobody is taken.
    fn offer(&mut self, offered: bool);
    /// Says who is shown the picture. `false` when that could not be said
    /// (an id or a key that cannot be one): then nobody is.
    fn admit(&mut self, who: &Who) -> bool;
}

/// Where the picture stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// An encoder is there and frames go through it.
    Running,
    /// An encoder has been put in; its first frame is not out yet.
    Opening,
    /// No encoder, because the picture was told to stop.
    Paused,
    /// No encoder, because none could be had. `resume` tries again.
    Wanting,
}

/// When an encoder was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum At {
    Start,
    Resume,
    Run,
}

/// What whoever started the program is to be told.
#[derive(Debug, Clone, PartialEq)]
pub enum Told {
    /// The encoder is closed. `took_ms` from being asked to its being closed.
    Paused { offered: bool, took_ms: f64 },
    /// The first frame of a new encoder is out. Counted from being asked:
    /// to the encoder being in, to the frame leaving it, to the frame having
    /// been handed on.
    Resumed {
        opened_ms: f64,
        encoded_ms: f64,
        written_ms: f64,
        key: bool,
    },
    /// No encoder could be had. No picture is made; everything else goes on.
    NoEncoder { at: At, reason: String },
    /// Who is shown the picture now: the device's id, or nobody.
    Admitted { device: Option<String> },
}

/// The picture of one session.
pub struct Picture {
    state: State,
    offered: bool,
    asked_ns: u64,
    opened_ns: u64,
    clock: Box<dyn Fn() -> u64 + Send>,
}

fn ms(from_ns: u64, to_ns: u64) -> f64 {
    to_ns.saturating_sub(from_ns) as f64 / 1e6
}

impl Picture {
    /// The picture as the program starts: `started` is how putting the first
    /// encoder in went. `clock` reads nanoseconds on the clock the frame
    /// log uses.
    pub fn begin(
        started: Result<(), String>,
        clock: impl Fn() -> u64 + Send + 'static,
    ) -> (Self, Vec<Told>) {
        let (state, told) = match started {
            Ok(()) => (State::Running, vec![]),
            Err(reason) => (
                State::Wanting,
                vec![Told::NoEncoder {
                    at: At::Start,
                    reason,
                }],
            ),
        };
        let picture = Picture {
            state,
            offered: true,
            asked_ns: 0,
            opened_ns: 0,
            clock: Box::new(clock),
        };
        (picture, told)
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Whether the picture is offered to devices.
    pub fn offered(&self) -> bool {
        self.offered
    }

    /// Stop making the picture and give the encoder back. Whoever watches
    /// stays connected.
    pub fn pause(&mut self, works: &mut impl Works) -> Vec<Told> {
        let took_ms = match self.state {
            State::Running | State::Opening => {
                let asked = (self.clock)();
                works.close_encoder();
                ms(asked, (self.clock)())
            }
            // Nothing is held: there is nothing to give back.
            State::Paused | State::Wanting => 0.0,
        };
        self.state = State::Paused;
        vec![Told::Paused {
            offered: self.offered,
            took_ms,
        }]
    }

    /// The same, and whoever watches is cut off first, before the encoder
    /// is closed: nothing goes to a device that was sent away while the
    /// encoder takes its time.
    pub fn detach(&mut self, works: &mut impl Works) -> Vec<Told> {
        works.offer(false);
        self.offered = false;
        self.pause(works)
    }

    /// Make the picture again, and offer it again.
    pub fn resume(&mut self, works: &mut impl Works) -> Vec<Told> {
        works.offer(true);
        self.offered = true;
        match self.state {
            State::Running | State::Opening => vec![],
            State::Paused | State::Wanting => {
                self.asked_ns = (self.clock)();
                match works.open_encoder() {
                    Ok(()) => {
                        self.opened_ns = (self.clock)();
                        self.state = State::Opening;
                        vec![]
                    }
                    Err(reason) => {
                        self.state = State::Wanting;
                        vec![Told::NoEncoder {
                            at: At::Resume,
                            reason,
                        }]
                    }
                }
            }
        }
    }

    /// The encoder's side has failed while it was in: a frame was refused
    /// at the gate, or one of its elements said so. It is taken out, and the
    /// picture waits for the next `resume`. A failure that arrives when
    /// there is no encoder any more (it was closed meanwhile) changes
    /// nothing.
    pub fn encoder_failed(&mut self, works: &mut impl Works, reason: String) -> Vec<Told> {
        let at = match self.state {
            State::Running => At::Run,
            State::Opening => At::Resume,
            State::Paused | State::Wanting => return vec![],
        };
        works.close_encoder();
        self.state = State::Wanting;
        vec![Told::NoEncoder { at, reason }]
    }

    /// A frame has left the encoder and has been handed on. The first one
    /// after `resume` is what `resume` is answered with.
    pub fn frame_out(&mut self, encoded_ns: u64, written_ns: u64, key: bool) -> Vec<Told> {
        if self.state != State::Opening {
            return vec![];
        }
        self.state = State::Running;
        vec![Told::Resumed {
            opened_ms: ms(self.asked_ns, self.opened_ns),
            encoded_ms: ms(self.asked_ns, encoded_ns),
            written_ms: ms(self.asked_ns, written_ns),
            key,
        }]
    }

    /// Says who is shown the picture from now on.
    pub fn admit(&mut self, works: &mut impl Works, who: Who) -> Vec<Told> {
        let said = works.admit(&who);
        let device = match who {
            Who::Only { id, .. } if said => Some(id),
            _ => None,
        };
        if !said {
            // What could not be said leaves nobody, never whoever it was before.
            works.admit(&Who::Nobody);
        }
        vec![Told::Admitted { device }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    /// A pipeline that only writes down what it was asked to do. Every call
    /// takes a millisecond on the clock of these tests.
    struct Asked {
        calls: Vec<String>,
        refuses: Option<String>,
        clock: Arc<AtomicU64>,
    }

    impl Asked {
        fn tick(&self) {
            self.clock.fetch_add(1_000_000, Ordering::SeqCst);
        }
    }

    impl Works for Asked {
        fn close_encoder(&mut self) {
            self.tick();
            self.calls.push("close".into());
        }
        fn open_encoder(&mut self) -> Result<(), String> {
            self.tick();
            self.calls.push("open".into());
            self.refuses.clone().map_or(Ok(()), Err)
        }
        fn offer(&mut self, offered: bool) {
            self.calls.push(format!("offer {offered}"));
        }
        fn admit(&mut self, who: &Who) -> bool {
            match who {
                Who::Only { id, .. } if id == "bad" => false,
                Who::Only { id, .. } => {
                    self.calls.push(format!("admit {id}"));
                    true
                }
                Who::Nobody => {
                    self.calls.push("admit nobody".into());
                    true
                }
                Who::Paired => {
                    self.calls.push("admit paired".into());
                    true
                }
            }
        }
    }

    fn started(how: Result<(), String>) -> (Picture, Vec<Told>, Asked) {
        let clock = Arc::new(AtomicU64::new(1_000_000_000));
        let asked = Asked {
            calls: vec![],
            refuses: None,
            clock: clock.clone(),
        };
        let (picture, told) = Picture::begin(how, move || clock.load(Ordering::SeqCst));
        (picture, told, asked)
    }

    fn no_encoder(at: At) -> Vec<Told> {
        vec![Told::NoEncoder {
            at,
            reason: "all sessions are taken".into(),
        }]
    }

    #[test]
    fn pause_closes_the_encoder_and_says_how_long_that_took() {
        let (mut picture, told, mut works) = started(Ok(()));
        assert!(told.is_empty());
        assert_eq!(picture.state(), State::Running);
        assert_eq!(
            picture.pause(&mut works),
            [Told::Paused {
                offered: true,
                took_ms: 1.0
            }]
        );
        assert_eq!(works.calls, ["close"]);
        assert_eq!(picture.state(), State::Paused);
    }

    #[test]
    fn pause_said_twice_is_answered_twice_and_closes_once() {
        let (mut picture, _, mut works) = started(Ok(()));
        picture.pause(&mut works);
        assert_eq!(
            picture.pause(&mut works),
            [Told::Paused {
                offered: true,
                took_ms: 0.0
            }]
        );
        assert_eq!(works.calls, ["close"]);
    }

    #[test]
    fn detach_cuts_the_viewers_off_before_the_encoder_is_closed() {
        let (mut picture, _, mut works) = started(Ok(()));
        assert_eq!(
            picture.detach(&mut works),
            [Told::Paused {
                offered: false,
                took_ms: 1.0
            }]
        );
        assert_eq!(works.calls, ["offer false", "close"]);
        // A pause that follows leaves the picture withheld.
        assert_eq!(
            picture.pause(&mut works),
            [Told::Paused {
                offered: false,
                took_ms: 0.0
            }]
        );
        assert!(!picture.offered());
    }

    #[test]
    fn resume_opens_an_encoder_and_is_answered_when_its_first_frame_is_out() {
        let (mut picture, _, mut works) = started(Ok(()));
        picture.detach(&mut works);
        works.calls.clear();
        let asked = works.clock.load(Ordering::SeqCst);
        assert!(picture.resume(&mut works).is_empty());
        assert_eq!(works.calls, ["offer true", "open"]);
        assert_eq!(picture.state(), State::Opening);
        assert!(picture.offered());
        let told = picture.frame_out(asked + 30_000_000, asked + 30_500_000, true);
        assert_eq!(
            told,
            [Told::Resumed {
                opened_ms: 1.0,
                encoded_ms: 30.0,
                written_ms: 30.5,
                key: true
            }]
        );
        assert_eq!(picture.state(), State::Running);
        // Later frames are only frames.
        assert!(picture
            .frame_out(asked + 40_000_000, asked + 41_000_000, false)
            .is_empty());
    }

    #[test]
    fn resume_while_the_picture_runs_changes_nothing_but_offers_it() {
        let (mut picture, _, mut works) = started(Ok(()));
        assert!(picture.resume(&mut works).is_empty());
        assert_eq!(works.calls, ["offer true"]);
        assert_eq!(picture.state(), State::Running);
    }

    #[test]
    fn an_encoder_refused_at_the_start_leaves_a_picture_that_waits_for_one() {
        let (mut picture, told, mut works) = started(Err("all sessions are taken".into()));
        assert_eq!(told, no_encoder(At::Start));
        assert_eq!(picture.state(), State::Wanting);
        // And `resume` tries again.
        assert!(picture.resume(&mut works).is_empty());
        assert_eq!(works.calls, ["offer true", "open"]);
        assert_eq!(picture.state(), State::Opening);
    }

    #[test]
    fn an_encoder_refused_on_the_way_back_leaves_the_picture_stopped_and_can_be_tried_again() {
        let (mut picture, _, mut works) = started(Ok(()));
        picture.pause(&mut works);
        works.refuses = Some("all sessions are taken".into());
        assert_eq!(picture.resume(&mut works), no_encoder(At::Resume));
        assert_eq!(picture.state(), State::Wanting);
        assert_eq!(picture.resume(&mut works), no_encoder(At::Resume));
        works.refuses = None;
        assert!(picture.resume(&mut works).is_empty());
        assert_eq!(picture.state(), State::Opening);
    }

    #[test]
    fn an_encoder_that_fails_while_the_picture_runs_is_taken_out_and_nothing_ends() {
        let (mut picture, _, mut works) = started(Ok(()));
        assert_eq!(
            picture.encoder_failed(&mut works, "all sessions are taken".into()),
            no_encoder(At::Run)
        );
        assert_eq!(works.calls, ["close"]);
        assert_eq!(picture.state(), State::Wanting);
        // A second report of the same failure says nothing more.
        assert!(picture
            .encoder_failed(&mut works, "again".into())
            .is_empty());
        assert_eq!(works.calls, ["close"]);
    }

    #[test]
    fn an_encoder_that_fails_before_its_first_frame_counts_as_refused_on_the_way_back() {
        let (mut picture, _, mut works) = started(Ok(()));
        picture.pause(&mut works);
        picture.resume(&mut works);
        assert_eq!(
            picture.encoder_failed(&mut works, "all sessions are taken".into()),
            no_encoder(At::Resume)
        );
        // No answer to `resume` comes after that.
        assert!(picture.frame_out(0, 0, true).is_empty());
    }

    #[test]
    fn a_failure_that_arrives_after_the_pause_changes_nothing() {
        let (mut picture, _, mut works) = started(Ok(()));
        picture.pause(&mut works);
        works.calls.clear();
        assert!(picture.encoder_failed(&mut works, "late".into()).is_empty());
        assert!(works.calls.is_empty());
        assert_eq!(picture.state(), State::Paused);
    }

    #[test]
    fn pause_ends_the_waiting_for_an_encoder() {
        let (mut picture, _, mut works) = started(Err("none".into()));
        assert_eq!(
            picture.pause(&mut works),
            [Told::Paused {
                offered: true,
                took_ms: 0.0
            }]
        );
        assert!(works.calls.is_empty());
        assert_eq!(picture.state(), State::Paused);
    }

    #[test]
    fn pause_before_the_first_frame_of_a_new_encoder_closes_it_and_resume_is_not_answered() {
        let (mut picture, _, mut works) = started(Ok(()));
        picture.pause(&mut works);
        picture.resume(&mut works);
        works.calls.clear();
        picture.pause(&mut works);
        assert_eq!(works.calls, ["close"]);
        assert!(picture.frame_out(0, 0, true).is_empty());
    }

    #[test]
    fn the_device_that_is_shown_the_picture_is_named_or_nobody_is() {
        let (mut picture, _, mut works) = started(Ok(()));
        let device = |id: &str| Who::Only {
            id: id.into(),
            public_key: "key".into(),
        };
        assert_eq!(
            picture.admit(&mut works, device("1a2b")),
            [Told::Admitted {
                device: Some("1a2b".into())
            }]
        );
        assert_eq!(
            picture.admit(&mut works, Who::Nobody),
            [Told::Admitted { device: None }]
        );
        // A device that cannot be named leaves nobody, not the one before.
        assert_eq!(
            picture.admit(&mut works, device("bad")),
            [Told::Admitted { device: None }]
        );
        assert_eq!(works.calls, ["admit 1a2b", "admit nobody", "admit nobody"]);
    }
}
