//! A viewer that lasts: when its connection is cut, it connects again.
//!
//! A connection can end while both ends are alive. A network may stop
//! delivering on it without a word (macOS does so when an application is
//! newly registered), a laptop sleeps, a wireless network hands it to
//! another base station. The picture then comes back by itself: the viewer
//! opens a new connection in the ordinary way and asks for the picture
//! again. Nothing of the channel's procedure changes for it (ADR 0015).
//!
//! It only does so when nobody said why the connection ended
//! ([`Error::again`]). A host that says that the stream is over, that the
//! picture is for another device now, or that this device may not look, is
//! not asked again.
//!
//! Connecting again is done with tries that overlap
//! ([`istesso_link::again`], ADR 0007): a host that does not answer makes
//! each try wait its deadline out, and the next try does not wait for
//! that. A try connects and runs the handshake ([`Viewer::open`]). The
//! request for the picture goes on the first connection that gets that far,
//! and on no other: the host closes the connection the device had before,
//! and asks its encoder for a key frame, only when that request arrives.
//! The numbers are the same as for input and for sound.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use istesso_link::{again, Pace, Racer, Turn};

use crate::viewer::{Asker, Event, Opened, Viewer};
use crate::wire::StreamInfo;
use crate::{Again, Error, Result};

/// How soon after it was made a connection has to be cut for the viewer
/// not to connect again at once ([`Pace::short_lived`]).
///
/// (It was also how long after a failed first attempt the next one was
/// made. That goes by the steps of [`First`] now.)
pub const RETRY: Duration = Duration::from_millis(250);
/// Before there has ever been a connection: how long after a failed
/// attempt the next one is made, once the viewer has been trying for
/// [`SOON_FOR`], and whenever the host said that it cannot take this
/// device at the moment.
pub const RETRY_SLOWLY: Duration = Duration::from_secs(1);
/// For how long the first attempts follow each other closely.
pub const SOON_FOR: Duration = Duration::from_secs(10);
/// What connecting, the handshake and the host's answer may take together,
/// the first time. When connecting again, a try has
/// [`Pace::try_within`] for connecting and the handshake, and the answer
/// [`Pace::answer_within`].
pub const FIRST_WITHIN: Duration = Duration::from_secs(3);

/// When to make the next attempt at a first connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    At(Instant),
    Never,
}

/// Decides when a viewer that has not had a connection yet makes its next
/// attempt at one. These attempts are made one after the other, as they
/// always were; tries that overlap are for connecting again after a cut.
/// It holds no connection and reads no clock of its own.
///
/// The attempts go by the steps the tries after a cut go by (ADR 0007):
/// counted from when the first attempt began, one after each of
/// [`Pace::early`] and then one every [`Pace::quick`]. Right after an
/// application was built anew, macOS drops the first connections it makes
/// for a moment (about 13 ms at worst, as far as was seen): the attempt at
/// the second early step is past that. This is for attempts that fail at
/// once. One that hangs takes [`FIRST_WITHIN`], every step has passed by
/// then, and the next attempt simply follows it at once: with one attempt
/// at a time the steps mean nothing there.
#[derive(Debug, Default)]
pub struct First {
    /// Since when attempts have been failing.
    without_since: Option<Instant>,
    /// When the first attempt began: the steps are counted from here.
    first_began: Option<Instant>,
}

impl First {
    /// The attempt that began at `began` has failed at `now`; `what` is
    /// what its error asks for.
    pub fn after(&mut self, what: Again, began: Instant, now: Instant) -> Next {
        let first = *self.first_began.get_or_insert(began);
        match what {
            Again::Never => Next::Never,
            Again::Slowly => {
                self.without_since.get_or_insert(now);
                Next::At(now + RETRY_SLOWLY)
            }
            Again::Soon => {
                let since = *self.without_since.get_or_insert(now);
                if now.saturating_duration_since(since) >= SOON_FOR {
                    return Next::At(now + RETRY_SLOWLY);
                }
                // The step after the one this attempt began at: now, if its
                // moment passed while the attempt was under way. One attempt
                // for every step at most, and never two at once.
                Next::At(
                    step_after(began.saturating_duration_since(first))
                        .map_or(now, |step| (first + step).max(now)),
                )
            }
        }
    }
}

