//! The two ends of an audio connection.
//!
//! A source listens ([`Source`]); an exit connects ([`subscribe`]). Both
//! run the handshake of the control channel on their connection
//! (`istesso_link::Channel`, ADR 0007), so each end knows which paired
//! device it is talking to and nobody else can read or alter the audio. No
//! cryptographic step is added here.
//!
//! On top of that a source checks the `audio` right of the device, at the
//! start and again at most a second apart while it sends: taking the right
//! away, or removing the pairing, stops a stream that is running.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use istesso_link::{Channel, Error as LinkError, LocalKeys};
use istesso_pair::{Identity, Peer, PeerStore, Permission};
use serde::Serialize;

use crate::clock;
use crate::fifo::Writer;
use crate::ledger::Entry;
use crate::source::Packetizer;
use crate::wire::{self, ErrorCode, Format, Header, Hello, Kind, Reply, WireError};
use crate::{Error, Result};

/// Connections a source serves at the same time; further ones are dropped.
const MAX_CONNECTIONS: usize = 16;
/// How long a source waits for the handshake and the first message.
const HELLO_WAIT: Duration = Duration::from_secs(5);
/// How long a source lets a single write to a listener take. A listener
/// that takes nothing for this long is gone.
const SEND_WAIT: Duration = Duration::from_secs(5);
/// How often a listener is looked up again in the list of paired devices
/// while it is being sent to.
const RECHECK: Duration = Duration::from_secs(1);
/// With no audio for this long, the source says that it has nothing to
/// play, and repeats it this often.
const IDLE_AFTER: Duration = Duration::from_millis(200);
const IDLE_EVERY: Duration = Duration::from_millis(500);
/// How long the sending thread sleeps when there is nothing to send.
const POLL: Duration = Duration::from_millis(1);
/// How often captured audio is thrown away while nobody listens.
const IDLE_DRAIN: Duration = Duration::from_millis(50);

/// Receives one line for each thing worth telling the person who runs the
/// source.
pub type Log = Arc<dyn Fn(String) + Send + Sync>;

/// The form of a source's stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StreamInfo {
    pub stream_id: String,
    pub sample_rate: u32,
    pub channels: u16,
    /// Frames in a typical packet.
    pub packet_frames: u32,
}

impl StreamInfo {
    /// A stream with a fresh, random identifier.
    pub fn new(sample_rate: u32, channels: u16, packet_frames: u32) -> Self {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(clock::now_ns());
        StreamInfo {
            stream_id: format!("{:016x}", hasher.finish()),
            sample_rate,
            channels,
            packet_frames,
        }
    }
}

/// Whether the device is, at this moment, still paired and still allowed
/// to have this host's sound: read from the list on disk, not remembered
/// from when the stream began.
pub fn may_still_hear(peers_path: &Path, peer: &Peer) -> bool {
    PeerStore::open(peers_path).is_ok_and(|peers| {
        peers.peers().iter().any(|known| {
            known.id == peer.id
                && known.public_key == peer.public_key
                && known.permissions.contains(&Permission::Audio)
        })
    })
}

/// A host that offers its sound to paired devices.
pub struct Source {
    listener: TcpListener,
    shared: Arc<Shared>,
}

struct Shared {
    keys: LocalKeys,
    peers_path: PathBuf,
    info: StreamInfo,
    packetizer: Mutex<Packetizer>,
    /// Counts the listeners. Only the newest one is sent to.
    listener_no: AtomicU64,
    /// Streams that are being sent now.
    streams: AtomicUsize,
    connections: AtomicUsize,
    sent: Option<Arc<Mutex<Vec<Entry>>>>,
    stop: AtomicBool,
    log: Log,
}

impl Source {
    /// Starts listening. `peers_path` is the list of paired devices; it is
    /// read afresh for every connection and while a stream runs. `sent`,
    /// when given, collects a line for every packet that is sent.
    pub fn bind(
        addr: SocketAddr,
        identity: &Identity,
        peers_path: PathBuf,
        info: StreamInfo,
        packetizer: Packetizer,
        sent: Option<Arc<Mutex<Vec<Entry>>>>,
        log: Log,
    ) -> Result<Self> {
        let keys = LocalKeys::new(identity)?;
        let listener = TcpListener::bind(addr)?;
        Ok(Source {
            listener,
            shared: Arc::new(Shared {
                keys,
                peers_path,
                info,
                packetizer: Mutex::new(packetizer),
                listener_no: AtomicU64::new(0),
                streams: AtomicUsize::new(0),
                connections: AtomicUsize::new(0),
                sent,
                stop: AtomicBool::new(false),
                log,
            }),
        })
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// A switch that ends the streams of this source when set.
    pub fn stopper(&self) -> Stopper {
        Stopper(self.shared.clone())
    }

    /// Serves paired devices until the process ends.
    pub fn run(self) {
        // While nobody listens, what is captured is not kept: it would only
        // fill the ring and be let go when a listener comes.
        let shared = self.shared.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(IDLE_DRAIN);
            if shared.streams.load(Ordering::SeqCst) == 0 {
                shared.packetizer().flush();
            }
        });
        for stream in self.listener.incoming() {
            let Ok(stream) = stream else {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            };
            let shared = self.shared.clone();
            if shared.connections.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                shared.connections.fetch_sub(1, Ordering::SeqCst);
                continue;
            }
            std::thread::spawn(move || {
                if let Err(err) = shared.serve(stream) {
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
                shared.connections.fetch_sub(1, Ordering::SeqCst);
            });
        }
    }
}

