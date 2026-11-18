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

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::viewer::{Asker, Event, Viewer};
use crate::wire::StreamInfo;
use crate::{Again, Error, Result};

/// How long after a failed attempt the next one is made, for as long as
/// the cut is fresh. The same quarter of a second as for sound.
pub const RETRY: Duration = Duration::from_millis(250);
/// The same, once the host has been away for [`SOON_FOR`], and whenever
/// the host said that it cannot take this device at the moment.
pub const RETRY_SLOWLY: Duration = Duration::from_secs(1);
/// For how long after a cut the attempts follow each other closely. A host
/// that is switched off is not knocked at four times a second for hours.
pub const SOON_FOR: Duration = Duration::from_secs(10);
/// What connecting, the handshake and the host's answer may take together,
/// the first time.
pub const FIRST_WITHIN: Duration = Duration::from_secs(3);
/// The same, when connecting again.
pub const AGAIN_WITHIN: Duration = Duration::from_secs(1);

/// When to connect again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    At(Instant),
    Never,
}

/// Decides when a viewer connects again. It holds no connection and reads
/// no clock of its own.
#[derive(Debug, Default)]
pub struct Pace {
    /// Since when there has been no connection.
    without_since: Option<Instant>,
    /// When the last connection was made.
    connected_at: Option<Instant>,
}

impl Pace {
    /// A connection has been made.
    pub fn connected(&mut self, now: Instant) {
        self.without_since = None;
        self.connected_at = Some(now);
    }

    /// A connection has ended, or could not be made, at `now`; `what` is
    /// what its error asks for.
    pub fn after(&mut self, what: Again, now: Instant) -> Next {
        match what {
            Again::Never => Next::Never,
            Again::Slowly => {
                self.without_since.get_or_insert(now);
                Next::At(now + RETRY_SLOWLY)
            }
            Again::Soon => match self.without_since {
                // Cut just now: at once. Unless the connection had only just been
                // made: a host that takes a device and lets go of it straight away
                // is not asked again faster than any other.
                None => {
                    self.without_since = Some(now);
                    let short_lived = self
                        .connected_at
                        .is_some_and(|at| now.saturating_duration_since(at) < RETRY);
                    Next::At(if short_lived { now + RETRY } else { now })
                }
                Some(since) if now.saturating_duration_since(since) < SOON_FOR => {
                    Next::At(now + RETRY)
                }
                Some(_) => Next::At(now + RETRY_SLOWLY),
            },
        }
    }
}

/// Makes a connection to the host: to the device with this id if one is
/// given, within this time.
pub type Connect = Box<dyn FnMut(Option<&str>, Duration) -> Result<Viewer> + Send>;

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
    connect: Connect,
    viewer: Option<Viewer>,
    /// The device this viewer began with (its id). It is the only one a
    /// later connection is made to: another paired device that answers at
    /// the same address is not the same picture.
    peer: Option<String>,
    pace: Pace,
    /// When the next attempt is due. `None`: now.
    due: Option<Instant>,
    /// When the host was last heard on the connection that was cut.
    heard: Option<Instant>,
    ever: bool,
    cuts: u64,
    /// It is over, and was said to be: nothing is tried again.
    over: bool,
    asker: Arc<Mutex<Option<Asker>>>,
}

