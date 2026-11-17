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
pub use viewer::{Asker, Event, Frame, Opened, Viewer};
pub use wire::{Chroma, Codec, Colour, Cursor, CursorImage, StreamInfo, DEFAULT_PORT};

/// Receives one line for each thing worth telling the person who runs a
/// host. Never a key, and never an address.
pub type Log = std::sync::Arc<dyn Fn(String) + Send + Sync>;

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
/// made: connect again soon, at leisure, or never. The same for input and
/// for sound.
pub use istesso_link::Again;

impl Error {
    /// What a viewer does after this error. Only a connection that ended
    /// without a word is taken up again: when the host said why it stops,
    /// or refused this device, the answer would be the same the next time,
    /// and a device that takes a picture back from the one it was moved to
    /// would never stop doing so.
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
