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
use crate::wire::{
    self, Ask, Codec, Cursor, ErrorCode, Header, Kind, Reply, StreamInfo, WireError,
};
use crate::{Error, Result};

/// How long one read may take once a frame has begun to arrive. A frame
/// crosses a local network in a millisecond or two; a host that stops in
/// the middle of one is gone.
const FRAME_WAIT: Duration = Duration::from_secs(3);
/// How long a single write to the host may take.
const ASK_WAIT: Duration = Duration::from_secs(1);
/// A host says something at least twice a second, a frame or that it has
/// none. One that has said nothing for this long is taken to be gone: two
/// of those words are missing then. A network may stop delivering on a
/// connection that both ends still believe in, and this is the only way
/// to notice. The same second as for sound.
pub const SILENT_AFTER: Duration = Duration::from_secs(1);

/// A connection to a host that shows this device its picture.
pub struct Viewer {
    channel: Channel<Timed>,
    peer: Peer,
    info: StreamInfo,
    asking: Arc<Asking>,
    last_heard: Instant,
    /// The packet handed out last.
    packet: Vec<u8>,
    /// The packet that said what the cursor is, and where in it that is said. Kept apart
    /// from the frames, which come and go while the cursor stays.
    cursor_packet: Vec<u8>,
    cursor_at: Option<std::ops::Range<usize>>,
}