/// A handle on a [`Source`] that is running: ends its streams, and reads
/// its counters.
pub struct Stopper(Arc<Shared>);

impl Stopper {
    pub fn stop(&self) {
        self.0.stop.store(true, Ordering::SeqCst);
    }

    /// What the source's side has sent and let go so far.
    pub fn stats(&self) -> crate::source::SourceStats {
        self.0.packetizer().stats()
    }
}

fn reply(channel: &mut Channel<TcpStream>, reply: &Reply) -> Result<()> {
    let bytes = serde_json::to_vec(reply).map_err(|_| Error::Malformed)?;
    Ok(channel.send(&bytes)?)
}

impl Shared {
    fn packetizer(&self) -> std::sync::MutexGuard<'_, Packetizer> {
        self.packetizer.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn serve(&self, stream: TcpStream) -> Result<()> {
        // Small packets must leave at once, not wait to be bundled.
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(HELLO_WAIT))?;
        stream.set_write_timeout(Some(SEND_WAIT))?;

        // The list of paired devices is read for every caller: a pairing
        // that was removed a moment ago must not be honoured any more.
        let peers = PeerStore::open(&self.peers_path)?;
        let mut found: Option<Peer> = None;
        let mut channel = Channel::respond(stream, &self.keys, |key| {
            found = peers.find_by_key(key).cloned();
            found.is_some()
        })?;
        let peer = found.ok_or(LinkError::UnknownPeer)?;

        let first = channel.recv()?;
        let arrived = clock::now_ns();
        let Ok(hello) = serde_json::from_slice::<Hello>(&first) else {
            return reply(
                &mut channel,
                &Reply::Error {
                    code: ErrorCode::Unsupported,
                    message: "this version of istesso does not know that request".into(),
                },
            );
        };
        if !peer.permissions.contains(&Permission::Audio) {
            (self.log)(format!(
                "refused to send sound to {}: it is paired, but may not have this host's sound",
                peer.name
            ));
            return reply(
                &mut channel,
                &Reply::Error {
                    code: ErrorCode::Forbidden,
                    message: "this device may not have the host's sound".into(),
                },
            );
        }
        match hello {
            Hello::Time { t1 } => self.tell_time(channel, t1, arrived),
            Hello::Subscribe { .. } => self.send_stream(channel, &peer),
        }
    }

    /// Answers questions about this machine's clock until the caller is
    /// done.
    fn tell_time(&self, mut channel: Channel<TcpStream>, t1: u64, arrived: u64) -> Result<()> {
        let (mut t1, mut t2) = (t1, arrived);
        loop {
            reply(
                &mut channel,
                &Reply::Time {
                    t1,
                    t2,
                    t3: clock::now_ns(),
                },
            )?;
            let next = match channel.recv() {
                Ok(next) => next,
                Err(LinkError::Closed | LinkError::Timeout) => return Ok(()),
                Err(err) => return Err(err.into()),
            };
            t2 = clock::now_ns();
            match serde_json::from_slice::<Hello>(&next) {
                Ok(Hello::Time { t1: again }) => t1 = again,
                _ => return Ok(()),
            }
        }
    }

    fn send_stream(&self, mut channel: Channel<TcpStream>, peer: &Peer) -> Result<()> {
        let number = self.listener_no.fetch_add(1, Ordering::SeqCst) + 1;
        reply(
            &mut channel,
            &Reply::Stream {
                version: u32::from(wire::VERSION),
                stream_id: self.info.stream_id.clone(),
                sample_rate: self.info.sample_rate,
                channels: self.info.channels,
                format: Format::F32le,
                packet_frames: self.info.packet_frames,
            },
        )?;

        (self.log)(format!("sending sound to {}", peer.name));
        // A listener that has just arrived is given what is captured from
        // now on, not what piled up before it came.
        self.streams.fetch_add(1, Ordering::SeqCst);
        self.packetizer().flush();
        let result = self.send_packets(&mut channel, peer, number);
        self.streams.fetch_sub(1, Ordering::SeqCst);
        result
    }

    fn send_packets(
        &self,
        channel: &mut Channel<TcpStream>,
        peer: &Peer,
        number: u64,
    ) -> Result<()> {
        let mut packet = Vec::new();
        let mut checked = Instant::now();
        let mut last_audio = Instant::now();
        let mut last_idle: Option<Instant> = None;
        let why = loop {
            if self.stop.load(Ordering::SeqCst) {
                break "the source is stopping";
            }
            if self.listener_no.load(Ordering::SeqCst) != number {
                break "another device asked for the sound";
            }
            if checked.elapsed() >= RECHECK {
                checked = Instant::now();
                if !may_still_hear(&self.peers_path, peer) {
                    break "it is no longer allowed to have this host's sound";
                }
            }
            // The packetizer is held only while a packet is put together,
            // never while it is sent: a listener that takes nothing must
            // not keep the next one waiting.
            let entry = self.packetizer().next_packet().map(|(header, samples)| {
                wire::encode(&header, samples, &mut packet);
                Entry {
                    seq: header.seq,
                    position: header.position,
                    frames: (samples.len() / usize::from(self.info.channels)) as u32,
                    digest: if self.sent.is_some() {
                        wire::digest(samples)
                    } else {
                        0
                    },
                }
            });
            match entry {
                Some(entry) => {
                    if let Some(sent) = &self.sent {
                        sent.lock().unwrap_or_else(|e| e.into_inner()).push(entry);
                    }
                    channel.send(&packet)?;
                    last_audio = Instant::now();
                    last_idle = None;
                }
                None => {
                    let quiet = last_audio.elapsed() >= IDLE_AFTER;
                    if quiet && last_idle.is_none_or(|at| at.elapsed() >= IDLE_EVERY) {
                        let header = self.packetizer().idle(clock::now_ns());
                        wire::encode(&header, &[], &mut packet);
                        channel.send(&packet)?;
                        last_idle = Some(Instant::now());
                    }
                    std::thread::sleep(POLL);
                }
            }
        };
        (self.log)(format!("stopped sending sound to {}: {why}", peer.name));
        let header = self.packetizer().end(clock::now_ns());
        wire::encode(&header, &[], &mut packet);
        // The listener may be gone already.
        let _ = channel.send(&packet);
        Ok(())
    }
}