/// The first of the steps that lies later than `began` after the first
/// attempt: the early ones, and then one every [`Pace::quick`]. Nothing
/// where that cannot be said (a pace without a beat).
fn step_after(began: Duration) -> Option<Duration> {
    let pace = Pace::default();
    let beats = began.as_nanos().checked_div(pace.quick.as_nanos())? + 1;
    let beat = pace.quick.checked_mul(u32::try_from(beats).ok()?)?;
    let early = pace.early.iter().copied().find(|step| *step > began);
    Some(early.map_or(beat, |early| early.min(beat)))
}

/// Opens a connection to the host, to the device with this id if one is
/// given: connecting and the handshake, within this time, all of it
/// together ([`Viewer::open`]). The picture is not asked for here. When
/// the viewer connects again, this is one try, and several run at once,
/// each on a thread of its own: each makes its own keys and reads the list
/// of paired devices for itself.
pub type Open = Arc<dyn Fn(Option<&str>, Duration) -> Result<Opened> + Send + Sync>;

/// What a lasting viewer hands over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step<'a> {
    /// What the host sent.
    Event(Event<'a>),
    /// The connection has ended and nobody said why. It is being made
    /// again; what is on the screen is not the host's picture of this
    /// moment any more. Said once for each cut.
    Cut,
    /// There is a connection: the first one, or one again after a cut.
    /// [`Lasting::info`] says what the host sends now, and the first frame
    /// that follows is a key frame.
    Connected {
        again: bool,
        /// How long ago the host was last heard on the connection before
        /// this one. Nothing for the first connection.
        silent_for: Duration,
    },
}

/// Asks the host of a [`Lasting`] viewer for a key frame, from any thread,
/// whichever connection it has at the moment.
#[derive(Clone)]
pub struct LastingAsker(Arc<Mutex<Option<Asker>>>);

impl LastingAsker {
    /// As [`Asker::ask_for_key_frame`]. Without a connection nothing is
    /// asked: the next one begins with a key frame in any case.
    pub fn ask_for_key_frame(&self) {
        if let Some(asker) = &*self.0.lock().unwrap_or_else(|e| e.into_inner()) {
            asker.ask_for_key_frame();
        }
    }
}

/// A viewer that connects again when its connection is cut.
pub struct Lasting {
    open: Open,
    /// What one try at connecting again does: `open`, to the device this
    /// viewer began with. There once there has been a connection.
    again: Option<again::Open<Opened, Error>>,
    viewer: Option<Viewer>,
    /// The device this viewer began with (its id). It is the only one a
    /// later connection is made to: another paired device that answers at
    /// the same address is not the same picture.
    peer: Option<String>,
    /// The attempts at a first connection, and when the next is due.
    /// `None`: now.
    first: First,
    due: Option<Instant>,
    /// The tries at connecting again, and how often the connection was
    /// cut.
    racer: Racer<Opened, Error>,
    /// When the host was last heard on the connection that was cut.
    heard: Option<Instant>,
    ever: bool,
    /// It is over, and was said to be: nothing is tried again.
    over: bool,
    asker: Arc<Mutex<Option<Asker>>>,
}

impl Lasting {
    /// A viewer that connects with `open` when it is first asked for
    /// something. `peer` names the one device it may be shown the picture
    /// by (its id); without it, the device that answers first is the one.
    pub fn new(open: Open, peer: Option<String>) -> Self {
        Lasting {
            open,
            again: None,
            viewer: None,
            peer,
            first: First::default(),
            due: None,
            racer: Racer::new(Pace::default()),
            heard: None,
            ever: false,
            over: false,
            asker: Arc::new(Mutex::new(None)),
        }
    }

    /// The same viewer with other numbers for its tries at connecting
    /// again. For tests, which cannot wait two seconds for every try that
    /// gets no answer. To be said before the viewer is used.
    pub fn paced(mut self, pace: Pace) -> Self {
        self.racer = Racer::new(pace);
        self
    }