/// What a host sends a viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event<'a> {
    Frame(Frame<'a>),
    /// The pointer's cursor looks like this from now on. The viewer draws it where its own
    /// pointer is; the host does not draw it into the frames.
    Cursor(Cursor<'a>),
}

/// What [`Viewer::pull`] found, without the bytes.
pub(crate) enum Pulled {
    Frame {
        header: Header,
        at: std::ops::Range<usize>,
        first_ns: u64,
        whole_ns: u64,
    },
    Cursor,
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
        Self::open(addr, keys, peers, expected, within)?.watch()
    }

    /// The first half of [`Viewer::connect`]: connects to the host at
    /// `addr` and runs the handshake, within this time, all of it together.
    /// Nothing is asked of the host yet, and the host does nothing for this
    /// connection yet: a device that had a picture keeps getting it on the
    /// connection it has. One try at connecting again is this, and several
    /// are made at once ([`crate::rejoin`]).
    pub fn open(
        addr: SocketAddr,
        keys: &LocalKeys,
        peers: &PeerStore,
        expected: Option<&str>,
        within: Duration,
    ) -> Result<Opened> {
        let deadline = Instant::now() + within;
        let stream = TcpStream::connect_timeout(&addr, within).map_err(connect_error)?;
        // A request for a key frame leaves when it is written.
        stream.set_nodelay(true)?;
        let stream = Timed::new(stream, deadline);

        let mut found: Option<Peer> = None;
        let channel = Channel::initiate(stream, keys, |key| {
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
        Ok(Opened { channel, peer })
    }
}

/// A connection to a host that has shown who it is, on which nothing has
/// been asked yet. Dropping it closes it, and that is all it ever was to
/// the host.
pub struct Opened {
    channel: Channel<Timed>,
    peer: Peer,
}

impl Opened {
    /// The host at the other end.
    pub fn peer(&self) -> &Peer {
        &self.peer
    }

    /// The second half of [`Viewer::connect`]: asks the host for its
    /// picture on this connection, which is the request that makes the
    /// host use this connection for the device from now on. The answer has
    /// what is left of the time the connection was opened with.
    pub fn watch(self) -> Result<Viewer> {
        let Opened { mut channel, peer } = self;
        let watch = Ask::Watch {
            version: u32::from(wire::VERSION),
        };
        channel.send(&serde_json::to_vec(&watch).map_err(|_| Error::Malformed)?)?;
        let answer = channel.recv()?;
        let info = match serde_json::from_slice::<Reply>(&answer).map_err(|_| Error::Malformed)? {
            Reply::Stream { info, .. } => info,
            Reply::Error { code, message } => {
                return Err(match code {
                    ErrorCode::Forbidden => Error::Forbidden,
                    ErrorCode::NotShown => Error::NotShown,
                    ErrorCode::Busy => Error::Busy,
                    ErrorCode::Unsupported => Error::Unsupported(message),
                    // A reason a later version gives: refused, whatever it is.
                    ErrorCode::Other => Error::Refused(message),
                });
            }
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
            cursor_packet: Vec::new(),
            cursor_at: None,
        })
    }

    /// As [`Opened::watch`], with `within` this time for the host's
    /// answer, counted from now: a try that completed its handshake just
    /// before its time was up can still be asked on.
    pub fn watch_within(self, within: Duration) -> Result<Viewer> {
        self.channel.stream().limit(Instant::now() + within);
        self.watch()
    }
}

impl Viewer {
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

    /// When the host was last heard: a frame, what the cursor is, or that
    /// there is no picture to send.
    pub fn last_heard(&self) -> Instant {
        self.last_heard
    }

    /// [`Viewer::next`] in two steps, for a caller that has to let go of
    /// the viewer in between: what came, without its bytes.
    pub(crate) fn pull_for(&mut self, wait: Duration) -> Result<Option<Pulled>> {
        self.pull(wait)
    }

    /// The second step: what was pulled last, with its bytes.
    pub(crate) fn event_of(&self, pulled: Pulled) -> Option<Event<'_>> {
        match pulled {
            Pulled::Frame {
                header,
                at,
                first_ns,
                whole_ns,
            } => Some(Event::Frame(self.frame(header, at, first_ns, whole_ns))),
            Pulled::Cursor => self.cursor().map(Event::Cursor),
        }
    }

    /// What the host last said the cursor is. `None` until it has said so: a host that
    /// draws the cursor into its frames never does.
    pub fn cursor(&self) -> Option<Cursor<'_>> {
        let at = self.cursor_at.clone()?;
        wire::decode_cursor(&self.cursor_packet[at]).ok()
    }

    /// Waits for the next frame, for `wait` at most. `Ok(None)` means that
    /// none came in that time, which is what happens while the picture
    /// stands still. An error means that the connection is over: the host
    /// has ended it, has said nothing for too long, or sent something that
    /// cannot be read. The viewer is then dropped, and a new one connects.
    ///
    /// What the host says about the cursor meanwhile is kept for
    /// [`Viewer::cursor`]; [`Viewer::next`] hands it over as it comes.
    pub fn next_frame(&mut self, wait: Duration) -> Result<Option<Frame<'_>>> {
        let until = Instant::now() + wait;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match self.pull(left)? {
                Some(Pulled::Frame {
                    header,
                    at,
                    first_ns,
                    whole_ns,
                }) => return Ok(Some(self.frame(header, at, first_ns, whole_ns))),
                Some(Pulled::Cursor) => continue,
                None => return Ok(None),
            }
        }
    }

    /// Waits for the next thing the host sends, a frame or what the cursor
    /// is, for `wait` at most. Otherwise as [`Viewer::next_frame`].
    pub fn next(&mut self, wait: Duration) -> Result<Option<Event<'_>>> {
        Ok(match self.pull(wait)? {
            Some(Pulled::Frame {
                header,
                at,
                first_ns,
                whole_ns,
            }) => Some(Event::Frame(self.frame(header, at, first_ns, whole_ns))),
            Some(Pulled::Cursor) => self.cursor().map(Event::Cursor),
            None => None,
        })
    }

    fn frame(
        &self,
        header: Header,
        at: std::ops::Range<usize>,
        first_ns: u64,
        whole_ns: u64,
    ) -> Frame<'_> {
        let data = &self.packet[at];
        Frame {
            data,
            key_frame: header.key_frame,
            discontinuity: header.discontinuity,
            seq: header.seq,
            pts_ns: header.pts_ns,
            encoded_ns: header.encoded_ns,
            first_ns,
            whole_ns,
            messages: wire::channel_messages(data.len()),
        }
    }

    /// Receives until a frame or a cursor has come, for `wait` at most.
    fn pull(&mut self, wait: Duration) -> Result<Option<Pulled>> {
        let until = Instant::now() + wait;
        loop {
            if self.asking.key_frame.swap(false, Ordering::SeqCst) {
                self.asking.wake.clear();
                let ask = serde_json::to_vec(&Ask::KeyFrame).map_err(|_| Error::Malformed)?;
                self.channel.send(&ask)?;
            }
            // The host counts as silent only when nothing of it is there to be read:
            // what arrived while nobody was asking is not silence.
            let left = until.saturating_duration_since(Instant::now());
            let silent_in = SILENT_AFTER.saturating_sub(self.last_heard.elapsed());
            let ready = wait_for(
                self.channel.stream().stream(),
                Some(&self.asking.wake),
                left.min(silent_in),
            )?;
            match ready {
                Ready::Woken => {
                    self.asking.wake.clear();
                    continue;
                }
                Ready::Neither if self.last_heard.elapsed() >= SILENT_AFTER => {
                    return Err(Error::Silent)
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
                    Kind::Frame => {
                        return Ok(Some(Pulled::Frame {
                            header,
                            at,
                            first_ns,
                            whole_ns,
                        }))
                    }
                    Kind::Idle => continue,
                    Kind::End => {
                        return Err(match wire::decode_end(&self.packet[at]) {
                            wire::Ending::Over => Error::Ended,
                            wire::Ending::NotShown => Error::NotShown,
                            wire::Ending::SetAside => Error::SetAside,
                        })
                    }
                    Kind::Cursor => {
                        // The image is whatever an application on the host handed in: it
                        // is taken only if its numbers fit its length and the limits.
                        // One that does not is passed over, and the cursor stays as it
                        // was.
                        if wire::decode_cursor(&self.packet[at.clone()]).is_ok() {
                            std::mem::swap(&mut self.packet, &mut self.cursor_packet);
                            self.cursor_at = Some(at);
                            return Ok(Some(Pulled::Cursor));
                        }
                    }
                },
                // A kind of packet that a later version sends: passed over.
                Err(WireError::Kind(_)) => continue,
                Err(err) => return Err(err.into()),
            }
        }
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
