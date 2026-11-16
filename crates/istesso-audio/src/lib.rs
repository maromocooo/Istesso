//! Carries the sound of one host to the audio exit on another.
//!
//! Sound is a service of its own, apart from any window that is being
//! relayed (ADR 0005). One device, the *exit*, plays what the others, the
//! *sources*, produce. The exit connects to a source, proves who it is with
//! the keys pinned at pairing, and is sent the source's sound as it is
//! captured, uncompressed (ADR 0011).
//!
//! The pieces, in the order the sound passes through them:
//!
//! * [`source`]: from the thread that captures audio to packets, without
//!   ever making that thread wait.
//! * [`wire`]: the packets and the first exchange of a connection.
//! * [`session`]: the two ends of a connection, on the channel of
//!   `istesso-link`. A source only sends to a paired device that holds the
//!   `audio` right, and looks the device up again while it sends.
//! * [`fifo`]: the queue between the receiving thread and the sound card's
//!   thread.
//! * [`playout`]: plays the queue at the sound card's pace. Decides when to
//!   begin, what to do when the queue runs dry, and when to skip ahead.
//! * [`control`] and [`resample`]: keep the source's clock and the sound
//!   card's clock together by moving the ratio of a resampler.
//! * [`probe`] and [`ledger`]: a test tone, what a recording of it shows,
//!   and a packet-by-packet comparison of what was sent and received.
//! * [`sim`]: a source, a network and a sound card with clocks of their
//!   own, for running hours of audio through the playout in a test.
//!
//! * `mac`: playing through Core Audio, on macOS.
//!
//! Capturing and playing need a real machine; everything else here does
//! not, and is tested without one.

pub mod clock;
pub mod control;
pub mod fifo;
pub mod ledger;
pub mod playout;
pub mod probe;
pub mod resample;
pub mod session;
pub mod sim;
pub mod source;
pub mod wire;

#[cfg(target_os = "macos")]
pub mod mac;

/// The ring buffers that hand samples between threads. Re-exported for the
/// programs that put the pieces of this crate together.
pub use rtrb;

/// Everything that can go wrong between a source and an exit.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Link(#[from] istesso_link::Error),
    #[error(transparent)]
    Pair(#[from] istesso_pair::Error),
    #[error("the other device sent a packet that cannot be read: {0}")]
    Wire(#[from] wire::WireError),
    #[error("the other device sent something that is not part of the audio protocol")]
    Malformed,
    #[error(
        "this device may not have that host's sound; allow it on the host with \
         `istesso peers allow <this device> audio`"
    )]
    Forbidden,
    #[error("the source refused: {0}")]
    Refused(String),
    #[error("the source sends its sound in a form this version cannot play: {0}")]
    Unsupported(String),
    #[error("the sound device cannot be used: {0}")]
    Device(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