/// Connects to `addr` and checks that a paired device answers there. With
/// `expected` set to a peer id, any other device is refused even if it is
/// paired.
fn open(
    addr: SocketAddr,
    keys: &LocalKeys,
    peers: &PeerStore,
    expected: Option<&str>,
    timeout: Duration,
) -> Result<(Channel<TcpStream>, Peer)> {
    let stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
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
    let peer = found.ok_or(LinkError::UnknownPeer)?;
    Ok((channel, peer))
}

/// An exit's connection to one source.
pub struct Subscription {
    channel: Channel<TcpStream>,
    peer: Peer,
    info: StreamInfo,
}

/// Asks the source at `addr` for its sound.
pub fn subscribe(
    addr: SocketAddr,
    keys: &LocalKeys,
    peers: &PeerStore,
    expected: Option<&str>,
    timeout: Duration,
) -> Result<Subscription> {
    let (mut channel, peer) = open(addr, keys, peers, expected, timeout)?;
    let hello = Hello::Subscribe {
        version: u32::from(wire::VERSION),
    };
    channel.send(&serde_json::to_vec(&hello).map_err(|_| Error::Malformed)?)?;
    let answer = channel.recv()?;
    match serde_json::from_slice::<Reply>(&answer).map_err(|_| Error::Malformed)? {
        Reply::Stream {
            stream_id,
            sample_rate,
            channels,
            format,
            packet_frames,
            ..
        } => {
            if format != Format::F32le {
                return Err(Error::Unsupported("an unknown sample format".into()));
            }
            if channels == 0 || channels > 8 || !(8_000..=384_000).contains(&sample_rate) {
                return Err(Error::Unsupported(format!(
                    "{channels} channels at {sample_rate} Hz"
                )));
            }
            Ok(Subscription {
                channel,
                peer,
                info: StreamInfo {
                    stream_id,
                    sample_rate,
                    channels,
                    packet_frames,
                },
            })
        }
        Reply::Error {
            code: ErrorCode::Forbidden,
            ..
        } => Err(Error::Forbidden),
        Reply::Error { message, .. } => Err(Error::Refused(message)),
        Reply::Time { .. } => Err(Error::Malformed),
    }
}

impl Subscription {
    pub fn info(&self) -> &StreamInfo {
        &self.info
    }

    /// The paired device that is the source.
    pub fn peer(&self) -> &Peer {
        &self.peer
    }

