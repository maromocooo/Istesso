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
//! Who waits for whom:
//!
//! * The thread that hands a frame over never waits for a viewer. The frame
//!   is sealed for each viewer and put on its connection as far as the
//!   connection takes it at that moment, which on a local network is all of
//!   it. What a connection does not take is left to that viewer's own
//!   thread, with one deadline for the whole frame; until it has gone, the
//!   viewer is sent nothing more, and then begins again with a key frame.
//!   A viewer that takes its frames slowly, or not at all, costs the others
//!   nothing.
//! * Every viewer has a thread of its own that reads what the viewer says,
//!   finishes what the connection did not take at once, and says that the
//!   picture stands still when it does.
//! * The list of paired devices is read on yet another thread, which holds
//!   the list of viewers only to strike viewers off it: no reading of a
//!   file and no writing to a viewer ever stands between a pairing being
//!   removed and its stream ending.

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::fd::{AsRawFd, RawFd};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use istesso_link::{Channel, Error as LinkError, LocalKeys};
use istesso_pair::{Identity, Peer, PeerStore, Permission};

use crate::clock;
use crate::timed::{wait_for, Ready, Timed, Wake};
use crate::wire::{
    self, Ask, Cursor, Ending, ErrorCode, Header, Kind, Reply, StreamInfo, TextField,
};
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
/// How long a viewer has to take a frame, all of it together, from the
/// moment its connection would not take the whole of it at once. A viewer
/// that has not taken it by then is gone, however steadily it takes a
/// little.
const SEND_WAIT: Duration = Duration::from_millis(200);
/// How long reading one message of a viewer may take, all of it together,
/// once its first bytes are there. They are a few dozen bytes and arrive in
/// one piece; a viewer that sends them a byte at a time is dropped.
const ASK_WAIT: Duration = Duration::from_millis(50);
/// How often the viewers are looked up again in the list of paired devices
/// while they are sent to.
const RECHECK: Duration = Duration::from_millis(250);
/// How long ago a viewer may last have been found in the list of paired
/// devices, with the right to look, for a frame to be sent to it. Longer
/// ago than this, and it gets nothing until it has been found again.
const STALE: Duration = Duration::from_secs(1);
/// With no frame for this long, the host says that it has nothing new to
/// show, and repeats it this often.
const IDLE_EVERY: Duration = Duration::from_millis(500);
/// A viewer that still waits for a key frame after this long makes the host
/// ask for one again.
const KEY_AGAIN: Duration = Duration::from_millis(500);
/// How often a viewer's thread looks up when nothing happens.
const TICK: Duration = Duration::from_millis(100);
/// How long a frame waits for a viewer's connection while that viewer's own thread has it
/// for a moment. Longer than this, and the frame is not sent to that viewer.
const LINE_WAIT: Duration = Duration::from_micros(300);

/// Which of the paired devices that may look are shown the picture.
///
/// Being paired and allowed to look is checked underneath, as it always
/// was, for every connection and while a stream runs. This narrows it: a
/// session is shown to one device at a time, and whoever runs the session
/// says which.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admit {
    /// Every one of them that connects. What a host does that was never
    /// told otherwise.
    Paired,
    /// This one device: its id and its public key, as the list of paired
    /// devices has them. Both have to match.
    Only { id: String, public_key: String },
    /// Nobody.
    Nobody,
}

impl Admit {
    fn lets_in(&self, peer: &Peer) -> bool {
        match self {
            Admit::Paired => true,
            Admit::Only { id, public_key } => peer.id == *id && peer.public_key == *public_key,
            Admit::Nobody => false,
        }
    }
}

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
    /// Viewers the frame went to. A viewer that waits for a key frame is
    /// not sent the frames before it.
    pub viewers: u32,
    /// Of those, the viewers whose connection did not take all of the frame
    /// at once: the rest of it is on its way, and has left this thread's
    /// hands.
    pub queued: u32,
    /// Messages of the encrypted channel the frame took, for each viewer.
    pub messages: u32,
}

struct Shared {
    keys: LocalKeys,
    peers_path: PathBuf,
    info: StreamInfo,
    watchers: Mutex<Watchers>,
    /// Which paired devices are shown the picture. Taken after `watchers`
    /// where both are held, and never the other way round.
    admit: Mutex<Admit>,
    /// Callers that have not yet shown who they are and said what they
    /// want, and devices that are being shown the picture.
    pending: Arc<AtomicUsize>,
    serving: Arc<AtomicUsize>,
    seq: AtomicU64,
    /// Somebody needs a key frame, and the encoder has not been told yet.
    key_wanted: AtomicBool,
    /// What the cursor is, as the packet that says so, and the number of that packet: one
    /// more for every time the cursor is set. Nothing until it has been set.
    cursor: Mutex<Option<(u64, Arc<Vec<u8>>)>>,
    /// The number of the packet in `cursor`, to look at without taking it. 0: none.
    cursor_no: AtomicU64,
    /// Trial only: the text field, as `cursor`. Nothing until it has been set, and then
    /// nothing goes out on the frames' path but one comparison.
    field: Mutex<Option<(u64, Arc<Vec<u8>>)>>,
    field_no: AtomicU64,
    /// How long a caller has for the handshake and its first message.
    hello_wait_ms: AtomicU64,
    /// The picture is not offered: no caller is taken. Set while the session it belongs to
    /// is shown to nobody ([`Host::offer`]).
    withheld: AtomicBool,
    /// For tests: the viewers are not looked up while this is set.
    recheck_paused: AtomicBool,
    stop: AtomicBool,
    log: Log,
}

