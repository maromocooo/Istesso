//! Carries the picture of an application from a host to a paired device.
//!
//! A device that wants to look, the *viewer*, connects to the *host* that
//! shows the application, proves who it is with the keys pinned at pairing,
//! and is sent the host's encoded frames as they leave the encoder. Both
//! ends run the handshake of the control channel on their connection
//! (`istesso_link::Channel`, ADR 0007): each end knows which paired device
//! it is talking to, and nobody else can read or alter the picture. No
//! cryptographic step is added here (ADR 0015).
//!
//! * [`wire`]: the first exchange of a connection, and the packet that
//!   carries a frame.
//! * [`host`]: the sending end. It only sends to a paired device that may
//!   look at this host, and looks the device up again while it sends.
//! * [`viewer`]: the receiving end. Hands over whole frames, and asks for a
//!   key frame.
//! * [`rejoin`]: a viewer that connects again when its connection is cut,
//!   and when it does not.
//! * [`clock`]: the clock the times in a frame's record are read from.
//! * [`realtime`]: scheduling for the thread that receives.
//! * `ffi`: the same behind a C interface (`include/istesso_media.h`), for
//!   the host's measuring pipeline and the viewer on the Mac.
//!
//! Encoding and decoding are not done here. The host is handed encoded
//! frames and the viewer hands them on.

#[cfg(unix)]
pub mod clock;
#[cfg(unix)]
mod ffi;
#[cfg(unix)]
pub mod host;
pub mod realtime;
#[cfg(unix)]
pub mod rejoin;
#[cfg(all(test, unix))]
mod session_tests;
#[cfg(unix)]
mod timed;
#[cfg(unix)]
pub mod viewer;
pub mod wire;

#[cfg(unix)]
pub use host::{Admit, Host};
#[cfg(unix)]
pub use rejoin::{Lasting, LastingAsker, Step};
#[cfg(unix)]
pub use viewer::{Asker, Event, Frame, Viewer};
pub use wire::{Chroma, Codec, Colour, Cursor, CursorImage, StreamInfo, DEFAULT_PORT};

/// Receives one line for each thing worth telling the person who runs a
/// host. Never a key, and never an address.
pub type Log = std::sync::Arc<dyn Fn(String) + Send + Sync>;

/// Why an attempt to reach a host failed, in one word that names no address
/// and no device. For a log, and for telling "the host is there but nothing
/// listens" from "there is no way to it from this machine".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Failure {
    /// The machine answered, and nothing listens where it was asked.
    Refused = 1,
    /// There is no way to the machine from here: no route, the network is
    /// down, or this machine does not let the program out onto the network.
    Unreachable = 2,
    /// Connecting, or the first exchange, got no answer in time.
    TimedOut = 3,
    /// The host was reached and greeted, and then said nothing more (in
    /// time): the request was not answered, or the connection went silent
    /// or was closed.
    NoAnswer = 4,
    /// The host answered that it cannot do it right now.
    NotReady = 5,
    /// Anything else.
    Other = 6,
}

impl Failure {
    pub fn word(self) -> &'static str {
        match self {
            Failure::Refused => "refused",
            Failure::Unreachable => "unreachable",
            Failure::TimedOut => "timed_out",
            Failure::NoAnswer => "no_answer",
            Failure::NotReady => "not_ready",
            Failure::Other => "other",
        }
    }

    /// What a socket error says about the way to the other machine.
    pub fn of_io(err: &std::io::Error) -> Self {
        use std::io::ErrorKind;
        #[cfg(unix)]
        if let Some(code) = err.raw_os_error() {
            let no_way = [
                libc::EHOSTUNREACH,
                libc::ENETUNREACH,
                libc::ENETDOWN,
                libc::EHOSTDOWN,
                libc::EADDRNOTAVAIL,
                libc::EACCES,
                libc::EPERM,
            ];
            if no_way.contains(&code) {
                return Failure::Unreachable;
            }
        }
        match err.kind() {
            ErrorKind::ConnectionRefused => Failure::Refused,
            ErrorKind::TimedOut | ErrorKind::WouldBlock => Failure::TimedOut,
            ErrorKind::UnexpectedEof
            | ErrorKind::ConnectionReset
            | ErrorKind::ConnectionAborted
            | ErrorKind::BrokenPipe => Failure::NoAnswer,
            _ => Failure::Other,
        }
    }

    /// What a failure of the control channel's kind of connection says.
    pub fn of_link(err: &istesso_link::Error) -> Self {
        use istesso_link::Error as Link;
        match err {
            Link::Refused => Failure::Refused,
            Link::Timeout => Failure::TimedOut,
            Link::Closed => Failure::NoAnswer,
            Link::Io(err) => Failure::of_io(err),
            _ => Failure::Other,
        }
    }
}