impl Lasting {
    /// A viewer that connects with `connect` when it is first asked for
    /// something. `peer` names the one device it may be shown the picture
    /// by (its id); without it, the device that answers first is the one.
    pub fn new(connect: Connect, peer: Option<String>) -> Self {
        Lasting {
            connect,
            viewer: None,
            peer,
            pace: Pace::default(),
            due: None,
            heard: None,
            ever: false,
            cuts: 0,
            over: false,
            asker: Arc::new(Mutex::new(None)),
        }
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
    pub fn cuts(&self) -> u64 {
        self.cuts
    }

    /// A handle for asking for a key frame from another thread.
    pub fn asker(&self) -> LastingAsker {
        LastingAsker(self.asker.clone())
    }

    /// Waits for the next thing, for `wait` at most (an attempt to connect
    /// may take longer: [`FIRST_WITHIN`], [`AGAIN_WITHIN`]). `Ok(None)`
    /// means that nothing came in that time. An error means that it is
    /// over and is not tried again: the host said so or refused this
    /// device. Whatever else ends a connection is answered with
    /// [`Step::Cut`], and then with a new connection.
    pub fn next(&mut self, wait: Duration) -> Result<Option<Step<'_>>> {
        if self.over {
            return Err(Error::Ended);
        }
        let Some(viewer) = self.viewer.as_mut() else {
            return self.connect_when_due(wait);
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
                match self.pace.after(err.again(), Instant::now()) {
                    Next::Never => {
                        self.over = true;
                        Err(err)
                    }
                    Next::At(due) => {
                        self.due = Some(due);
                        self.heard = Some(heard);
                        self.cuts += 1;
                        Ok(Some(Step::Cut))
                    }
                }
            }
        }
    }

    fn connect_when_due(&mut self, wait: Duration) -> Result<Option<Step<'_>>> {
        if let Some(due) = self.due {
            let now = Instant::now();
            if due > now {
                std::thread::sleep(wait.min(due - now));
                if Instant::now() < due {
                    return Ok(None);
                }
            }
        }
        let within = if self.ever {
            AGAIN_WITHIN
        } else {
            FIRST_WITHIN
        };
        match (self.connect)(self.peer.as_deref(), within) {
            Ok(viewer) => {
                let again = self.ever;
                self.ever = true;
                if self.peer.is_none() {
                    self.peer = Some(viewer.peer().id.clone());
                }
                *self.asker.lock().unwrap_or_else(|e| e.into_inner()) = Some(viewer.asker());
                let silent_for = self
                    .heard
                    .take()
                    .map(|heard| heard.elapsed())
                    .unwrap_or_default();
                self.viewer = Some(viewer);
                self.pace.connected(Instant::now());
                self.due = None;
                Ok(Some(Step::Connected { again, silent_for }))
            }
            Err(err) => match self.pace.after(err.again(), Instant::now()) {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_is_answered_at_once_and_then_four_times_a_second() {
        let mut pace = Pace::default();
        let began = Instant::now();
        pace.connected(began);
        let cut = began + Duration::from_secs(60);
        assert_eq!(pace.after(Again::Soon, cut), Next::At(cut));
        // The attempt failed: a quarter of a second, each time.
        assert_eq!(pace.after(Again::Soon, cut), Next::At(cut + RETRY));
        let later = cut + Duration::from_secs(9);
        assert_eq!(pace.after(Again::Soon, later), Next::At(later + RETRY));
    }

    #[test]
    fn a_host_that_stays_away_is_asked_once_a_second() {
        let mut pace = Pace::default();
        let cut = Instant::now();
        assert_eq!(pace.after(Again::Soon, cut), Next::At(cut));
        let later = cut + SOON_FOR;
        assert_eq!(
            pace.after(Again::Soon, later),
            Next::At(later + RETRY_SLOWLY)
        );
        // A connection, and the next cut is answered at once again.
        let back = later + Duration::from_secs(5);
        pace.connected(back);
        let cut = back + Duration::from_secs(5);
        assert_eq!(pace.after(Again::Soon, cut), Next::At(cut));
    }

    #[test]
    fn a_connection_that_does_not_last_is_not_made_again_at_once() {
        let mut pace = Pace::default();
        let made = Instant::now();
        pace.connected(made);
        let cut = made + Duration::from_millis(20);
        assert_eq!(pace.after(Again::Soon, cut), Next::At(cut + RETRY));
    }

    #[test]
    fn a_host_that_is_full_is_asked_at_leisure_and_one_that_refuses_never() {
        let mut pace = Pace::default();
        let now = Instant::now();
        assert_eq!(pace.after(Again::Slowly, now), Next::At(now + RETRY_SLOWLY));
        assert_eq!(pace.after(Again::Never, now), Next::Never);
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