/// The viewers, and what is being sent to them. Held only for moments:
/// never while a file is read, and never while a connection is waited for.
struct Watchers {
    list: Vec<Arc<Watcher>>,
    /// The packet being sent, kept to spare an allocation per frame.
    packet: Vec<u8>,
}

/// One device that is being shown the picture.
struct Watcher {
    peer: Peer,
    /// When this device was last found in a list of paired devices that
    /// had just been read, with the right to look, on [`clock::now_ns`].
    /// Only a look-up of this very device moves it.
    confirmed_ns: AtomicU64,
    /// The stream is over; nothing more is sent, and the viewer's thread
    /// ends.
    closed: AtomicBool,
    /// Why, when this host ends the stream for a reason the viewer is to know (0: it is
    /// not told; otherwise [`farewell`] of the reason). A viewer that is told does not
    /// connect again.
    farewell: AtomicU8,
    /// A frame was not sent to this viewer since the last one it got.
    missed: AtomicBool,
    /// Part of what was sealed for the viewer waits for its connection to
    /// take it (`Line::unsent_since`, readable without the connection).
    unsent: AtomicBool,
    /// The viewer's thread has the connection for something that may take
    /// long: finishing a frame, or reading what the viewer says.
    ///
    /// The order is the same at every such place, and has to be: this is set BEFORE the
    /// connection (`line`) is taken, and cleared after it has been let go. A frame that
    /// finds the connection taken looks here and, when this is set, goes on without this
    /// viewer at once. The viewer's thread also takes the connection without setting
    /// this, for work that cannot wait on anything (sealing a small packet into memory
    /// and offering it to a connection that never waits); a frame gives that at most
    /// [`LINE_WAIT`], so that nothing rests on the work really being that short.
    busy: AtomicBool,
    /// When a frame last went to this viewer, on [`clock::now_ns`]. While frames go,
    /// what else the viewer is to be told goes behind a frame, on the frames' thread, and
    /// its own thread leaves the connection alone.
    last_frame_ns: AtomicU64,
    /// For tests: the viewer's thread takes the connection for this many milliseconds
    /// without saying that it will be long.
    #[cfg(test)]
    hold_ms: AtomicU64,
    /// For tests: after this many more packets sealed whole, the next is sealed but not put
    /// on the connection, as if the connection took only part of it. `u32::MAX`: never.
    #[cfg(test)]
    part_after: std::sync::atomic::AtomicU32,
    /// When something was last sent, on [`clock::now_ns`].
    last_sent_ns: AtomicU64,
    /// Which cursor this viewer was told last (its number), and when.
    cursor_told: AtomicU64,
    cursor_told_ns: AtomicU64,
    /// Which text field this viewer was told last, and when (trial only).
    field_told: AtomicU64,
    field_told_ns: AtomicU64,
    /// Makes the viewer's thread look up.
    wake: Wake,
    /// The connection, for waiting on it without holding `line`.
    fd: RawFd,
    line: Mutex<Line>,
    _place: Place,
}

