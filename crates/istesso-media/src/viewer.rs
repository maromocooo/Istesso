//! The receiving end: a device that is shown a host's picture.
//!
//! A [`Viewer`] connects to a host, checks that a paired device answers
//! there, and is then handed the host's frames one by one, whole, in the
//! order they were sent. An [`Asker`] asks the host for a key frame from
//! any thread.

use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use istesso_link::{Channel, Error as LinkError, LocalKeys};
use istesso_pair::{Peer, PeerStore};

use crate::clock;
use crate::timed::{wait_for, Ready, Timed, Wake};
use crate::wire::{self, Ask, Codec, ErrorCode, Header, Kind, Reply, StreamInfo, WireError};
use crate::{Error, Result};

/// How long one read may take once a frame has begun to arrive. A frame
/// crosses a local network in a millisecond or two; a host that stops in
/// the middle of one is gone.
const FRAME_WAIT: Duration = Duration::from_secs(3);
/// How long a single write to the host may take.
const ASK_WAIT: Duration = Duration::from_secs(1);
/// A host says something at least twice a second, a frame or that it has
/// none. One that has said nothing for this long is taken to be gone.
const SILENT_AFTER: Duration = Duration::from_secs(3);

/// A connection to a host that shows this device its picture.
pub struct Viewer {
    channel: Channel<Timed>,
    peer: Peer,
    info: StreamInfo,
    asking: Arc<Asking>,
    last_heard: Instant,
    /// The packet handed out last.
    packet: Vec<u8>,
}

struct Asking {
    key_frame: AtomicBool,
    wake: Wake,
}

/// Asks the host of a [`Viewer`] for a key frame, from any thread.
#[derive(Clone)]
pub struct Asker(Arc<Asking>);

impl Asker {
    /// The picture cannot be decoded from here on: have the host send a key
    /// frame. The request leaves on the thread that receives, at once if
    /// that thread is waiting for a frame.
    pub fn ask_for_key_frame(&self) {
        self.0.key_frame.store(true, Ordering::SeqCst);
        self.0.wake.wake();
    }
}

/// One frame, as the host sent it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame<'a> {
    /// The encoded frame.
    pub data: &'a [u8],
    pub key_frame: bool,
    /// The host sent this viewer nothing between the frame before and this
    /// one, though it encoded frames in between.
    pub discontinuity: bool,
    pub seq: u64,
    /// Presentation time stamp, on the clock of the host's pipeline.
    pub pts_ns: u64,
    /// When the host's encoder had finished the frame, on the host's clock.
    pub encoded_ns: u64,
    /// When the first bytes of the frame were there to be read, on
    /// [`clock::now_ns`].
    pub first_ns: u64,
    /// When the whole frame had been read and opened, on the same clock.
    pub whole_ns: u64,
    /// Messages of the encrypted channel the frame came in.
    pub messages: u32,
}

impl Viewer {
    /// Connects to the host at `addr` and asks for its picture.
    ///
    /// Any paired device that answers there is taken, unless `expected`
    /// names one (by id): then any other is refused, even if it is paired.
    /// Connecting, the handshake and the host's answer have `within` this
    /// time, all of it together.
    pub fn connect(
        addr: SocketAddr,
        keys: &LocalKeys,
        peers: &PeerStore,
        expected: Option<&str>,
        within: Duration,
    ) -> Result<Self> {
        let deadline = Instant::now() + within;
        let stream = TcpStream::connect_timeout(&addr, within).map_err(connect_error)?;
        // A request for a key frame leaves when it is written.
        stream.set_nodelay(true)?;
        let stream = Timed::new(stream, deadline);

        let mut found: Option<Peer> = None;
        let mut channel = Channel::initiate(stream, keys, |key| {
            let peer = peers.find_by_key(key).ok_or(LinkError::UnknownPeer)?;
            if let Some(expected) = expected {
                if peer.id != expected {
                    return Err(LinkError::WrongPeer {
                        expected: expected.to_string(),
                        found: format!("{} ({})", peer.name, peer.id),
                    });
                }
            }
            found = Some(peer.clone());
            Ok(())
        })?;
        // `initiate` only succeeds after the closure has run and agreed.
        let peer = found.ok_or(LinkError::UnknownPeer)?;

        let watch = Ask::Watch {
            version: u32::from(wire::VERSION),
        };
        channel.send(&serde_json::to_vec(&watch).map_err(|_| Error::Malformed)?)?;
        let answer = channel.recv()?;
        let info = match serde_json::from_slice::<Reply>(&answer).map_err(|_| Error::Malformed)? {
            Reply::Stream { info, .. } => info,
            Reply::Error {
                code: ErrorCode::Forbidden,
                ..
            } => return Err(Error::Forbidden),
            Reply::Error { message, .. } => return Err(Error::Refused(message)),
        };
        if info.codec == Codec::Unknown {
            return Err(Error::Unsupported(
                "an encoding this version does not know".into(),
            ));
        }
        channel.stream().lift(FRAME_WAIT, ASK_WAIT)?;
        Ok(Viewer {
            channel,
            peer,
            info,
            asking: Arc::new(Asking {
                key_frame: AtomicBool::new(false),
                wake: Wake::new()?,
            }),
            last_heard: Instant::now(),
            packet: Vec::new(),
        })
    }