    /// Changes how long [`Subscription::next_packet`] waits. A source sends
    /// something at least twice a second, so a few seconds of nothing mean
    /// that the connection is gone.
    pub fn set_timeout(&self, timeout: Duration) -> std::io::Result<()> {
        self.channel.stream().set_read_timeout(Some(timeout))
    }

    /// A second handle on the connection. Shutting it down from another
    /// thread ends a wait in [`Subscription::next_packet`].
    pub fn connection(&self) -> std::io::Result<TcpStream> {
        self.channel.stream().try_clone()
    }

    /// Waits for the next packet. Its samples replace what is in
    /// `samples`; the number of frames is returned with the header.
    pub fn next_packet(&mut self, samples: &mut Vec<f32>) -> Result<(Header, usize)> {
        loop {
            let packet = self.channel.recv()?;
            match wire::decode(&packet, usize::from(self.info.channels), samples) {
                Ok(read) => return Ok(read),
                // A kind of packet from a newer version: nothing for us.
                Err(WireError::Kind(_)) => continue,
                Err(err) => return Err(err.into()),
            }
        }
    }
}

/// How two machines' clocks relate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ClockOffset {
    /// Add this to a time on the source's clock to get the time on this
    /// machine's clock, in nanoseconds.
    pub source_to_local_ns: i64,
    /// Round trip of the exchange the offset was taken from (the shortest
    /// one). The offset is off by less than half of this.
    pub round_trip_ns: u64,
    pub rounds: u32,
}

/// Compares this machine's clock with the clock of the source at `addr`,
/// by asking it the time `rounds` times and keeping the quickest answer.
pub fn clock_offset(
    addr: SocketAddr,
    keys: &LocalKeys,
    peers: &PeerStore,
    expected: Option<&str>,
    rounds: u32,
    timeout: Duration,
) -> Result<ClockOffset> {
    let (mut channel, _) = open(addr, keys, peers, expected, timeout)?;
    let mut best: Option<ClockOffset> = None;
    for round in 0..rounds.max(1) {
        if round > 0 {
            std::thread::sleep(Duration::from_millis(2));
        }
        let t1 = clock::now_ns();
        let hello = Hello::Time { t1 };
        channel.send(&serde_json::to_vec(&hello).map_err(|_| Error::Malformed)?)?;
        let answer = channel.recv()?;
        let t4 = clock::now_ns();
        let (t2, t3) = match serde_json::from_slice::<Reply>(&answer) {
            Ok(Reply::Time { t1: echoed, t2, t3 }) if echoed == t1 => (t2, t3),
            Ok(Reply::Error {
                code: ErrorCode::Forbidden,
                ..
            }) => return Err(Error::Forbidden),
            _ => return Err(Error::Malformed),
        };
        let round_trip_ns = (t4 - t1).saturating_sub(t3.saturating_sub(t2));
        let source_to_local_ns = ((t1 as i64 - t2 as i64) + (t4 as i64 - t3 as i64)) / 2;
        if best.is_none_or(|best| round_trip_ns < best.round_trip_ns) {
            best = Some(ClockOffset {
                source_to_local_ns,
                round_trip_ns,
                rounds: rounds.max(1),
            });
        }
    }
    best.ok_or(Error::Malformed)
}

/// What the receiving end saw arrive.
#[derive(Debug, Default)]
pub struct Arrivals {
    packets: u64,
    frames: u64,
    idle_packets: u64,
    lost_packets: u64,
    breaks: u64,
    refused_frames: u64,
    last_seq: Option<u32>,
    /// When the last audio packet arrived, if the last packet was audio.
    last_audio_ns: Option<u64>,
    /// Time between audio packets that followed each other, microseconds.
    intervals_us: Vec<f64>,
    received: Option<Vec<Entry>>,
}

/// [`Arrivals`] in numbers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArrivalStats {
    pub packets: u64,
    pub frames: u64,
    pub idle_packets: u64,
    /// Sequence numbers that never arrived.
    pub lost_packets: u64,
    /// Packets the source marked as not following on.
    pub breaks: u64,
    /// Frames that found the queue to the sound card full.
    pub refused_frames: u64,
    /// Time between one audio packet and the next, in milliseconds.
    pub interval_ms: Option<crate::probe::Spread>,
}

impl Arrivals {
    /// `keep_ledger` keeps a line for every audio packet, for comparing
    /// with what the source sent.
    pub fn new(keep_ledger: bool) -> Self {
        Arrivals {
            received: keep_ledger.then(Vec::new),
            ..Arrivals::default()
        }
    }