/// A viewer's connection and where its stream stands.
struct Line {
    channel: Channel<Timed>,
    /// Since when the viewer has been waiting for a key frame: it cannot
    /// decode anything else. `None` once it has one.
    awaiting_key: Option<Instant>,
    /// Frames were held back from this viewer since the last one it got.
    skipped: bool,
    /// Since when a part of what was sealed for the viewer has been waiting
    /// for the connection to take it.
    unsent_since: Option<Instant>,
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

/// A connection by its number, for waiting on it.
struct Fd(RawFd);

impl AsRawFd for Fd {
    fn as_raw_fd(&self) -> RawFd {
        self.0
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

/// A write to a connection whose other end has gone must fail, not end the
/// program. Rust's own programs see to that; this library is also loaded
/// into programs that do not.
#[cfg(target_vendor = "apple")]
fn no_signal_on_a_closed_connection(stream: &TcpStream) {
    let on: libc::c_int = 1;
    // SAFETY: the option's value is one integer, and its size is given.
    unsafe {
        libc::setsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_NOSIGPIPE,
            (&on as *const libc::c_int).cast(),
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
}

/// Elsewhere every single write says so itself.
#[cfg(not(target_vendor = "apple"))]
fn no_signal_on_a_closed_connection(_stream: &TcpStream) {}

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
        Self::listen_admitting(addr, identity, peers_path, info, log, Admit::Paired)
    }

    /// The same, with who is shown the picture said before anybody can
    /// connect: a host that belongs to a session starts with
    /// [`Admit::Nobody`] and is told the device when there is one.
    pub fn listen_admitting(
        addr: SocketAddr,
        identity: &Identity,
        peers_path: PathBuf,
        info: StreamInfo,
        log: Log,
        admit: Admit,
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
            admit: Mutex::new(admit),
            pending: Arc::new(AtomicUsize::new(0)),
            serving: Arc::new(AtomicUsize::new(0)),
            seq: AtomicU64::new(0),
            key_wanted: AtomicBool::new(false),
            cursor: Mutex::new(None),
            cursor_no: AtomicU64::new(0),
            field: Mutex::new(None),
            field_no: AtomicU64::new(0),
            hello_wait_ms: AtomicU64::new(HELLO_WAIT.as_millis() as u64),
            withheld: AtomicBool::new(false),
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

    /// Says which paired devices are shown the picture from now on. A
    /// viewer that is not among them is cut off before this returns: no
    /// frame that is handed in afterwards goes to it. Whoever connects
    /// later and is not among them is refused.
    pub fn admit(&self, who: Admit) {
        let shared = &*self.shared;
        *shared.admit.lock().unwrap_or_else(|e| e.into_inner()) = who.clone();
        let mut watchers = shared.watchers();
        watchers.list.retain(|watcher| {
            who.lets_in(&watcher.peer) || {
                shared.close_saying(
                    watcher,
                    &"the picture is for another device now, or for none",
                    Ending::NotShown,
                );
                false
            }
        });
    }

    /// Devices that are being shown the picture now.
    pub fn viewers(&self) -> usize {
        let watchers = self.shared.watchers();
        let open = |watcher: &&Arc<Watcher>| !watcher.closed.load(Ordering::SeqCst);
        watchers.list.iter().filter(open).count()
    }

    /// Sends one encoded frame to every viewer that can decode it. Never
    /// waits for a viewer: what a connection does not take at once is
    /// finished by that viewer's own thread ([`Sent::queued`]).
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
            queued: 0,
            messages: wire::channel_messages(frame.len()),
        };
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
        for watcher in list.iter() {
            match shared.offer(watcher, packet, key_frame) {
                Offered::No => {}
                Offered::Written => sent.viewers += 1,
                Offered::Queued => {
                    sent.viewers += 1;
                    sent.queued += 1;
                }
            }
        }
        Ok(sent)
    }

    /// Offers the picture to paired devices (`true`, as a host does from the start), or
    /// stops offering it (`false`): every viewer is cut off at once, and no caller is taken
    /// until it is offered again. For a session that is shown to nobody for the time being:
    /// whoever was watching is let go, and nobody else can come in its place.
    pub fn offer(&self, offered: bool) {
        let shared = &*self.shared;
        shared.withheld.store(!offered, Ordering::SeqCst);
        if !offered {
            for watcher in shared.watchers().list.drain(..) {
                shared.close_saying(
                    &watcher,
                    &"the picture is no longer offered",
                    Ending::SetAside,
                );
            }
        }
    }

    /// Says what the pointer's cursor looks like from now on. The viewers draw it themselves,
    /// where their own pointers are; the frames are expected not to have it in them.
    ///
    /// The cursor is whatever an application on this host has handed in. One that may not
    /// travel is refused here and reaches nobody: an image of more than 256 pixels a side or
    /// of none, one whose bytes are not as many as its size says, a hot spot outside the
    /// image. A viewer is told the latest cursor only, at most once in a frame's interval,
    /// and never before a frame that is still on its way to it; a viewer that connects
    /// later is told what the cursor is then.
    pub fn set_cursor(&self, cursor: &Cursor) -> Result<()> {
        let shared = &*self.shared;
        let mut packet = Vec::new();
        wire::encode_cursor(cursor, shared.seq.load(Ordering::SeqCst), &mut packet)?;
        {
            let mut now = shared.cursor.lock().unwrap_or_else(|e| e.into_inner());
            let number = shared.cursor_no.load(Ordering::SeqCst) + 1;
            *now = Some((number, Arc::new(packet)));
            shared.cursor_no.store(number, Ordering::SeqCst);
        }
        // Each viewer's own thread tells it, when it is that viewer's turn.
        for watcher in shared.watchers().list.iter() {
            watcher.wake.wake();
        }
        Ok(())
    }

    /// Trial only: says what the text input field of the application with the keyboard is,
    /// and where its text cursor is, as [`Host::set_cursor`] says what the cursor is: the
    /// latest only, at most once in a frame's interval, behind frames, and to a viewer that
    /// connects later. Only viewers that may be sent the frames are told (it says where a
    /// person types). Nothing is sent until this is first called.
    pub fn set_text_field(&self, field: &TextField) -> Result<()> {
        let shared = &*self.shared;
        let mut packet = Vec::new();
        wire::encode_text_field(field, shared.seq.load(Ordering::SeqCst), &mut packet)?;
        {
            let mut now = shared.field.lock().unwrap_or_else(|e| e.into_inner());
            let number = shared.field_no.load(Ordering::SeqCst) + 1;
            *now = Some((number, Arc::new(packet)));
            shared.field_no.store(number, Ordering::SeqCst);
        }
        for watcher in shared.watchers().list.iter() {
            watcher.wake.wake();
        }
        Ok(())
    }

