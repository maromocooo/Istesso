//! The sending end: a host that shows its picture to paired devices.
//!
//! A [`Host`] listens. A device that connects runs the handshake of the
//! control channel and says that it wants to watch, all before one
//! deadline; it is then sent every frame the host is handed, from the next
//! key frame on. The host only sends to a device that is paired and may
//! look at this host, and it looks the device up again while it sends:
//! removing the pairing, or taking the right away, ends a stream that is
//! running. When the list of paired devices cannot be read, nothing is
//! sent.
//!
//! Frames are written to the connections on the thread that hands them
//! over, with nothing queued in between. The list of paired devices is read
//! on another thread, so that reading a file never holds up a frame.

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use istesso_link::{Channel, Error as LinkError, LocalKeys};
use istesso_pair::{Identity, Peer, PeerStore, Permission};

use crate::clock;
use crate::timed::{wait_for, Ready, Timed};
use crate::wire::{self, Ask, ErrorCode, Header, Kind, Reply, StreamInfo};
use crate::{Error, Log, Result};

/// Callers a host deals with at the same time that have not yet shown who
/// they are and said what they want; further ones are dropped. These places
/// are apart from the ones below, so that callers who never get that far
/// cannot use up what paired devices are served with.
pub(crate) const MAX_PENDING: usize = 8;
/// Devices a host shows its picture to at the same time. One device has one
/// stream: when it connects again, its earlier connection is closed.
pub(crate) const MAX_STREAMS: usize = 4;
/// How long a caller has for the handshake and its first message, all of it
/// together. On a local network both take a few milliseconds.
const HELLO_WAIT: Duration = Duration::from_secs(3);
/// How long a single write to a viewer may take. A viewer that takes nothing
/// for this long is gone; the frames wait for nobody longer than this.
const SEND_WAIT: Duration = Duration::from_millis(200);
/// How long reading one message of a viewer may take, all of it together,
/// once its first bytes are there. They are a few dozen bytes and arrive in
/// one piece; a viewer that sends them a byte at a time is dropped.
const ASK_WAIT: Duration = Duration::from_millis(50);
/// Messages read from one viewer before the next frame goes out.
const ASKS_AT_ONCE: usize = 4;
/// How often the viewers are looked up again in the list of paired devices
/// while they are sent to.
const RECHECK: Duration = Duration::from_millis(250);
/// How old the last look at the list of paired devices may be for a frame
/// to be sent. Older than this, and nothing goes out until the list has
/// been read again.
const STALE: Duration = Duration::from_secs(1);
/// With no frame for this long, the host says that it has nothing new to
/// show, and repeats it this often.
const IDLE_EVERY: Duration = Duration::from_millis(500);
/// A viewer that still waits for a key frame after this long makes the host
/// ask for one again.
const KEY_AGAIN: Duration = Duration::from_millis(500);

/// A host that shows its picture to paired devices.
pub struct Host {
    shared: Arc<Shared>,
    addr: SocketAddr,
    threads: Vec<JoinHandle<()>>,
}

/// What became of a frame handed to [`Host::send_frame`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sent {
    /// The frame's sequence number.
    pub seq: u64,
    /// Viewers the frame was written to. A viewer that waits for a key
    /// frame is not sent the frames before it.
    pub viewers: u32,
    /// Messages of the encrypted channel the frame took, for each viewer.
    pub messages: u32,
}

struct Shared {
    keys: LocalKeys,
    peers_path: PathBuf,
    info: StreamInfo,
    watchers: Mutex<Watchers>,
    /// Callers that have not yet shown who they are and said what they
    /// want, and devices that are being shown the picture.
    pending: Arc<AtomicUsize>,
    serving: Arc<AtomicUsize>,
    seq: AtomicU64,
    /// Somebody needs a key frame, and the encoder has not been told yet.
    key_wanted: AtomicBool,
    /// When the list of paired devices was last read and the viewers were
    /// checked against it, on [`clock::now_ns`].
    last_read_ns: AtomicU64,
    /// How long a caller has for the handshake and its first message.
    hello_wait_ms: AtomicU64,
    /// For tests: the viewers are not looked up while this is set.
    recheck_paused: AtomicBool,
    stop: AtomicBool,
    log: Log,
}