    /// What the host said about its picture on the connection there is.
    pub fn info(&self) -> Option<&StreamInfo> {
        self.viewer.as_ref().map(Viewer::info)
    }

    /// The viewer of the connection there is.
    pub fn viewer(&self) -> Option<&Viewer> {
        self.viewer.as_ref()
    }

    /// How often the connection was cut and taken up again, or is being.
    /// One cut is one, however many tries it took to come back.
    pub fn cuts(&self) -> u64 {
        self.racer.tries().cuts()
    }

    /// How many tries at connecting again were started, over all cuts.
    pub fn tries(&self) -> u64 {
        self.racer.tries().started()
    }

    /// A handle for asking for a key frame from another thread.
    pub fn asker(&self) -> LastingAsker {
        LastingAsker(self.asker.clone())
    }

    /// Waits for the next thing, for `wait` at most (the first connection
    /// may take longer, [`FIRST_WITHIN`], and so may the host's answer when
    /// connecting again, [`Pace::answer_within`]). `Ok(None)` means that
    /// nothing came in that time. An error means that it is over and is
    /// not tried again: the host said so or refused this device. Whatever
    /// else ends a connection is answered with [`Step::Cut`], and then with
    /// a new connection.
    pub fn next(&mut self, wait: Duration) -> Result<Option<Step<'_>>> {
        if self.over {
            return Err(Error::Ended);
        }
        if self.viewer.is_none() {
            return if self.ever {
                self.connect_again(wait)
            } else {
                self.connect_first(wait)
            };
        }
        // Tries that were dropped when the connection came back end later;
        // what they bring is closed here.
        if let Some(again) = &self.again {
            let _ = self.racer.turn(Duration::ZERO, again, Error::again);
        }
        let Some(viewer) = self.viewer.as_mut() else {
            return Ok(None);
        };
        match viewer.pull_for(wait) {
            Ok(Some(pulled)) => Ok(self
                .viewer
                .as_ref()
                .and_then(|viewer| viewer.event_of(pulled))
                .map(Step::Event)),
            Ok(None) => Ok(None),
            Err(err) => {
                let heard = viewer.last_heard();
                self.viewer = None;
                *self.asker.lock().unwrap_or_else(|e| e.into_inner()) = None;
                match err.again() {
                    Again::Never => {
                        self.over = true;
                        Err(err)
                    }
                    Again::Soon => {
                        // The first try at once, unless the connection had only
                        // just been made.
                        self.racer.tries_mut().cut(Instant::now());
                        self.heard = Some(heard);
                        Ok(Some(Step::Cut))
                    }
                    Again::Slowly => {
                        // No error of a stream that runs asks for leisure today
                        // (a host that is full says so in answer to the request,
                        // and a list of paired devices is read when connecting).
                        // Should one come to, the pause it asks for is kept, as
                        // it was before tries overlapped.
                        self.racer.tries_mut().cut_at_leisure(Instant::now());
                        self.heard = Some(heard);
                        Ok(Some(Step::Cut))
                    }
                }
            }
        }
    }

    /// The first connection: one attempt after the other, each with the
    /// request for the picture and its answer in it, as it always was.
    /// When the next one is made after a failure is for [`First`] to say.
    fn connect_first(&mut self, wait: Duration) -> Result<Option<Step<'_>>> {
        if let Some(due) = self.due {
            let now = Instant::now();
            if due > now {
                std::thread::sleep(wait.min(due - now));
                if Instant::now() < due {
                    return Ok(None);
                }
            }
        }
        let began = Instant::now();
        match (self.open)(self.peer.as_deref(), FIRST_WITHIN).and_then(Opened::watch) {
            Ok(viewer) => {
                self.ever = true;
                // Later connections go to this device and to no other.
                let peer = self
                    .peer
                    .get_or_insert_with(|| viewer.peer().id.clone())
                    .clone();
                let open = self.open.clone();
                self.again = Some(Arc::new(move |within| open(Some(&peer), within)));
                *self.asker.lock().unwrap_or_else(|e| e.into_inner()) = Some(viewer.asker());
                self.viewer = Some(viewer);
                self.racer.tries_mut().made(Instant::now());
                self.due = None;
                Ok(Some(Step::Connected {
                    again: false,
                    silent_for: Duration::ZERO,
                }))
            }
            Err(err) => match self.first.after(err.again(), began, Instant::now()) {
                Next::Never => {
                    self.over = true;
                    Err(err)
                }
                Next::At(due) => {
                    self.due = Some(due);
                    Ok(None)
                }
            },
        }
    }

    /// Connecting again: starts the tries that are due, waits for the
    /// first whose handshake completes, and asks for the picture on that
    /// connection.
    fn connect_again(&mut self, wait: Duration) -> Result<Option<Step<'_>>> {
        let Some(again) = self.again.clone() else {
            return Ok(None);
        };
        // A connection that the host has closed since its handshake (it holds
        // only so many of a device that have not asked for the picture yet) is
        // not one that was opened: it counts as a try that failed.
        let still_open = |opened: &Opened| !opened.closed_by_host();
        let turn = self.racer.turn_if(wait, &again, Error::again, still_open);
        let opened = match turn {
            Turn::Nothing => return Ok(None),
            Turn::Stopped(err) => {
                self.over = true;
                return Err(err);
            }
            Turn::Opened(opened) => opened,
        };
        // This is the connection that is used: the request goes on it and on
        // no other. The host closes the one before, and makes a key frame,
        // when it arrives.
        let within = self.racer.tries().pace().answer_within;
        match opened.watch_within(within) {
            Ok(viewer) => {
                *self.asker.lock().unwrap_or_else(|e| e.into_inner()) = Some(viewer.asker());
                let silent_for = self
                    .heard
                    .take()
                    .map(|heard| heard.elapsed())
                    .unwrap_or_default();
                self.viewer = Some(viewer);
                self.racer.tries_mut().answered(Instant::now());
                Ok(Some(Step::Connected {
                    again: true,
                    silent_for,
                }))
            }
            Err(err) => {
                // Refused for good, refused for the moment, or not answered: it
                // is over, or the tries go on. It is still the same cut.
                let what = err.again();
                self.racer.tries_mut().unanswered(what, Instant::now());
                if what == Again::Never {
                    self.over = true;
                    return Err(err);
                }
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// (Until the early steps: made again at once, and then four times a
    /// second.)
    #[test]
    fn a_first_attempt_that_fails_is_made_again_10_ms_and_50_ms_later_and_then_ten_times_a_second()
    {
        let mut first = First::default();
        let began = Instant::now();
        // Each fails in the moment it began.
        let after =
            |first: &mut First, at: u64| first.after(Again::Soon, began + ms(at), began + ms(at));
        assert_eq!(after(&mut first, 0), Next::At(began + ms(10)));
        assert_eq!(after(&mut first, 10), Next::At(began + ms(50)));
        // That failed as well: a tenth of a second, each time, counted from
        // the first.
        assert_eq!(after(&mut first, 50), Next::At(began + ms(100)));
        assert_eq!(after(&mut first, 100), Next::At(began + ms(200)));
        assert_eq!(after(&mut first, 9_000), Next::At(began + ms(9_100)));
    }

    /// What the steps are for: attempts that fail at once for the first
    /// 13 ms after a start (the worst that was seen of macOS right after an
    /// application was built anew) and pass from then on. The attempt at
    /// 50 ms is the one that connects, and it is the third.
    #[test]
    fn a_start_whose_first_13_ms_fail_is_connected_by_the_attempt_at_50_ms() {
        // The attempts of a start whose attempts fail, a millisecond after
        // they began, for this long, and for a second at most: when each
        // began, and whether the last of them passed.
        let attempts = |fails_for: Duration| {
            let mut first = First::default();
            let base = Instant::now();
            let (mut at, mut began) = (Duration::ZERO, Vec::new());
            while at < Duration::from_secs(1) {
                began.push(at.as_millis());
                if at >= fails_for {
                    return (began, true);
                }
                let failed = base + at + ms(1);
                let Next::At(due) = first.after(Again::Soon, base + at, failed) else {
                    panic!("given up");
                };
                assert!(due >= failed, "an attempt before the last has ended");
                at = due - base;
            }
            (began, false)
        };
        assert_eq!(attempts(ms(13)), (vec![0, 10, 50], true));
        // The usual moment (about 2 ms): the attempt at 10 ms.
        assert_eq!(attempts(ms(2)), (vec![0, 10], true));
        // Just after an early step: the next one, and after the second, the
        // tenth of a second.
        assert_eq!(attempts(ms(11)), (vec![0, 10, 50], true));
        assert_eq!(attempts(ms(51)), (vec![0, 10, 50, 100], true));
        // Failing on: twelve attempts in the first second.
        let (began, passed) = attempts(Duration::from_secs(5));
        assert!(!passed);
        assert_eq!(began[..5], [0, 10, 50, 100, 200]);
        assert_eq!(began.len(), 12);
    }

    /// One attempt at a time: an attempt that hangs until its time is up
    /// is followed at once, and the steps mean nothing. An attempt that
    /// takes a while is followed at the next step that has not passed.
    #[test]
    fn an_attempt_that_took_long_is_followed_at_once_and_never_by_two() {
        let mut first = First::default();
        let began = Instant::now();
        let failed = began + FIRST_WITHIN;
        assert_eq!(first.after(Again::Soon, began, failed), Next::At(failed));
        let failed_again = failed + FIRST_WITHIN;
        assert_eq!(
            first.after(Again::Soon, failed, failed_again),
            Next::At(failed_again)
        );

        // Thirty milliseconds each: at once for the step at 10 ms, at once
        // for the one at 50 ms, and then the one at 100 ms is waited for.
        let mut first = First::default();
        let at = |n: u64| began + ms(n);
        assert_eq!(first.after(Again::Soon, at(0), at(30)), Next::At(at(30)));
        assert_eq!(first.after(Again::Soon, at(30), at(60)), Next::At(at(60)));
        assert_eq!(first.after(Again::Soon, at(60), at(90)), Next::At(at(100)));
    }

    #[test]
    fn a_host_that_was_never_there_is_asked_once_a_second() {
        let mut first = First::default();
        let began = Instant::now();
        assert_eq!(
            first.after(Again::Soon, began, began),
            Next::At(began + ms(10))
        );
        let later = began + SOON_FOR;
        assert_eq!(
            first.after(Again::Soon, later, later),
            Next::At(later + RETRY_SLOWLY)
        );
    }

    #[test]
    fn a_host_that_is_full_is_asked_at_leisure_and_one_that_refuses_never() {
        let mut first = First::default();
        let now = Instant::now();
        assert_eq!(
            first.after(Again::Slowly, now, now),
            Next::At(now + RETRY_SLOWLY)
        );
        assert_eq!(first.after(Again::Never, now, now), Next::Never);
    }

    #[test]
    fn a_connection_that_does_not_last_is_not_made_again_at_once() {
        // The quarter of a second of this module is the one the tries go by.
        assert_eq!(Pace::default().short_lived, RETRY);
    }

    #[test]
    fn only_a_connection_that_ended_without_a_word_is_taken_up_again() {
        use istesso_link::Error as Link;
        for err in [
            Error::Silent,
            Error::Malformed,
            Error::Link(Link::Closed),
            Error::Link(Link::Timeout),
            Error::Link(Link::Refused),
            Error::Link(Link::Crypto("altered".into())),
            Error::Io(std::io::ErrorKind::ConnectionReset.into()),
        ] {
            assert_eq!(err.again(), Again::Soon, "{err}");
        }
        assert_eq!(Error::Busy.again(), Again::Slowly);
        for err in [
            Error::Ended,
            Error::NotShown,
            Error::SetAside,
            Error::Forbidden,
            Error::Refused("something a later version says".into()),
            Error::Unsupported("another encoding".into()),
            Error::NoIdentity,
            Error::Link(Link::NotTrusted),
            Error::Link(Link::UnknownPeer),
            Error::Link(Link::WrongPeer {
                expected: "one".into(),
                found: "another".into(),
            }),
        ] {
            assert_eq!(err.again(), Again::Never, "{err}");
        }
    }
}