    /// Whether a key frame is needed: a viewer has connected, has asked for
    /// one, or was held back. True once for each time it is needed; the
    /// caller tells the encoder.
    pub fn key_frame_wanted(&self) -> bool {
        self.shared.key_wanted.swap(false, Ordering::SeqCst)
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

    /// Makes the thread of the viewer called `name` take that viewer's connection for
    /// `time` without saying that it will be long: what must never happen, to see what a
    /// frame does if it happens all the same.
    #[cfg(test)]
    pub(crate) fn hold_the_line_of(&self, name: &str, time: Duration) {
        for watcher in self.shared.watchers().list.iter() {
            if watcher.peer.name == name {
                watcher
                    .hold_ms
                    .store(time.as_millis() as u64, Ordering::SeqCst);
                watcher.wake.wake();
            }
        }
    }

    /// Makes the connection of the viewer called `name` take only part of the packet that
    /// comes after `whole` more packets that it takes whole.
    #[cfg(test)]
    pub(crate) fn part_after(&self, name: &str, whole: u32) {
        for watcher in self.shared.watchers().list.iter() {
            if watcher.peer.name == name {
                watcher.part_after.store(whole, Ordering::SeqCst);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn pending(&self) -> usize {
        self.shared.pending.load(Ordering::SeqCst)
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        // Every viewer's thread says that the stream is over, and ends.
        for watcher in self.shared.watchers().list.drain(..) {
            watcher.wake.wake();
        }
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
        let began = Instant::now();
        while self.shared.serving.load(Ordering::SeqCst) > 0
            && began.elapsed() < Duration::from_secs(1)
        {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// How a reason for ending a stream is kept in a viewer's record: never 0, which says
/// that the viewer is not told.
fn farewell(said: Ending) -> u8 {
    match said {
        Ending::Over => 1,
        Ending::NotShown => 2,
        Ending::SetAside => 3,
    }
}

fn said_in_farewell(kept: u8) -> Option<Ending> {
    match kept {
        1 => Some(Ending::Over),
        2 => Some(Ending::NotShown),
        3 => Some(Ending::SetAside),
        _ => None,
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

/// What became of a packet that was to be sealed for one viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sealed {
    /// Not sealed: the viewer may not be sent anything at the moment.
    Nothing,
    /// Sealed, and all of it on the wire.
    Whole,
    /// Sealed, and part of it left for the viewer's thread to finish.
    Part,
}

/// What became of a frame for one viewer.
enum Offered {
    /// Not sent: the viewer cannot have it, or cannot use it.
    No,
    /// Sealed and all of it on the wire.
    Written,
    /// Sealed, and part of it left for the viewer's thread to finish.
    Queued,
}

impl Watcher {
    fn line(&self) -> MutexGuard<'_, Line> {
        self.line.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether this device was found in the list of paired devices, with
    /// the right to look, recently enough for a frame to be sent to it.
    fn confirmed_lately(&self) -> bool {
        let confirmed = self.confirmed_ns.load(Ordering::SeqCst);
        clock::now_ns().saturating_sub(confirmed) <= STALE.as_nanos() as u64
    }

    /// The connection, for a moment. `None` while the viewer's thread has
    /// it for something that may take long: the frames wait for nobody.
    fn line_for_a_frame(&self) -> Option<MutexGuard<'_, Line>> {
        let began = Instant::now();
        loop {
            match self.line.try_lock() {
                Ok(line) => return Some(line),
                Err(TryLockError::Poisoned(e)) => return Some(e.into_inner()),
                // The viewer's own thread has it, for something that may take long.
                Err(TryLockError::WouldBlock) if self.busy.load(Ordering::SeqCst) => return None,
                // The viewer's own thread has it and has not said so: it is sealing a
                // small packet. That takes microseconds, and the frame waits them out
                // rather than be lost. It does not wait longer than that, whatever the
                // other thread is doing.
                Err(TryLockError::WouldBlock) if began.elapsed() < LINE_WAIT => {
                    std::hint::spin_loop()
                }
                Err(TryLockError::WouldBlock) => return None,
            }
        }
    }
}

impl Shared {
    fn watchers(&self) -> MutexGuard<'_, Watchers> {
        self.watchers.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether `peer` is among the devices that are shown the picture.
    fn admits(&self, peer: &Peer) -> bool {
        self.admit
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .lets_in(peer)
    }

    /// Ends a viewer's stream: nothing more is sent to it, and its thread
    /// closes the connection.
    /// Ends a viewer's stream as [`Shared::close`] does, and has its thread tell the
    /// viewer why before the connection is closed, so that the viewer does not come
    /// back by itself.
    fn close_saying(&self, watcher: &Watcher, why: &dyn std::fmt::Display, said: Ending) {
        // Set first: the thread that finds the stream closed looks for it.
        watcher.farewell.store(farewell(said), Ordering::SeqCst);
        self.close(watcher, why);
    }

    fn close(&self, watcher: &Watcher, why: &dyn std::fmt::Display) {
        if !watcher.closed.swap(true, Ordering::SeqCst) {
            (self.log)(format!(
                "stopped showing the picture to {}: {why}",
                watcher.peer.name
            ));
            watcher.wake.wake();
        }
    }

    /// Seals one packet for one viewer and puts it on its connection as far as the
    /// connection takes it now. Waits for nothing.
    ///
    /// The one place anything is sealed for a viewer once its stream runs: a frame, what
    /// the cursor is, that the picture stands still, that the stream is over. So it is
    /// also the one gate. Nothing is sealed for a viewer whose stream has been closed, or
    /// that was not found in the list of paired devices, with the right to look, within
    /// the last second: such a viewer gets `Sealed::Nothing`, whatever the packet is.
    ///
    /// One packet passes a closed stream, and only through [`Shared::say_why`]: the word
    /// that the stream is over and why. It carries nothing of the picture, and it too is
    /// sealed only for a device that was found paired and allowed within the last second.
    fn seal(&self, watcher: &Watcher, line: &mut Line, packet: &[u8]) -> Result<Sealed> {
        if watcher.closed.load(Ordering::SeqCst) {
            return Ok(Sealed::Nothing);
        }
        self.seal_for_the_confirmed(watcher, line, packet)
    }

    fn seal_for_the_confirmed(
        &self,
        watcher: &Watcher,
        line: &mut Line,
        packet: &[u8],
    ) -> Result<Sealed> {
        if !watcher.confirmed_lately() {
            return Ok(Sealed::Nothing);
        }
        // Nothing is sealed behind a packet whose rest still waits for the connection:
        // it would wait with it, and move that packet's deadline. Every caller looks
        // first; a debug build, which the tests are, checks that they did.
        debug_assert!(
            line.unsent_since.is_none(),
            "a packet was sealed behind one that still waits for the connection"
        );
        // Sealing writes into memory; only `push` touches the connection.
        line.channel.send(packet)?;
        #[cfg(test)]
        {
            let left = watcher.part_after.load(Ordering::SeqCst);
            if left != u32::MAX {
                if left == 0 {
                    // Not pushed: it waits as the rest of a packet does.
                    watcher.part_after.store(u32::MAX, Ordering::SeqCst);
                    line.unsent_since = Some(Instant::now());
                    watcher.unsent.store(true, Ordering::SeqCst);
                    watcher.wake.wake();
                    return Ok(Sealed::Part);
                }
                watcher.part_after.store(left - 1, Ordering::SeqCst);
            }
        }
        if line.channel.stream().push()? {
            return Ok(Sealed::Whole);
        }
        line.unsent_since = Some(Instant::now());
        watcher.unsent.store(true, Ordering::SeqCst);
        watcher.wake.wake();
        Ok(Sealed::Part)
    }

    /// Seals one frame for one viewer and puts it on its connection as far
    /// as the connection takes it now. Waits for nothing.
    fn offer(&self, watcher: &Watcher, packet: &mut [u8], key_frame: bool) -> Offered {
        if watcher.closed.load(Ordering::SeqCst) {
            return Offered::No;
        }
        // Whether this device may still look is known only as of its own
        // last look-up. Too long ago, and it is sent nothing.
        if !watcher.confirmed_lately() {
            watcher.missed.store(true, Ordering::SeqCst);
            return Offered::No;
        }
        let Some(mut line) = watcher.line_for_a_frame() else {
            watcher.missed.store(true, Ordering::SeqCst);
            return Offered::No;
        };
        if line.unsent_since.is_some() {
            // The connection has not even taken the frame before this one.
            watcher.missed.store(true, Ordering::SeqCst);
            return Offered::No;
        }
        if watcher.missed.swap(false, Ordering::SeqCst) {
            // It was not sent a frame, and cannot decode the ones after it
            // until a key frame comes.
            line.skipped = true;
            if line.awaiting_key.is_none() {
                line.awaiting_key = Some(Instant::now());
                self.key_wanted.store(true, Ordering::SeqCst);
            }
        }
        if let (Some(since), false) = (line.awaiting_key, key_frame) {
            line.skipped = true;
            if since.elapsed() >= KEY_AGAIN {
                line.awaiting_key = Some(Instant::now());
                self.key_wanted.store(true, Ordering::SeqCst);
            }
            return Offered::No;
        }
        wire::set_discontinuity(packet, line.skipped);
        let sealed = match self.seal(watcher, &mut line, packet) {
            Ok(Sealed::Nothing) => {
                // No longer confirmed, between the look above and here.
                watcher.missed.store(true, Ordering::SeqCst);
                return Offered::No;
            }
            Ok(sealed) => sealed,
            Err(err) => {
                drop(line);
                self.close(watcher, &err);
                return Offered::No;
            }
        };
        line.awaiting_key = None;
        line.skipped = false;
        let at = clock::now_ns();
        watcher.last_sent_ns.store(at, Ordering::SeqCst);
        watcher.last_frame_ns.store(at, Ordering::SeqCst);
        if sealed == Sealed::Part {
            return Offered::Queued;
        }
        // The text field goes behind the frame (trial only; never set, nothing happens here),
        // before the cursor: it is small, and a large cursor that changes with every frame
        // would otherwise keep it waiting.
        if self.field_due(watcher) == Some(Duration::ZERO) {
            if let Err(err) = self.tell_field(watcher, &mut line) {
                drop(line);
                self.close(watcher, &err);
                return Offered::Written;
            }
        }
        // The cursor goes behind the frame, never in front of it, and only on a connection
        // with nothing left half sent (the field may have gone out only in part).
        if line.unsent_since.is_none() && self.cursor_due(watcher) == Some(Duration::ZERO) {
            if let Err(err) = self.tell_cursor(watcher, &mut line) {
                drop(line);
                self.close(watcher, &err);
            }
        }
        Offered::Written
    }

    /// How long until this viewer may be told what the cursor is: not at all (`None`) when
    /// it has been told the latest one, at once when a frame's interval has passed since
    /// it was last told one. An application that changes its cursor with every frame
    /// costs a viewer one small packet per frame at most, and only ever the latest.
    fn cursor_due(&self, watcher: &Watcher) -> Option<Duration> {
        if watcher.cursor_told.load(Ordering::SeqCst) == self.cursor_no.load(Ordering::SeqCst) {
            return None;
        }
        let since = clock::now_ns().saturating_sub(watcher.cursor_told_ns.load(Ordering::SeqCst));
        Some(Duration::from_nanos(
            self.frame_interval_ns().saturating_sub(since),
        ))
    }

    /// As `cursor_due`, for the text field.
    fn field_due(&self, watcher: &Watcher) -> Option<Duration> {
        if watcher.field_told.load(Ordering::SeqCst) == self.field_no.load(Ordering::SeqCst) {
            return None;
        }
        let since = clock::now_ns().saturating_sub(watcher.field_told_ns.load(Ordering::SeqCst));
        Some(Duration::from_nanos(
            self.frame_interval_ns().saturating_sub(since),
        ))
    }

    /// As `tell_cursor`, for the text field.
    fn tell_field(&self, watcher: &Watcher, line: &mut Line) -> Result<()> {
        let now = self.field.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let Some((number, packet)) = now else {
            return Ok(());
        };
        if self.seal(watcher, line, &packet)? == Sealed::Nothing {
            return Ok(());
        }
        let at = clock::now_ns();
        watcher.field_told.store(number, Ordering::SeqCst);
        watcher.field_told_ns.store(at, Ordering::SeqCst);
        watcher.last_sent_ns.store(at, Ordering::SeqCst);
        Ok(())
    }

    /// How far apart frames are at the frame rate the viewers were told.
    fn frame_interval_ns(&self) -> u64 {
        1_000_000_000 / u64::from(self.info.fps.clamp(1, 1000))
    }

    /// Whether frames are going to this viewer at the moment: one went within the last
    /// three frame intervals.
    fn frames_are_going(&self, watcher: &Watcher) -> bool {
        let since = clock::now_ns().saturating_sub(watcher.last_frame_ns.load(Ordering::SeqCst));
        since < 3 * self.frame_interval_ns()
    }

    /// Seals the latest cursor for one viewer and puts it on its connection, behind
    /// everything that was sealed for it before. Only called while nothing of an earlier
    /// packet is waiting for the connection.
    fn tell_cursor(&self, watcher: &Watcher, line: &mut Line) -> Result<()> {
        let now = self
            .cursor
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let Some((number, packet)) = now else {
            return Ok(());
        };
        // A viewer that may not be sent frames is not told the cursor either: it stays
        // due, and is told once the viewer has been confirmed again.
        if self.seal(watcher, line, &packet)? == Sealed::Nothing {
            return Ok(());
        }
        let at = clock::now_ns();
        watcher.cursor_told.store(number, Ordering::SeqCst);
        watcher.cursor_told_ns.store(at, Ordering::SeqCst);
        watcher.last_sent_ns.store(at, Ordering::SeqCst);
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
                if let Err(err) = shared.clone().serve(stream, pending) {
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

    /// The first exchange with a caller; when it is a paired device that
    /// may look, the rest of this thread's life is that viewer's.
    fn serve(self: Arc<Self>, stream: TcpStream, pending: Place) -> Result<()> {
        // While the picture is not offered, a caller is not even answered.
        if self.withheld.load(Ordering::SeqCst) {
            return Ok(());
        }
        // On some systems a connection takes over the listener's way of not
        // waiting; this one waits, until its deadline.
        stream.set_nonblocking(false)?;
        // A frame leaves when it is written, not when more has piled up.
        stream.set_nodelay(true)?;
        no_signal_on_a_closed_connection(&stream);
        let fd = stream.as_raw_fd();
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
        // Paired and allowed to look is not all: the picture of a session
        // is for the device that is shown the session.
        if !self.admits(&peer) {
            (self.log)(format!(
                "refused to show the picture to {}: it is for another device, or for none",
                peer.name
            ));
            return refuse(
                &mut channel,
                ErrorCode::NotShown,
                "the host shows this picture to another device, or to none",
            );
        }
        // A paired device that may look, and has said what it wants: it
        // moves from the callers to the viewers. One device has one stream.
        drop(pending);
        let came_back = self.close_earlier(&peer);
        let mut place = Place::take(&self.serving, MAX_STREAMS);
        // The place of its earlier connection is free a moment after that
        // connection was told to close.
        let began = Instant::now();
        while place.is_none() && came_back && began.elapsed() < ASK_WAIT * 4 {
            std::thread::sleep(Duration::from_millis(2));
            place = Place::take(&self.serving, MAX_STREAMS);
        }
        let Some(place) = place else {
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
        // No deadline for a stream, and from here on nothing waits for this
        // connection unless it is told how long.
        channel.stream().hold()?;

        let watcher = Arc::new(Watcher {
            peer,
            // What was read for this caller says something about this
            // caller, and about nobody else.
            confirmed_ns: AtomicU64::new(read_at),
            closed: AtomicBool::new(false),
            farewell: AtomicU8::new(0),
            missed: AtomicBool::new(false),
            unsent: AtomicBool::new(false),
            busy: AtomicBool::new(false),
            last_frame_ns: AtomicU64::new(0),
            #[cfg(test)]
            hold_ms: AtomicU64::new(0),
            #[cfg(test)]
            part_after: std::sync::atomic::AtomicU32::new(u32::MAX),
            last_sent_ns: AtomicU64::new(clock::now_ns()),
            cursor_told: AtomicU64::new(0),
            cursor_told_ns: AtomicU64::new(0),
            field_told: AtomicU64::new(0),
            field_told_ns: AtomicU64::new(0),
            wake: Wake::new()?,
            fd,
            line: Mutex::new(Line {
                channel,
                awaiting_key: Some(Instant::now()),
                skipped: false,
                unsent_since: None,
            }),
            _place: place,
        });
        self.close_earlier(&watcher.peer);
        {
            let mut watchers = self.watchers();
            // Looked at with the viewers in hand, as `offer` empties the list with them in
            // hand: a caller that was on its way in when the offer ended does not get in.
            if self.stop.load(Ordering::SeqCst) || self.withheld.load(Ordering::SeqCst) {
                return Ok(());
            }
            // Who is shown the picture may have been said again while this
            // device was being let in.
            if !self.admits(&watcher.peer) {
                return Ok(());
            }
            (self.log)(format!("showing the picture to {}", watcher.peer.name));
            watchers.list.push(watcher.clone());
            // A viewer can only begin with a key frame, and is not made to ask. Said
            // with the viewers still in hand: whoever finds this viewer among them (a
            // frame, or a question how many there are) finds the key frame asked for.
            self.key_wanted.store(true, Ordering::SeqCst);
        }
        self.attend(&watcher);
        self.watchers()
            .list
            .retain(|other| !Arc::ptr_eq(other, &watcher));
        Ok(())
    }

    /// Closes the connection a device already has, when it connects again:
    /// a viewer that was stopped and started must not find its place taken
    /// by what it left behind.
    fn close_earlier(&self, peer: &Peer) -> bool {
        let mut closed = false;
        self.watchers().list.retain(|watcher| {
            let same = watcher.peer.id == peer.id;
            if same {
                closed = true;
                self.close(
                    watcher,
                    &"it connected again; its earlier connection is closed",
                );
            }
            !same
        });
        closed
    }

    /// Tells a viewer that its stream is over and why, as the last thing on its
    /// connection. Best effort: the viewer may be gone already, and a viewer whose
    /// connection still holds part of a frame is not told (the word would have to wait
    /// behind the frame). One that is not told connects again and is answered then.
    fn say_why(&self, watcher: &Watcher, said: Ending, packet: &mut Vec<u8>) {
        // This may wait for the connection, so it is said first, as everywhere.
        watcher.busy.store(true, Ordering::SeqCst);
        let mut line = watcher.line();
        if line.unsent_since.is_none() {
            wire::encode_end(said, self.seq.load(Ordering::SeqCst), packet);
            if let Ok(Sealed::Part) = self.seal_for_the_confirmed(watcher, &mut line, packet) {
                let _ = line.channel.stream().push_by(Instant::now() + ASK_WAIT);
            }
        }
        drop(line);
        watcher.busy.store(false, Ordering::SeqCst);
    }

    /// A viewer's thread, for as long as its stream lasts: reads what the
    /// viewer says, finishes what its connection did not take at once, and
    /// says that the picture stands still when it does.
    fn attend(&self, watcher: &Watcher) {
        let mut packet = Vec::new();
        loop {
            if self.stop.load(Ordering::SeqCst) {
                return self.say_why(watcher, Ending::Over, &mut packet);
            }
            if watcher.closed.load(Ordering::SeqCst) {
                // Closed for a reason the viewer is to know: it is told, so that it does
                // not connect again. Closed for any other reason (not paired any more, a
                // connection that failed), it is told nothing.
                if let Some(said) = said_in_farewell(watcher.farewell.load(Ordering::SeqCst)) {
                    self.say_why(watcher, said, &mut packet);
                }
                return;
            }
            #[cfg(test)]
            {
                let hold = watcher.hold_ms.swap(0, Ordering::SeqCst);
                if hold > 0 {
                    let _line = watcher.line();
                    std::thread::sleep(Duration::from_millis(hold));
                }
            }
            if let Err(err) = self.finish_what_was_begun(watcher) {
                return self.close(watcher, &err);
            }
            // What the cursor is, when this viewer has not been told the latest and may
            // be told again; otherwise this thread looks up when it may.
            let mut tick = TICK;
            match self.cursor_due(watcher) {
                // While frames go to this viewer the cursor goes behind the next of
                // them, on their thread: this thread keeps off the connection, so that
                // no frame finds it taken.
                Some(_) if self.frames_are_going(watcher) => {
                    tick = tick.min(Duration::from_nanos(self.frame_interval_ns()));
                }
                Some(Duration::ZERO) => {
                    let mut line = watcher.line();
                    // Looked at again now that the connection is in hand: a frame may
                    // have gone out meanwhile, and the cursor behind it.
                    let still_due = self.cursor_due(watcher) == Some(Duration::ZERO);
                    if still_due && line.unsent_since.is_none() {
                        if let Err(err) = self.tell_cursor(watcher, &mut line) {
                            drop(line);
                            return self.close(watcher, &err);
                        }
                    }
                }
                Some(wait) => tick = tick.min(wait),
                None => {}
            }
            // The text field the same way (trial only).
            match self.field_due(watcher) {
                Some(_) if self.frames_are_going(watcher) => {
                    tick = tick.min(Duration::from_nanos(self.frame_interval_ns()));
                }
                Some(Duration::ZERO) => {
                    let mut line = watcher.line();
                    let still_due = self.field_due(watcher) == Some(Duration::ZERO);
                    if still_due && line.unsent_since.is_none() {
                        if let Err(err) = self.tell_field(watcher, &mut line) {
                            drop(line);
                            return self.close(watcher, &err);
                        }
                    }
                }
                Some(wait) => tick = tick.min(wait),
                None => {}
            }
            match wait_for(&Fd(watcher.fd), Some(&watcher.wake), tick) {
                Ok(Ready::Woken) => {
                    watcher.wake.clear();
                    continue;
                }
                Ok(Ready::Connection) => {
                    if let Err(err) = self.read_ask(watcher) {
                        return self.close(watcher, &err);
                    }
                }
                Ok(Ready::Neither) => {}
                Err(err) => return self.close(watcher, &err),
            }
            let quiet = clock::now_ns().saturating_sub(watcher.last_sent_ns.load(Ordering::SeqCst))
                >= IDLE_EVERY.as_nanos() as u64;
            if quiet {
                // A picture that stands still is not a broken connection.
                let mut line = watcher.line();
                if line.unsent_since.is_none() {
                    let seq = self.seq.load(Ordering::SeqCst);
                    wire::encode(&Header::without_frame(Kind::Idle, seq), &[], &mut packet);
                    if let Err(err) = self.seal(watcher, &mut line, &packet) {
                        drop(line);
                        return self.close(watcher, &err);
                    }
                    watcher
                        .last_sent_ns
                        .store(clock::now_ns(), Ordering::SeqCst);
                }
            }
        }
    }

    /// Puts the rest of what was sealed for the viewer on the wire: all of
    /// it within [`SEND_WAIT`] of when the connection first would not take
    /// it, however little the viewer takes at a time.
    fn finish_what_was_begun(&self, watcher: &Watcher) -> Result<()> {
        // Nearly always there is nothing to finish, and then the connection
        // is not touched: a frame that comes at this moment must find it
        // free.
        if !watcher.unsent.load(Ordering::SeqCst) {
            return Ok(());
        }
        // Said before the connection is taken, so that a frame that comes
        // meanwhile does not wait for it.
        watcher.busy.store(true, Ordering::SeqCst);
        let mut line = watcher.line();
        let result = match line.unsent_since {
            Some(since) => line.channel.stream().push_by(since + SEND_WAIT),
            None => Ok(()),
        };
        if result.is_ok() {
            line.unsent_since = None;
            watcher.unsent.store(false, Ordering::SeqCst);
        }
        drop(line);
        watcher.busy.store(false, Ordering::SeqCst);
        Ok(result?)
    }

    /// Reads one message of the viewer, whose first bytes are there (or
    /// the viewer has hung up). The rest has to be there within a short
    /// time, all of it together.
    fn read_ask(&self, watcher: &Watcher) -> Result<()> {
        watcher.busy.store(true, Ordering::SeqCst);
        let mut line = watcher.line();
        line.channel.stream().limit(Instant::now() + ASK_WAIT);
        let message = line.channel.recv();
        line.channel.stream().unlimit();
        drop(line);
        watcher.busy.store(false, Ordering::SeqCst);
        let message = message?;
        if message.len() > wire::MAX_ASK_BYTES {
            return Err(Error::Malformed);
        }
        // What this version does not know is passed over.
        if let Ok(Ask::KeyFrame) = serde_json::from_slice::<Ask>(&message) {
            self.key_wanted.store(true, Ordering::SeqCst);
        }
        Ok(())
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
            // The file is read with the viewers let go: frames go out
            // meanwhile.
            let peers = PeerStore::open(&self.peers_path);
            let read_at = clock::now_ns();
            let mut watchers = self.watchers();
            let Ok(peers) = peers else {
                for watcher in watchers.list.drain(..) {
                    self.close(
                        &watcher,
                        &"the list of paired devices cannot be read, and then nothing is sent",
                    );
                }
                continue;
            };
            watchers.list.retain(|watcher| {
                if may_still_view(&peers, &watcher.peer) {
                    // This device, in a list read just now.
                    watcher.confirmed_ns.fetch_max(read_at, Ordering::SeqCst);
                    return true;
                }
                self.close(
                    watcher,
                    &"it is no longer paired, or may no longer look at this host",
                );
                false
            });
        }
    }
}