    fn note(&mut self, header: &Header, frames: usize, samples: &[f32], arrival_ns: u64) {
        if let Some(last) = self.last_seq {
            self.lost_packets += u64::from(header.seq.wrapping_sub(last).wrapping_sub(1));
        }
        self.last_seq = Some(header.seq);
        match header.kind {
            Kind::Audio => {
                self.packets += 1;
                self.frames += frames as u64;
                if header.discontinuity {
                    self.breaks += 1;
                }
                if let Some(last) = self.last_audio_ns {
                    // Keep a day's worth at most; the longest is in there
                    // from early on or not at all.
                    if self.intervals_us.len() < 20_000_000 {
                        self.intervals_us
                            .push(arrival_ns.saturating_sub(last) as f64 * 1e-3);
                    }
                }
                self.last_audio_ns = Some(arrival_ns);
                if let Some(received) = &mut self.received {
                    received.push(Entry {
                        seq: header.seq,
                        position: header.position,
                        frames: frames as u32,
                        digest: wire::digest(samples),
                    });
                }
            }
            Kind::Idle => {
                self.idle_packets += 1;
                self.last_audio_ns = None;
            }
            Kind::End => {}
        }
    }

    pub fn stats(&self) -> ArrivalStats {
        let mut intervals: Vec<f64> = self.intervals_us.iter().map(|us| us * 1e-3).collect();
        ArrivalStats {
            packets: self.packets,
            frames: self.frames,
            idle_packets: self.idle_packets,
            lost_packets: self.lost_packets,
            breaks: self.breaks,
            refused_frames: self.refused_frames,
            interval_ms: crate::probe::Spread::of(&mut intervals),
        }
    }

    /// The lines kept for comparing with the source's list.
    pub fn ledger(&self) -> &[Entry] {
        self.received.as_deref().unwrap_or(&[])
    }
}

/// Why [`pump`] returned without an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// The source said that it stops sending on this connection.
    BySource,
    /// `stop` was set.
    Stopped,
}