/// Everything that can go wrong between a host and a viewer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Link(#[from] istesso_link::Error),
    #[error(transparent)]
    Pair(#[from] istesso_pair::Error),
    #[error("the other device sent a packet that cannot be read: {0}")]
    Wire(#[from] wire::WireError),
    #[error("the other device sent something that is not part of the picture's protocol")]
    Malformed,
    #[error("this device may not look at that host; it is paired, but without the right to view")]
    Forbidden,
    #[error("the host refused: {0}")]
    Refused(String),
    #[error("the host sends its picture in a form this version cannot show: {0}")]
    Unsupported(String),
    #[error("nothing has come from the host for too long")]
    Silent,
    #[error("the host has stopped sending")]
    Ended,
    #[error("the host shows this picture to another device now, or to none")]
    NotShown,
    #[error("the host has put the picture aside for the time being")]
    SetAside,
    #[error("the host shows its picture to as many devices as it can")]
    Busy,
    #[error("this device has no identity yet; pair it with the other one first")]
    NoIdentity,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// What a viewer does about a connection that has ended, or could not be
/// made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Again {
    /// Connect again at once, and then at short intervals: the connection
    /// was cut and nobody said why. A network may drop a connection that
    /// both ends still believe in.
    Soon,
    /// Connect again, but at leisure: the host said that it cannot take
    /// this device at the moment, or something on this device is in the
    /// way that may pass.
    Slowly,
    /// Do not connect again: the host said that it is over, or refused,
    /// and asking again changes nothing.
    Never,
}

impl Error {
    /// What a viewer does after this error. Only a connection that ended
    /// without a word is taken up again: when the host said why it stops,
    /// or refused this device, the answer would be the same the next time,
    /// and a device that takes a picture back from the one it was moved to
    /// would never stop doing so.
    /// The failure in one word that names no address.
    pub fn failure(&self) -> Failure {
        match self {
            Error::Link(err) => Failure::of_link(err),
            Error::Io(err) => Failure::of_io(err),
            Error::Silent => Failure::NoAnswer,
            Error::Busy => Failure::NotReady,
            _ => Failure::Other,
        }
    }

    pub fn again(&self) -> Again {
        use istesso_link::Error as Link;
        match self {
            // Not paired any more, on either side, or another device than
            // the one this viewer began with.
            Error::Link(Link::UnknownPeer | Link::NotTrusted | Link::WrongPeer { .. }) => {
                Again::Never
            }
            // Another version of the control channel altogether.
            Error::Link(Link::Version { .. }) => Again::Never,
            Error::Forbidden
            | Error::Refused(_)
            | Error::Unsupported(_)
            | Error::Ended
            | Error::NotShown
            | Error::SetAside
            | Error::NoIdentity => Again::Never,
            Error::Busy | Error::Pair(_) => Again::Slowly,
            // Everything else is a connection that failed, fell silent, or
            // carried something that could not be read.
            Error::Link(_) | Error::Wire(_) | Error::Malformed | Error::Silent | Error::Io(_) => {
                Again::Soon
            }
        }
    }
}

#[cfg(all(test, unix))]
pub(crate) mod testing {
    //! Helpers shared by the tests of this crate.

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use istesso_pair::{Hello, Identity, Peer, PeerStore, Permission};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A list of paired devices in its own temporary directory, removed
    /// when dropped.
    pub struct TempPeers {
        dir: PathBuf,
    }

    impl TempPeers {
        /// A list that trusts the given devices, each with the rights a
        /// newly paired device has.
        pub fn trusting(devices: &[(&str, &Identity)]) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "istesso-media-test-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let peers = TempPeers { dir };
            let mut store = peers.open();
            for (name, identity) in devices {
                store.add(peer(name, identity)).unwrap();
            }
            peers
        }

        pub fn path(&self) -> PathBuf {
            self.dir.join("peers.json")
        }

        pub fn open(&self) -> PeerStore {
            PeerStore::open(self.path()).unwrap()
        }

        /// Gives the device called `name` exactly these rights.
        pub fn allow(&self, name: &str, permissions: &[Permission]) {
            let mut store = self.open();
            let mut peer = store.find(name).unwrap().unwrap().clone();
            peer.permissions = permissions.to_vec();
            store.add(peer).unwrap();
        }

        pub fn remove(&self, name: &str) {
            self.open().remove(name).unwrap().unwrap();
        }
    }

    impl Drop for TempPeers {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    pub fn peer(name: &str, identity: &Identity) -> Peer {
        Peer::from_hello(&Hello {
            device_name: name.to_string(),
            public_key: identity.public_key(),
        })
    }
}

#[cfg(test)]
mod failure_tests {
    use super::{Error, Failure};
    use istesso_link::Error as Link;
    use std::io;

    #[test]
    fn a_failure_is_one_of_six_words_and_names_nothing() {
        let words: Vec<&str> = [
            Failure::Refused,
            Failure::Unreachable,
            Failure::TimedOut,
            Failure::NoAnswer,
            Failure::NotReady,
            Failure::Other,
        ]
        .into_iter()
        .map(Failure::word)
        .collect();
        assert_eq!(
            words,
            [
                "refused",
                "unreachable",
                "timed_out",
                "no_answer",
                "not_ready",
                "other"
            ]
        );
        // The numbers are the ones of the header.
        assert_eq!((Failure::Refused as u32, Failure::Other as u32), (1, 6));
    }

    #[test]
    fn nothing_listening_is_told_from_no_way_to_the_machine() {
        // The machine answered: nothing listens there.
        assert_eq!(
            Failure::of_io(&io::Error::from(io::ErrorKind::ConnectionRefused)),
            Failure::Refused
        );
        assert_eq!(Failure::of_link(&Link::Refused), Failure::Refused);
        // No way to the machine from here, in every form the system says it. The last
        // two are what a program hears that this machine does not let out.
        #[cfg(unix)]
        for code in [
            libc::EHOSTUNREACH,
            libc::ENETUNREACH,
            libc::ENETDOWN,
            libc::EHOSTDOWN,
            libc::EADDRNOTAVAIL,
            libc::EACCES,
            libc::EPERM,
        ] {
            let err = io::Error::from_raw_os_error(code);
            assert_eq!(Failure::of_io(&err), Failure::Unreachable, "{code}");
            assert_eq!(
                Failure::of_link(&Link::Io(io::Error::from_raw_os_error(code))),
                Failure::Unreachable,
                "{code}"
            );
            assert_eq!(
                Error::Io(io::Error::from_raw_os_error(code)).failure(),
                Failure::Unreachable,
                "{code}"
            );
        }
    }

    #[test]
    fn silence_before_and_after_the_greeting_are_told_apart() {
        // No answer to connecting or to the first exchange.
        assert_eq!(
            Failure::of_io(&io::Error::from(io::ErrorKind::TimedOut)),
            Failure::TimedOut
        );
        assert_eq!(Failure::of_link(&Link::Timeout), Failure::TimedOut);
        assert_eq!(Error::Link(Link::Timeout).failure(), Failure::TimedOut);
        // Reached and greeted, and then nothing more, or the end.
        assert_eq!(Failure::of_link(&Link::Closed), Failure::NoAnswer);
        assert_eq!(Error::Silent.failure(), Failure::NoAnswer);
        assert_eq!(
            Failure::of_io(&io::Error::from(io::ErrorKind::ConnectionReset)),
            Failure::NoAnswer
        );
    }

    #[test]
    fn a_host_that_cannot_right_now_and_everything_else() {
        assert_eq!(Error::Busy.failure(), Failure::NotReady);
        assert_eq!(Error::Malformed.failure(), Failure::Other);
        assert_eq!(Failure::of_link(&Link::Malformed), Failure::Other);
        assert_eq!(
            Failure::of_io(&io::Error::from(io::ErrorKind::InvalidData)),
            Failure::Other
        );
    }
}