    /// The host at the other end.
    pub fn peer(&self) -> &Peer {
        &self.peer
    }

    /// What the host said about its picture.
    pub fn info(&self) -> &StreamInfo {
        &self.info
    }

    /// A handle for asking for a key frame from another thread.
    pub fn asker(&self) -> Asker {
        Asker(self.asking.clone())
    }

    /// Waits for the next frame, for `wait` at most. `Ok(None)` means that
    /// none came in that time, which is what happens while the picture
    /// stands still. An error means that the connection is over: the host
    /// has ended it, has said nothing for too long, or sent something that
    /// cannot be read. The viewer is then dropped, and a new one connects.
    pub fn next_frame(&mut self, wait: Duration) -> Result<Option<Frame<'_>>> {
        let until = Instant::now() + wait;
        let (header, at, first_ns, whole_ns) = loop {
            if self.asking.key_frame.swap(false, Ordering::SeqCst) {
                self.asking.wake.clear();
                let ask = serde_json::to_vec(&Ask::KeyFrame).map_err(|_| Error::Malformed)?;
                self.channel.send(&ask)?;
            }
            if self.last_heard.elapsed() >= SILENT_AFTER {
                return Err(Error::Silent);
            }
            let left = until.saturating_duration_since(Instant::now());
            let ready = wait_for(
                self.channel.stream().stream(),
                Some(&self.asking.wake),
                left.min(SILENT_AFTER),
            )?;
            match ready {
                Ready::Woken => {
                    self.asking.wake.clear();
                    continue;
                }
                Ready::Neither if left.is_zero() || Instant::now() >= until => return Ok(None),
                Ready::Neither => continue,
                Ready::Connection => {}
            }
            let first_ns = clock::now_ns();
            self.packet = self.channel.recv()?;
            let whole_ns = clock::now_ns();
            self.last_heard = Instant::now();
            match wire::decode(&self.packet) {
                Ok((header, at)) => match header.kind {
                    Kind::Frame => break (header, at, first_ns, whole_ns),
                    Kind::Idle => continue,
                    Kind::End => return Err(Error::Ended),
                },
                // A kind of packet that a later version sends: passed over.
                Err(WireError::Kind(_)) => continue,
                Err(err) => return Err(err.into()),
            }
        };
        let Header {
            key_frame,
            discontinuity,
            seq,
            pts_ns,
            encoded_ns,
            ..
        } = header;
        let data = &self.packet[at];
        Ok(Some(Frame {
            data,
            key_frame,
            discontinuity,
            seq,
            pts_ns,
            encoded_ns,
            first_ns,
            whole_ns,
            messages: wire::channel_messages(data.len()),
        }))
    }
}

/// What failing to connect means, in the words of the control channel.
fn connect_error(err: std::io::Error) -> Error {
    use std::io::ErrorKind::*;
    match err.kind() {
        WouldBlock | TimedOut => LinkError::Timeout.into(),
        ConnectionRefused => LinkError::Refused.into(),
        _ => Error::Io(err),
    }
}