struct Watchers {
    list: Vec<Watcher>,
    /// The packet being sent, kept to spare an allocation per frame.
    packet: Vec<u8>,
}

/// One device that is being shown the picture.
struct Watcher {
    channel: Channel<Timed>,
    peer: Peer,
    /// Since when the viewer has been waiting for a key frame: it cannot
    /// decode anything else. `None` once it has one.
    awaiting_key: Option<Instant>,
    /// Frames were held back from this viewer since the last one it got.
    skipped: bool,
    last_sent: Instant,
    _place: Place,
}

/// One of a limited number of places, given back when dropped.
struct Place(Arc<AtomicUsize>);

impl Place {
    /// Takes a place if fewer than `limit` are taken.
    fn take(taken: &Arc<AtomicUsize>, limit: usize) -> Option<Self> {
        if taken.fetch_add(1, Ordering::SeqCst) >= limit {
            taken.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Place(taken.clone()))
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Whether the device is, at this moment, still paired and still allowed to
/// look at this host, going by a list that has just been read: not by what
/// was true when the stream began.
fn may_still_view(peers: &PeerStore, peer: &Peer) -> bool {
    peers.peers().iter().any(|known| {
        known.id == peer.id
            && known.public_key == peer.public_key
            && known.permissions.contains(&Permission::View)
    })
}

impl Host {
    /// Starts listening on `addr`. `peers_path` is the list of paired
    /// devices; it is read afresh for every connection and while a stream
    /// runs. `info` is what viewers are told about the picture.
    pub fn listen(
        addr: SocketAddr,
        identity: &Identity,
        peers_path: PathBuf,
        info: StreamInfo,
        log: Log,
    ) -> Result<Self> {
        let keys = LocalKeys::new(identity)?;
        let listener = TcpListener::bind(addr)?;
        // The listening thread looks up now and then, to see whether the
        // host has been closed.
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared {
            keys,
            peers_path,
            info,
            watchers: Mutex::new(Watchers {
                list: Vec::new(),
                packet: Vec::new(),
            }),
            pending: Arc::new(AtomicUsize::new(0)),
            serving: Arc::new(AtomicUsize::new(0)),
            seq: AtomicU64::new(0),
            key_wanted: AtomicBool::new(false),
            last_read_ns: AtomicU64::new(0),
            hello_wait_ms: AtomicU64::new(HELLO_WAIT.as_millis() as u64),
            recheck_paused: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            log,
        });
        let accepting = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("istesso-media-accept".into())
                .spawn(move || shared.accept(listener))?
        };
        let checking = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("istesso-media-pairing".into())
                .spawn(move || shared.watch_pairing())?
        };
        Ok(Host {
            shared,
            addr,
            threads: vec![accepting, checking],
        })
    }

    /// Where the host listens.
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// What viewers are told about the picture.
    pub fn info(&self) -> &StreamInfo {
        &self.shared.info
    }

    /// Devices that are being shown the picture now.
    pub fn viewers(&self) -> usize {
        self.shared.watchers().list.len()
    }

    /// Sends one encoded frame to every viewer that can decode it, and
    /// returns when it has been written to their connections.
    ///
    /// `pts_ns` is the frame's presentation time stamp and `encoded_ns` the
    /// moment the encoder had finished it, on this machine's clock; both
    /// travel with the frame. A viewer that has just connected, or that was
    /// held back, is sent nothing until `key_frame` is set.
    pub fn send_frame(
        &self,
        frame: &[u8],
        pts_ns: u64,
        encoded_ns: u64,
        key_frame: bool,
    ) -> Result<Sent> {
        if frame.len() > wire::MAX_FRAME_BYTES {
            return Err(wire::WireError::Length.into());
        }
        let shared = &*self.shared;
        let seq = shared.seq.fetch_add(1, Ordering::SeqCst);
        let mut sent = Sent {
            seq,
            viewers: 0,
            messages: wire::channel_messages(frame.len()),
        };
        // Read before the lock is taken: a list that goes stale while this
        // frame waits for the lock is found stale by the next one.
        let fresh = shared.pairing_is_fresh();
        let mut watchers = shared.watchers();
        let Watchers { list, packet } = &mut *watchers;
        if list.is_empty() {
            return Ok(sent);
        }
        let header = Header {
            kind: Kind::Frame,
            key_frame,
            discontinuity: false,
            seq,
            pts_ns,
            encoded_ns,
        };
        wire::encode(&header, frame, packet);
        list.retain_mut(|watcher| {
            if let Err(err) = shared.read_asks(watcher) {
                shared.gone(watcher, &err);
                return false;
            }
            if !fresh {
                // Nobody has looked at the list of paired devices for too
                // long: whether this device may still look is not known.
                shared.hold_back(watcher);
                return true;
            }
            if let (Some(since), false) = (watcher.awaiting_key, key_frame) {
                watcher.skipped = true;
                if since.elapsed() >= KEY_AGAIN {
                    watcher.awaiting_key = Some(Instant::now());
                    shared.key_wanted.store(true, Ordering::SeqCst);
                }
                return true;
            }
            wire::set_discontinuity(packet, watcher.skipped);
            match watcher.channel.send(packet) {
                Ok(()) => {
                    watcher.awaiting_key = None;
                    watcher.skipped = false;
                    watcher.last_sent = Instant::now();
                    sent.viewers += 1;
                    true
                }
                Err(err) => {
                    shared.gone(watcher, &err.into());
                    false
                }
            }
        });
        Ok(sent)
    }

    /// Whether a key frame is needed: a viewer has connected, has asked for
    /// one, or was held back. True once for each time it is needed; the
    /// caller tells the encoder.
    pub fn key_frame_wanted(&self) -> bool {
        let shared = &*self.shared;
        // A viewer's request is noticed here too, not only when the next
        // frame goes out: while the picture stands still there is none.
        shared.watchers().list.retain_mut(|watcher| {
            shared
                .read_asks(watcher)
                .map_err(|err| shared.gone(watcher, &err))
                .is_ok()
        });
        shared.key_wanted.swap(false, Ordering::SeqCst)
    }

    /// Changes how long a caller has for the handshake and its first
    /// message. Tests use short times.
    #[cfg(test)]
    pub(crate) fn set_hello_wait(&self, wait: Duration) {
        self.shared
            .hello_wait_ms
            .store(wait.as_millis() as u64, Ordering::Relaxed);
    }

    /// Stops, or starts again, the looking up of viewers in the list of
    /// paired devices, as if the thread that does it were stuck.
    #[cfg(test)]
    pub(crate) fn pause_recheck(&self, paused: bool) {
        self.shared.recheck_paused.store(paused, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn pending(&self) -> usize {
        self.shared.pending.load(Ordering::SeqCst)
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
        let mut watchers = self.shared.watchers();
        let Watchers { list, packet } = &mut *watchers;
        let seq = self.shared.seq.load(Ordering::SeqCst);
        wire::encode(&Header::without_frame(Kind::End, seq), &[], packet);
        for watcher in list.iter_mut() {
            // Best effort: the viewer may be gone already.
            let _ = watcher.channel.send(packet);
        }
        list.clear();
    }
}

fn reply(channel: &mut Channel<Timed>, reply: &Reply) -> Result<()> {
    let bytes = serde_json::to_vec(reply).map_err(|_| Error::Malformed)?;
    Ok(channel.send(&bytes)?)
}

fn refuse(channel: &mut Channel<Timed>, code: ErrorCode, message: &str) -> Result<()> {
    reply(
        channel,
        &Reply::Error {
            code,
            message: message.into(),
        },
    )
}

impl Shared {
    fn watchers(&self) -> MutexGuard<'_, Watchers> {
        self.watchers.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether the viewers were checked against the list of paired devices
    /// recently enough for a frame to be sent.
    fn pairing_is_fresh(&self) -> bool {
        let read = self.last_read_ns.load(Ordering::SeqCst);
        read != 0 && clock::now_ns().saturating_sub(read) <= STALE.as_nanos() as u64
    }

    fn gone(&self, watcher: &Watcher, why: &Error) {
        (self.log)(format!(
            "stopped showing the picture to {}: {why}",
            watcher.peer.name
        ));
    }

    /// The viewer is not sent this frame. It cannot decode the ones after
    /// it either, until a key frame comes.
    fn hold_back(&self, watcher: &mut Watcher) {
        watcher.skipped = true;
        if watcher.awaiting_key.is_none() {
            watcher.awaiting_key = Some(Instant::now());
            self.key_wanted.store(true, Ordering::SeqCst);
        }
    }

    /// Reads what the viewer has sent, without waiting for anything that is
    /// not there yet.
    fn read_asks(&self, watcher: &mut Watcher) -> Result<()> {
        for _ in 0..ASKS_AT_ONCE {
            let timed = watcher.channel.stream();
            if wait_for(timed.stream(), None, Duration::ZERO)? != Ready::Connection {
                break;
            }
            // Something has come (or the viewer has hung up). The rest of
            // the message has to be there within a short time, all of it
            // together: the frames wait behind this read.
            timed.limit(Instant::now() + ASK_WAIT);
            let message = watcher.channel.recv();
            watcher.channel.stream().lift(ASK_WAIT, SEND_WAIT)?;
            let message = message?;
            if message.len() > wire::MAX_ASK_BYTES {
                return Err(Error::Malformed);
            }
            // What this version does not know is passed over.
            if let Ok(Ask::KeyFrame) = serde_json::from_slice::<Ask>(&message) {
                self.key_wanted.store(true, Ordering::SeqCst);
            }
        }
        Ok(())
    }

    /// Takes callers until the host is closed. Every caller gets a thread
    /// of its own, if there is a place for it.
    fn accept(self: Arc<Self>, listener: TcpListener) {
        while !self.stop.load(Ordering::SeqCst) {
            match wait_for(&listener, None, Duration::from_millis(200)) {
                Ok(Ready::Connection) => {}
                Ok(_) => continue,
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
            }
            let Ok((stream, _)) = listener.accept() else {
                // Typically "too many open files"; give the system a moment.
                std::thread::sleep(Duration::from_millis(100));
                continue;
            };
            let Some(pending) = Place::take(&self.pending, MAX_PENDING) else {
                continue;
            };
            let shared = self.clone();
            std::thread::spawn(move || {
                if let Err(err) = shared.serve(stream, pending) {
                    // Connections that die during the handshake are port
                    // scans and the like; only what somebody can act on is
                    // shown.
                    if matches!(
                        err,
                        Error::Link(
                            LinkError::UnknownPeer
                                | LinkError::Version { .. }
                                | LinkError::BadIdentity
                        )
                    ) {
                        (shared.log)(format!("refused a connection: {err}"));
                    }
                }
            });
        }
    }

    fn serve(&self, stream: TcpStream, pending: Place) -> Result<()> {
        // On some systems a connection takes over the listener's way of not
        // waiting; this one waits, with the times set below.
        stream.set_nonblocking(false)?;
        // A frame leaves when it is written, not when more has piled up.
        stream.set_nodelay(true)?;
        // Whoever calls has this long to show who it is and to say what it
        // wants, all of it together, however slowly the bytes come.
        let hello_wait = Duration::from_millis(self.hello_wait_ms.load(Ordering::Relaxed));
        let stream = Timed::new(stream, Instant::now() + hello_wait);

        // The list of paired devices is read for every caller: a pairing
        // that was removed a moment ago must not be honoured any more. A
        // list that cannot be read lets nobody in.
        let peers = PeerStore::open(&self.peers_path)?;
        let read_at = clock::now_ns();
        let mut found: Option<Peer> = None;
        let mut channel = Channel::respond(stream, &self.keys, |key| {
            found = peers.find_by_key(key).cloned();
            found.is_some()
        })?;
        let peer = found.ok_or(LinkError::UnknownPeer)?;

        let first = channel.recv()?;
        if !matches!(serde_json::from_slice::<Ask>(&first), Ok(Ask::Watch { .. })) {
            return refuse(
                &mut channel,
                ErrorCode::Unsupported,
                "this version of istesso does not know that request",
            );
        }
        if !peer.permissions.contains(&Permission::View) {
            (self.log)(format!(
                "refused to show the picture to {}: it is paired, but may not look at this host",
                peer.name
            ));
            return refuse(
                &mut channel,
                ErrorCode::Forbidden,
                "this device may not look at the host",
            );
        }
        // A paired device that may look, and has said what it wants: it
        // moves from the callers to the viewers. One device has one stream.
        drop(pending);
        self.close_earlier(&peer);
        let Some(place) = Place::take(&self.serving, MAX_STREAMS) else {
            return refuse(
                &mut channel,
                ErrorCode::Busy,
                "the host shows its picture to as many devices as it can",
            );
        };
        reply(
            &mut channel,
            &Reply::Stream {
                version: u32::from(wire::VERSION),
                info: self.info,
            },
        )?;
        // No deadline for a stream; a single write that takes too long ends
        // it.
        channel.stream().lift(ASK_WAIT, SEND_WAIT)?;

        self.close_earlier(&peer);
        let mut watchers = self.watchers();
        if self.stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        (self.log)(format!("showing the picture to {}", peer.name));
        watchers.list.push(Watcher {
            channel,
            peer,
            awaiting_key: Some(Instant::now()),
            skipped: false,
            last_sent: Instant::now(),
            _place: place,
        });
        // The list was read for this caller a moment ago; with the first
        // viewer the regular look-ups begin.
        self.last_read_ns.fetch_max(read_at, Ordering::SeqCst);
        // A viewer can only begin with a key frame, and is not made to ask.
        self.key_wanted.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Closes the connection a device already has, when it connects again:
    /// a viewer that was stopped and started must not find its place taken
    /// by what it left behind.
    fn close_earlier(&self, peer: &Peer) {
        self.watchers().list.retain(|watcher| {
            let same = watcher.peer.id == peer.id;
            if same {
                (self.log)(format!(
                    "{} connected again; its earlier connection is closed",
                    peer.name
                ));
            }
            !same
        });
    }

    /// Looks every viewer up in the list of paired devices once every
    /// [`RECHECK`], until the host is closed. A viewer that is no longer
    /// paired, has another key, or has lost the right to look is cut off;
    /// when the list cannot be read, all of them are.
    fn watch_pairing(&self) {
        let mut checked = Instant::now();
        while !self.stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(10));
            if checked.elapsed() < RECHECK {
                continue;
            }
            checked = Instant::now();
            if self.recheck_paused.load(Ordering::SeqCst) || self.watchers().list.is_empty() {
                continue;
            }
            // The file is read with the lock let go: frames go out meanwhile.
            let peers = PeerStore::open(&self.peers_path);
            let read_at = clock::now_ns();
            let mut watchers = self.watchers();
            let Watchers { list, packet } = &mut *watchers;
            let Ok(peers) = peers else {
                if !list.is_empty() {
                    (self.log)(
                        "the list of paired devices cannot be read: nothing is sent, and every \
                         viewer is cut off"
                            .into(),
                    );
                }
                list.clear();
                continue;
            };
            let seq = self.seq.load(Ordering::SeqCst);
            list.retain_mut(|watcher| {
                if !may_still_view(&peers, &watcher.peer) {
                    (self.log)(format!(
                        "stopped showing the picture to {}: it is no longer paired, or may no \
                         longer look at this host",
                        watcher.peer.name
                    ));
                    return false;
                }
                if let Err(err) = self.read_asks(watcher) {
                    self.gone(watcher, &err);
                    return false;
                }
                if watcher.last_sent.elapsed() >= IDLE_EVERY {
                    // A picture that stands still is not a broken connection.
                    wire::encode(&Header::without_frame(Kind::Idle, seq), &[], packet);
                    if let Err(err) = watcher.channel.send(packet) {
                        self.gone(watcher, &err.into());
                        return false;
                    }
                    watcher.last_sent = Instant::now();
                }
                true
            });
            self.last_read_ns.fetch_max(read_at, Ordering::SeqCst);
        }
    }
}
