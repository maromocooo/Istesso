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

/// Before there has ever been a connection: how long after a failed
/// attempt the next one is made, for as long as the viewer has not been
/// trying for long.
///
/// Also how soon after it was made a connection has to be cut for the
/// viewer not to connect again at once ([`Pace::short_lived`]).
pub const RETRY: Duration = Duration::from_millis(250);
/// The same, once the viewer has been trying for [`SOON_FOR`], and
/// whenever the host said that it cannot take this device at the moment.
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
#[derive(Debug, Default)]
pub struct First {
    /// Since when attempts have been failing.
    without_since: Option<Instant>,
}

impl First {
    /// An attempt has failed at `now`; `what` is what its error asks for.
    pub fn after(&mut self, what: Again, now: Instant) -> Next {
        match what {
            Again::Never => Next::Never,
            Again::Slowly => {
                self.without_since.get_or_insert(now);
                Next::At(now + RETRY_SLOWLY)
            }
            Again::Soon => match self.without_since {
                // The first failure: once more at once.
                None => {
                    self.without_since = Some(now);
                    Next::At(now)
                }
                Some(since) if now.saturating_duration_since(since) < SOON_FOR => {
                    Next::At(now + RETRY)
                }
                Some(_) => Next::At(now + RETRY_SLOWLY),
            },
        }
    }
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
                    Again::Soon | Again::Slowly => {
                        // The first try at once, unless the connection had only
                        // just been made.
                        self.racer.tries_mut().cut(Instant::now());
                        self.heard = Some(heard);
                        Ok(Some(Step::Cut))
                    }
                }
            }
        }
    }

    /// The first connection: one attempt after the other, each with the
    /// request for the picture and its answer in it, as it always was.
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
            Err(err) => match self.first.after(err.again(), Instant::now()) {
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
        let opened = match self.racer.turn(wait, &again, Error::again) {
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

    #[test]
    fn a_first_attempt_that_fails_is_made_again_at_once_and_then_four_times_a_second() {
        let mut first = First::default();
        let began = Instant::now();
        assert_eq!(first.after(Again::Soon, began), Next::At(began));
        // That failed as well: a quarter of a second, each time.
        assert_eq!(first.after(Again::Soon, began), Next::At(began + RETRY));
        let later = began + Duration::from_secs(9);
        assert_eq!(first.after(Again::Soon, later), Next::At(later + RETRY));
    }

    #[test]
    fn a_host_that_was_never_there_is_asked_once_a_second() {
        let mut first = First::default();
        let began = Instant::now();
        assert_eq!(first.after(Again::Soon, began), Next::At(began));
        let later = began + SOON_FOR;
        assert_eq!(
            first.after(Again::Soon, later),
            Next::At(later + RETRY_SLOWLY)
        );
    }

    #[test]
    fn a_host_that_is_full_is_asked_at_leisure_and_one_that_refuses_never() {
        let mut first = First::default();
        let now = Instant::now();
        assert_eq!(
            first.after(Again::Slowly, now),
            Next::At(now + RETRY_SLOWLY)
        );
        assert_eq!(first.after(Again::Never, now), Next::Never);
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