/// Moves packets from the connection into the queue of the playout until
/// the source ends the stream, `stop` is set, or the connection fails.
pub fn pump(
    subscription: &mut Subscription,
    queue: &mut Writer,
    arrivals: &mut Arrivals,
    stop: &AtomicBool,
) -> Result<Ended> {
    let mut samples = Vec::new();
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(Ended::Stopped);
        }
        let (header, frames) = subscription.next_packet(&mut samples)?;
        let arrival_ns = clock::now_ns();
        let lost_before = arrivals.lost_packets;
        arrivals.note(&header, frames, &samples, arrival_ns);
        match header.kind {
            Kind::Audio => {
                // A packet that never came breaks the stream as much as one
                // the source left out.
                let discontinuity = header.discontinuity || arrivals.lost_packets > lost_before;
                if !queue.push_audio(
                    frames,
                    &samples,
                    discontinuity,
                    header.position,
                    header.capture_ns,
                    arrival_ns,
                ) {
                    arrivals.refused_frames += frames as u64;
                }
            }
            Kind::Idle => queue.push_idle(arrival_ns),
            Kind::End => return Ok(Ended::BySource),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fifo::queue;
    use crate::source::{capture_ring, CaptureWriter};
    use std::sync::atomic::AtomicU32;

    const WAIT: Duration = Duration::from_secs(10);
    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// A device with its identity and its list of paired devices, in a
    /// directory of its own that is removed afterwards.
    struct Device {
        dir: PathBuf,
        identity: Identity,
    }

    impl Device {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "istesso-audio-test-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Device {
                dir,
                identity: Identity::generate(),
            }
        }

        fn peers_path(&self) -> PathBuf {
            self.dir.join("peers.json")
        }

        fn peers(&self) -> PeerStore {
            PeerStore::open(self.peers_path()).unwrap()
        }

        /// Pairs `other` with this device, as pairing would, and returns
        /// the id this device knows it by.
        fn trust(&self, name: &str, other: &Device) -> String {
            let peer = Peer::from_hello(&istesso_pair::Hello {
                device_name: name.to_string(),
                public_key: other.identity.public_key(),
            });
            let id = peer.id.clone();
            self.peers().add(peer).unwrap();
            id
        }

        fn allow_audio(&self, id: &str, allowed: bool) {
            self.peers()
                .set_permission(id, Permission::Audio, allowed)
                .unwrap()
                .unwrap();
        }

        fn keys(&self) -> LocalKeys {
            LocalKeys::new(&self.identity).unwrap()
        }
    }

    impl Drop for Device {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// The lines a source kept about what it sent, and what it logged.
    type Sent = Arc<Mutex<Vec<Entry>>>;
    type Lines = Arc<Mutex<Vec<String>>>;

    /// A source on this machine that sends what is pushed into the
    /// returned writer.
    fn start_source(host: &Device) -> (SocketAddr, CaptureWriter, Sent, Lines) {
        let (writer, packetizer) = capture_ring(2, 48_000.0, 1.0, 0.5, 0.05);
        let sent = Arc::new(Mutex::new(Vec::new()));
        let lines = Arc::new(Mutex::new(Vec::new()));
        let log_lines = lines.clone();
        let source = Source::bind(
            "127.0.0.1:0".parse().unwrap(),
            &host.identity,
            host.peers_path(),
            StreamInfo::new(48_000, 2, 256),
            packetizer,
            Some(sent.clone()),
            Arc::new(move |line| log_lines.lock().unwrap().push(line)),
        )
        .unwrap();
        let addr = source.local_addr().unwrap();
        std::thread::spawn(move || source.run());
        (addr, writer, sent, lines)
    }

    fn piece(n: u32) -> Vec<f32> {
        (0..512).map(|i| (n * 1000 + i) as f32).collect()
    }

    /// Two devices that are paired with each other; the exit may have the
    /// host's sound.
    fn paired() -> (Device, Device, String, String) {
        let host = Device::new();
        let exit = Device::new();
        let exit_id = host.trust("exit", &exit);
        let host_id = exit.trust("host", &host);
        host.allow_audio(&exit_id, true);
        (host, exit, host_id, exit_id)
    }

    #[test]
    fn sound_reaches_a_paired_device_that_may_have_it() {
        let (host, exit, host_id, _) = paired();
        let (addr, mut capture, sent, _) = start_source(&host);
        let mut subscription =
            subscribe(addr, &exit.keys(), &exit.peers(), Some(&host_id), WAIT).unwrap();
        assert_eq!(subscription.peer().name, "host");
        assert_eq!(subscription.info().sample_rate, 48_000);
        assert_eq!(subscription.info().channels, 2);
        subscription.set_timeout(WAIT).unwrap();

        // The stream starts with what is captured once the source has
        // taken the listener on. Feed until the first packet has arrived.
        let (mut queue_in, mut queue_out) = queue(2, 1 << 16);
        let mut arrivals = Arrivals::new(true);
        let feeding = Arc::new(AtomicBool::new(true));
        let feed = feeding.clone();
        let feeder = std::thread::spawn(move || {
            let mut n = 0;
            while feed.load(Ordering::Relaxed) {
                capture.push(&piece(n), u64::from(n));
                n += 1;
                // About the pace of real audio.
                std::thread::sleep(Duration::from_millis(5));
            }
            capture
        });

        let mut samples = Vec::new();
        let mut received = Vec::new();
        while received.len() < 50 {
            let (header, frames) = subscription.next_packet(&mut samples).unwrap();
            if header.kind != Kind::Audio {
                continue;
            }
            assert_eq!(frames, 256);
            arrivals.note(&header, frames, &samples, clock::now_ns());
            assert!(queue_in.push_audio(
                frames,
                &samples,
                header.discontinuity,
                header.position,
                header.capture_ns,
                0
            ));
            received.push((header, samples.clone()));
        }
        feeding.store(false, Ordering::Relaxed);
        feeder.join().unwrap();

        // Each packet is one of the pieces that were captured, unchanged,
        // and they follow on from each other.
        for pair in received.windows(2) {
            assert_eq!(pair[1].0.seq, pair[0].0.seq + 1);
            assert_eq!(pair[1].0.position, pair[0].0.position + 256);
            assert!(!pair[1].0.discontinuity);
        }
        for (header, samples) in &received {
            let n = (header.position / 256) as u32;
            assert_eq!(samples, &piece(n));
            assert_eq!(header.capture_ns, u64::from(n));
        }
        assert_eq!(queue_out.available(), 50 * 256);
        let mut out = vec![0.0; 512];
        queue_out.read(256, &mut out);
        assert_eq!(out, received[0].1);

        // Both ends kept the same lines.
        let stats = arrivals.stats();
        assert_eq!(stats.packets, 50);
        assert_eq!(stats.lost_packets, 0);
        let comparison = crate::ledger::compare(&sent.lock().unwrap(), arrivals.ledger());
        assert_eq!(comparison.received, 50);
        assert!(comparison.intact(), "{comparison:?}");
    }

    #[test]
    fn a_paired_device_without_the_right_gets_nothing() {
        let (host, exit, host_id, exit_id) = paired();
        host.allow_audio(&exit_id, false);
        let (addr, _capture, _, lines) = start_source(&host);
        let result = subscribe(addr, &exit.keys(), &exit.peers(), Some(&host_id), WAIT);
        assert!(matches!(result, Err(Error::Forbidden)));
        // Not even the time of day.
        let result = clock_offset(addr, &exit.keys(), &exit.peers(), Some(&host_id), 1, WAIT);
        assert!(matches!(result, Err(Error::Forbidden)));
        // The person at the source is told, by name and without keys.
        let lines = lines.lock().unwrap();
        assert!(lines.iter().any(|line| line.contains("exit")), "{lines:?}");
    }

    #[test]
    fn a_device_that_is_not_paired_is_refused() {
        let (host, _exit, _, _) = paired();
        let stranger = Device::new();
        // The stranger knows the host's key; the host does not know the
        // stranger.
        let host_id = stranger.trust("host", &host);
        let (addr, _capture, _, _) = start_source(&host);
        let result = subscribe(
            addr,
            &stranger.keys(),
            &stranger.peers(),
            Some(&host_id),
            WAIT,
        );
        assert!(matches!(result, Err(Error::Link(LinkError::NotTrusted))));
    }

    #[test]
    fn an_impostor_at_the_address_is_not_listened_to() {
        let (host, exit, host_id, _) = paired();
        // Someone else answers where the host is expected, and would be
        // glad to send the exit some sound.
        let impostor = Device::new();
        let exit_id = impostor.trust("exit", &exit);
        impostor.allow_audio(&exit_id, true);
        let (addr, _capture, _, _) = start_source(&impostor);
        let result = subscribe(addr, &exit.keys(), &exit.peers(), Some(&host_id), WAIT);
        assert!(matches!(result, Err(Error::Link(LinkError::UnknownPeer))));
        drop(host);
    }

    #[test]
    fn taking_the_right_away_stops_a_stream_that_is_running() {
        let (host, exit, host_id, exit_id) = paired();
        let (addr, mut capture, _, lines) = start_source(&host);
        let mut subscription =
            subscribe(addr, &exit.keys(), &exit.peers(), Some(&host_id), WAIT).unwrap();
        subscription.set_timeout(WAIT).unwrap();

        let mut samples = Vec::new();
        let begin = Instant::now();
        let mut withdrawn: Option<Instant> = None;
        let mut n = 0;
        let ended = loop {
            capture.push(&piece(n), 0);
            n += 1;
            let (header, _) = subscription.next_packet(&mut samples).unwrap();
            if header.kind == Kind::End {
                break Instant::now();
            }
            if withdrawn.is_none() && begin.elapsed() > Duration::from_millis(300) {
                host.allow_audio(&exit_id, false);
                withdrawn = Some(Instant::now());
            }
            assert!(begin.elapsed() < WAIT, "the stream was not stopped");
            std::thread::sleep(Duration::from_millis(5));
        };
        // Stopped within the second the source allows itself, and a bit.
        let took = ended.duration_since(withdrawn.unwrap());
        assert!(took < Duration::from_millis(1500), "{took:?}");
        // And it stays stopped: a new attempt is refused.
        let result = subscribe(addr, &exit.keys(), &exit.peers(), Some(&host_id), WAIT);
        assert!(matches!(result, Err(Error::Forbidden)));
        let lines = lines.lock().unwrap();
        assert!(
            lines.iter().any(|line| line.contains("no longer allowed")),
            "{lines:?}"
        );
    }

    #[test]
    fn removing_the_pairing_stops_a_stream_too() {
        let (host, exit, host_id, exit_id) = paired();
        let (addr, mut capture, _, _) = start_source(&host);
        let mut subscription =
            subscribe(addr, &exit.keys(), &exit.peers(), Some(&host_id), WAIT).unwrap();
        subscription.set_timeout(WAIT).unwrap();
        host.peers().remove(&exit_id).unwrap().unwrap();

        let mut samples = Vec::new();
        let begin = Instant::now();
        loop {
            capture.push(&piece(0), 0);
            let (header, _) = subscription.next_packet(&mut samples).unwrap();
            if header.kind == Kind::End {
                break;
            }
            assert!(begin.elapsed() < WAIT, "the stream was not stopped");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(begin.elapsed() < Duration::from_millis(1800));
    }

    #[test]
    fn a_second_listener_takes_over() {
        let (host, exit, host_id, _) = paired();
        let (addr, mut capture, _, _) = start_source(&host);
        let mut first = subscribe(addr, &exit.keys(), &exit.peers(), Some(&host_id), WAIT).unwrap();
        first.set_timeout(WAIT).unwrap();
        let mut second =
            subscribe(addr, &exit.keys(), &exit.peers(), Some(&host_id), WAIT).unwrap();
        second.set_timeout(WAIT).unwrap();

        let feeding = Arc::new(AtomicBool::new(true));
        let feed = feeding.clone();
        let feeder = std::thread::spawn(move || {
            let mut n = 0;
            while feed.load(Ordering::Relaxed) {
                capture.push(&piece(n), 0);
                n += 1;
                std::thread::sleep(Duration::from_millis(2));
            }
        });
        // The first one is told that it is over ...
        let mut samples = Vec::new();
        loop {
            let (header, _) = first.next_packet(&mut samples).unwrap();
            if header.kind == Kind::End {
                break;
            }
        }
        // ... and the second one gets the sound.
        loop {
            let (header, frames) = second.next_packet(&mut samples).unwrap();
            if header.kind == Kind::Audio {
                assert_eq!(frames, 256);
                break;
            }
        }
        feeding.store(false, Ordering::Relaxed);
        feeder.join().unwrap();
    }

    #[test]
    fn a_source_with_nothing_to_play_says_so() {
        let (host, exit, host_id, _) = paired();
        let (addr, _capture, _, _) = start_source(&host);
        let mut subscription =
            subscribe(addr, &exit.keys(), &exit.peers(), Some(&host_id), WAIT).unwrap();
        subscription.set_timeout(WAIT).unwrap();
        let mut samples = Vec::new();
        let begin = Instant::now();
        let (first, frames) = subscription.next_packet(&mut samples).unwrap();
        assert_eq!((first.kind, frames), (Kind::Idle, 0));
        let (second, _) = subscription.next_packet(&mut samples).unwrap();
        assert_eq!(second.kind, Kind::Idle);
        assert_eq!(second.seq, first.seq + 1);
        // The first after a fifth of a second, the next half a second on.
        let took = begin.elapsed();
        assert!(
            took > Duration::from_millis(600) && took < Duration::from_millis(1500),
            "{took:?}"
        );
    }

    #[test]
    fn the_pump_fills_the_queue_and_keeps_count() {
        let (host, exit, host_id, _) = paired();
        let (addr, mut capture, _, _) = start_source(&host);
        let mut subscription =
            subscribe(addr, &exit.keys(), &exit.peers(), Some(&host_id), WAIT).unwrap();
        subscription.set_timeout(WAIT).unwrap();
        let connection = subscription.connection().unwrap();
        let (mut queue_in, queue_out) = queue(2, 1 << 16);

        let stop = Arc::new(AtomicBool::new(false));
        let stop_pump = stop.clone();
        let pumping = std::thread::spawn(move || {
            let mut arrivals = Arrivals::new(false);
            let result = pump(&mut subscription, &mut queue_in, &mut arrivals, &stop_pump);
            (result, arrivals.stats())
        });
        let begin = Instant::now();
        let mut n = 0;
        while queue_out.available() < 20 * 256 {
            capture.push(&piece(n), 0);
            n += 1;
            std::thread::sleep(Duration::from_millis(2));
            assert!(begin.elapsed() < WAIT);
        }
        // Hanging up from this side ends the pump's wait.
        stop.store(true, Ordering::Relaxed);
        connection.shutdown(std::net::Shutdown::Both).unwrap();
        let (result, stats) = pumping.join().unwrap();
        assert!(
            matches!(result, Ok(Ended::Stopped) | Err(Error::Link(_))),
            "{result:?}"
        );
        assert!(stats.packets >= 20);
        assert_eq!(stats.lost_packets, 0);
        assert_eq!(stats.refused_frames, 0);
        assert!(stats.interval_ms.is_some());
    }

    #[test]
    fn two_clocks_on_one_machine_agree() {
        let (host, exit, host_id, _) = paired();
        let (addr, _capture, _, _) = start_source(&host);
        let offset =
            clock_offset(addr, &exit.keys(), &exit.peers(), Some(&host_id), 8, WAIT).unwrap();
        assert_eq!(offset.rounds, 8);
        // Both ends read the same clock here, so the offset is the error of
        // the method: less than half the round trip.
        assert!(offset.round_trip_ns < 50_000_000, "{offset:?}");
        assert!(
            offset.source_to_local_ns.unsigned_abs() <= offset.round_trip_ns / 2 + 1,
            "{offset:?}"
        );
    }

    #[test]
    fn the_right_is_looked_up_on_disk_each_time() {
        let host = Device::new();
        let exit = Device::new();
        let id = host.trust("exit", &exit);
        let peer = host.peers().find(&id).unwrap().unwrap().clone();
        // Paired, but only for looking and controlling.
        assert!(!may_still_hear(&host.peers_path(), &peer));
        host.allow_audio(&id, true);
        assert!(may_still_hear(&host.peers_path(), &peer));
        // Another device that took over the name does not inherit the right.
        let mut impostor = peer.clone();
        impostor.public_key = "22".repeat(32);
        assert!(!may_still_hear(&host.peers_path(), &impostor));
        // The pairing is removed while a stream runs.
        host.peers().remove(&id).unwrap();
        assert!(!may_still_hear(&host.peers_path(), &peer));
        // No list at all.
        assert!(!may_still_hear(&host.dir.join("nowhere.json"), &peer));
    }
}
