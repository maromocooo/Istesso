//! The control channel: how paired devices talk to each other.
//!
//! Pairing ([`istesso_pair`]) leaves each device with the other's public key. This
//! crate turns that into a private conversation:
//!
//! * [`channel`] runs a Noise XX handshake over a connection. Each side proves
//!   that its key-exchange key belongs to its device identity, and each side
//!   checks that identity against its list of paired devices. Nothing is
//!   exchanged with a device that is not on that list.
//! * [`message`] defines what one device can ask of another.
//! * [`client`] and [`server`] are the two ends of a conversation.
//! * [`shell`] is what a conversation carries once it has become a shell.
//!
//! Addresses are never trusted. Whoever answers at an address has to pass the
//! handshake, so a wrong or forged address (from discovery or from memory) can
//! only make a connection fail; it cannot send data to the wrong device.
//!
//! See `docs/adr/0007-control-channel.md` for the reasoning.

pub mod channel;
pub mod client;
mod frame;
pub mod message;
pub mod server;
pub mod shell;

pub use channel::{Channel, ChannelReader, ChannelWriter, LocalKeys};
pub use client::{Client, InputAnswer, ShellAnswer};
pub use message::{ErrorCode, Request, Response, ShellRequest};
pub use server::{HangUp, Session};

/// Port the agent listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 47801;

/// Everything that can go wrong on the control channel.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "the other device speaks a different protocol version ({theirs}, this device: {ours}); \
         update `istesso` on the older one"
    )]
    Version { ours: String, theirs: String },
    #[error("the other device is not paired with this one")]
    UnknownPeer,
    #[error("expected {expected} at that address but {found} answered")]
    WrongPeer { expected: String, found: String },
    #[error("the other device does not trust this one (was the pairing removed there?)")]
    NotTrusted,
    #[error("the other device could not prove its identity")]
    BadIdentity,
    #[error("the other device sent something that is not part of the protocol")]
    Malformed,
    #[error("the message is larger than the protocol allows")]
    TooLarge,
    #[error("timed out waiting for the other device")]
    Timeout,
    #[error("the machine is up but nothing listens there (is `istesso agent` running on it?)")]
    Refused,
    #[error("the connection was closed")]
    Closed,
    #[error("the encrypted channel failed: {0}")]
    Crypto(String),
    #[error("the other device reported: {0}")]
    Remote(String),
    #[error("I/O error: {0}")]
    Io(std::io::Error),
}

impl Error {
    /// Translates a socket error into what it means for a conversation.
    pub(crate) fn from_io(err: std::io::Error) -> Self {
        use std::io::ErrorKind::*;
        match err.kind() {
            WouldBlock | TimedOut => Error::Timeout,
            ConnectionRefused => Error::Refused,
            UnexpectedEof | ConnectionReset | ConnectionAborted | BrokenPipe => Error::Closed,
            _ => Error::Io(err),
        }
    }

    pub(crate) fn crypto(err: snow::Error) -> Self {
        Error::Crypto(err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
pub(crate) mod testing {
    //! Helpers shared by the tests of this crate.

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use istesso_pair::{Hello, Identity, Peer, PeerStore};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A peer store in its own temporary directory, removed when dropped.
    pub struct TempStore {
        dir: PathBuf,
        pub store: PeerStore,
    }

    impl TempStore {
        /// Creates a store that trusts the given devices.
        pub fn trusting(devices: &[(&str, &Identity)]) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "istesso-link-test-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let mut store = PeerStore::open(dir.join("peers.json")).unwrap();
            for (name, identity) in devices {
                store
                    .add(Peer::from_hello(&Hello {
                        device_name: name.to_string(),
                        public_key: identity.public_key(),
                    }))
                    .unwrap();
            }
            TempStore { dir, store }
        }
    }

    impl Drop for TempStore {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}
