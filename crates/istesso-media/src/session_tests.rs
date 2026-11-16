//! A host and its viewers on this machine's loopback: what arrives, who is
//! let in, and what ends a stream.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use istesso_link::{Channel, Error as LinkError, LocalKeys};
use istesso_pair::{Identity, Permission};

use crate::testing::TempPeers;
use crate::wire::{self, Ask, Chroma, Codec, Colour, Reply, StreamInfo};
use crate::{Error, Host, Viewer};

const SOON: Duration = Duration::from_secs(5);

fn info() -> StreamInfo {
    StreamInfo {
        codec: Codec::H265,
        width: 3440,
        height: 1440,
        fps: 85,
        chroma: Chroma::Full,
        colour: Colour::Srgb,
    }
}

/// A host called "studio" and a device called "air" that are paired with
/// each other.
struct Pair {
    host: Host,
    addr: SocketAddr,
    /// The devices the host trusts, and the ones the device trusts.
    studio_peers: TempPeers,
    air_peers: TempPeers,
    air: Identity,
    said: Arc<Mutex<Vec<String>>>,
}

fn pair() -> Pair {
    let studio = Identity::generate();
    let air = Identity::generate();
    let studio_peers = TempPeers::trusting(&[("air", &air)]);
    let air_peers = TempPeers::trusting(&[("studio", &studio)]);
    let said = Arc::new(Mutex::new(Vec::new()));
    let log = {
        let said = said.clone();
        Arc::new(move |line: String| said.lock().unwrap().push(line))
    };
    let host = Host::listen(
        "127.0.0.1:0".parse().unwrap(),
        &studio,
        studio_peers.path(),
        info(),
        log,
    )
    .unwrap();
    let addr = host.local_addr();
    Pair {
        host,
        addr,
        studio_peers,
        air_peers,
        air,
        said,
    }
}

impl Pair {
    fn connect(&self) -> crate::Result<Viewer> {
        self.connect_as(&self.air)
    }

    fn connect_as(&self, identity: &Identity) -> crate::Result<Viewer> {
        let keys = LocalKeys::new(identity).unwrap();
        Viewer::connect(self.addr, &keys, &self.air_peers.open(), None, SOON)
    }

    /// A viewer that the host has taken into its list, and that has been
    /// noted as needing a key frame.
    fn watching(&self) -> Viewer {
        let viewer = self.connect().unwrap();
        // The viewer is told that frames follow a moment before the host
        // has it on its list.
        self.until(|| self.host.viewers() >= 1);
        viewer
    }

    fn until(&self, mut done: impl FnMut() -> bool) {
        let began = Instant::now();
        while !done() {
            assert!(began.elapsed() < SOON, "waited too long");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn said(&self, part: &str) -> bool {
        self.said.lock().unwrap().iter().any(|l| l.contains(part))
    }

    /// The handshake of a device, without the request that follows it.
    fn raw(&self, identity: &Identity) -> istesso_link::Result<Channel<TcpStream>> {
        let stream = TcpStream::connect(self.addr).unwrap();
        stream.set_read_timeout(Some(SOON)).unwrap();
        let keys = LocalKeys::new(identity).unwrap();
        Channel::initiate(stream, &keys, |_| Ok(()))
    }

    /// A device that has asked for the picture and been answered, and that
    /// is driven by hand from there on.
    fn raw_watching(&self) -> Channel<TcpStream> {
        let mut channel = self.raw(&self.air).unwrap();
        channel
            .send(&serde_json::to_vec(&Ask::Watch { version: 1 }).unwrap())
            .unwrap();
        let reply: Reply = serde_json::from_slice(&channel.recv().unwrap()).unwrap();
        assert!(matches!(reply, Reply::Stream { .. }));
        self.until(|| self.host.viewers() == 1);
        channel
    }
}

fn picture(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

/// What a viewer was handed, kept beyond the next call.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Got {
    data: Vec<u8>,
    key_frame: bool,
    discontinuity: bool,
    seq: u64,
    pts_ns: u64,
    encoded_ns: u64,
    messages: u32,
}

fn take(viewer: &mut Viewer) -> crate::Result<Option<Got>> {
    Ok(viewer.next_frame(SOON)?.map(|frame| {
        assert!(frame.whole_ns >= frame.first_ns);
        Got {
            data: frame.data.to_vec(),
            key_frame: frame.key_frame,
            discontinuity: frame.discontinuity,
            seq: frame.seq,
            pts_ns: frame.pts_ns,
            encoded_ns: frame.encoded_ns,
            messages: frame.messages,
        }
    }))
}

#[test]
fn frames_arrive_whole_and_in_order() {
    let pair = pair();
    let mut viewer = pair.watching();
    assert_eq!(viewer.peer().name, "studio");
    assert_eq!(viewer.info(), &info());
    // A viewer that has just come needs a key frame, and that is said once.
    assert!(pair.host.key_frame_wanted());
    assert!(!pair.host.key_frame_wanted());

    // The size of a frame of the text picture, of a hard picture, one that
    // exactly fills a message of the channel, one byte more, none at all,
    // and a megabyte.
    let one_message = 65535 - 16 - 4 - wire::HEADER_LEN;
    let sizes = [140_000, 7_500, one_message, one_message + 1, 0, 1 << 20];
    let frames: Vec<Vec<u8>> = sizes
        .iter()
        .enumerate()
        .map(|(i, len)| picture(*len, i as u8))
        .collect();

    let receiving = std::thread::spawn(move || {
        let mut got = Vec::new();
        while got.len() < 6 {
            got.push(take(&mut viewer).unwrap().expect("a frame in time"));
        }
        got
    });
    for (i, frame) in frames.iter().enumerate() {
        let sent = pair
            .host
            .send_frame(frame, 1000 + i as u64, 2000 + i as u64, i == 0)
            .unwrap();
        assert_eq!(sent.viewers, 1, "frame {i}");
        assert_eq!(sent.seq, i as u64);
    }
    let got = receiving.join().unwrap();
    for (i, got) in got.iter().enumerate() {
        assert_eq!(got.data, frames[i], "frame {i}");
        assert_eq!(got.seq, i as u64);
        assert_eq!(got.key_frame, i == 0);
        assert!(!got.discontinuity);
        assert_eq!(
            (got.pts_ns, got.encoded_ns),
            (1000 + i as u64, 2000 + i as u64)
        );
    }
    let messages: Vec<u32> = got.iter().map(|g| g.messages).collect();
    assert_eq!(messages, [3, 1, 1, 2, 1, 17]);
    assert!(pair.said("showing the picture to air"));
}

#[test]
fn a_frame_larger_than_any_frame_may_be_is_not_sent() {
    let pair = pair();
    let _viewer = pair.watching();
    let result = pair
        .host
        .send_frame(&vec![0u8; wire::MAX_FRAME_BYTES + 1], 0, 0, true);
    assert!(matches!(result, Err(Error::Wire(_))));
    assert_eq!(pair.host.viewers(), 1);
}

#[test]
fn a_viewer_is_sent_nothing_before_a_key_frame() {
    let pair = pair();
    let mut viewer = pair.watching();
    // Frames that build on ones the viewer never had are of no use to it.
    for i in 0..3 {
        let sent = pair.host.send_frame(b"delta", i, i, false).unwrap();
        assert_eq!(sent.viewers, 0);
    }
    assert_eq!(pair.host.send_frame(b"key", 3, 3, true).unwrap().viewers, 1);
    assert_eq!(
        pair.host.send_frame(b"next", 4, 4, false).unwrap().viewers,
        1
    );

    let first = take(&mut viewer).unwrap().unwrap();
    assert_eq!(first.data, b"key");
    assert!(first.key_frame);
    // The host had encoded frames before it that this viewer did not get.
    assert!(first.discontinuity);
    assert_eq!(first.seq, 3);
    let second = take(&mut viewer).unwrap().unwrap();
    assert_eq!(second.data, b"next");
    assert!(!second.key_frame && !second.discontinuity);
}

#[test]
fn a_viewer_that_waits_for_a_key_frame_has_the_host_ask_again() {
    let pair = pair();
    let _viewer = pair.watching();
    assert!(pair.host.key_frame_wanted());
    // The encoder was told, and no key frame came of it.
    let began = Instant::now();
    let mut asked_again = false;
    while began.elapsed() < Duration::from_secs(2) && !asked_again {
        pair.host.send_frame(b"delta", 0, 0, false).unwrap();
        asked_again = pair.host.key_frame_wanted();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(asked_again);
    assert!(began.elapsed() >= Duration::from_millis(400));
}

#[test]
fn a_request_for_a_key_frame_reaches_the_host() {
    let pair = pair();
    let mut viewer = pair.watching();
    assert!(pair.host.key_frame_wanted());
    assert_eq!(pair.host.send_frame(b"key", 0, 0, true).unwrap().viewers, 1);
    assert!(!pair.host.key_frame_wanted());

    // Asked from another thread than the one that receives, while that one
    // waits for a frame.
    let asker = viewer.asker();
    let receiving = std::thread::spawn(move || {
        let first = take(&mut viewer).unwrap().unwrap();
        let second = take(&mut viewer).unwrap().unwrap();
        (first, second, viewer)
    });
    std::thread::sleep(Duration::from_millis(50));
    let asked = Instant::now();
    asker.ask_for_key_frame();
    // The host notices without a frame going out.
    pair.until(|| pair.host.key_frame_wanted());
    assert!(asked.elapsed() < Duration::from_millis(500));
    assert!(!pair.host.key_frame_wanted());

    assert_eq!(
        pair.host.send_frame(b"again", 1, 1, true).unwrap().viewers,
        1
    );
    let (first, second, _viewer) = receiving.join().unwrap();
    assert_eq!(first.data, b"key");
    assert_eq!(second.data, b"again");
    assert!(second.key_frame && !second.discontinuity);
}

#[test]
fn a_device_without_the_right_to_view_is_refused() {
    let pair = pair();
    // Paired, and allowed to send input, but not to look.
    pair.studio_peers.allow("air", &[Permission::Control]);
    assert!(matches!(pair.connect(), Err(Error::Forbidden)));
    assert_eq!(pair.host.viewers(), 0);
    assert_eq!(pair.host.send_frame(b"key", 0, 0, true).unwrap().viewers, 0);
    assert!(!pair.host.key_frame_wanted());
    assert!(pair.said("refused to show the picture to air"));

    pair.studio_peers.allow("air", &[Permission::View]);
    assert!(pair.connect().is_ok());
}

#[test]
fn a_device_that_is_not_paired_is_refused() {
    let pair = pair();
    let stranger = Identity::generate();
    assert!(matches!(
        pair.connect_as(&stranger),
        Err(Error::Link(LinkError::NotTrusted))
    ));
    pair.until(|| pair.said("refused a connection"));
    assert_eq!(pair.host.viewers(), 0);
    assert_eq!(pair.host.send_frame(b"key", 0, 0, true).unwrap().viewers, 0);
}

#[test]
fn a_viewer_only_talks_to_a_paired_host_and_to_the_one_it_named() {
    let pair = pair();
    let keys = LocalKeys::new(&pair.air).unwrap();
    // The device at the address is not one this device is paired with.
    let nobody = TempPeers::trusting(&[]);
    let result = Viewer::connect(pair.addr, &keys, &nobody.open(), None, SOON);
    assert!(matches!(result, Err(Error::Link(LinkError::UnknownPeer))));
    // It is paired, but it is not the one that was asked for.
    let result = Viewer::connect(
        pair.addr,
        &keys,
        &pair.air_peers.open(),
        Some("0000-0000-0000-0000"),
        SOON,
    );
    assert!(matches!(
        result,
        Err(Error::Link(LinkError::WrongPeer { .. }))
    ));
    let id = pair.air_peers.open().peers()[0].id.clone();
    let result = Viewer::connect(pair.addr, &keys, &pair.air_peers.open(), Some(&id), SOON);
    assert!(result.is_ok());
}

/// What ends a stream that is running, by a change to the list of paired
/// devices on the host: the viewer gets no further frame, within a second.
fn the_stream_stops_after(change: impl FnOnce(&Pair)) -> Pair {
    let pair = pair();
    let mut viewer = pair.watching();
    assert_eq!(pair.host.send_frame(b"key", 0, 0, true).unwrap().viewers, 1);
    assert_eq!(take(&mut viewer).unwrap().unwrap().data, b"key");

    change(&pair);
    let changed = Instant::now();
    // Frames keep coming from the encoder meanwhile.
    let mut last_sent = Instant::now();
    while pair.host.viewers() > 0 {
        assert!(changed.elapsed() < Duration::from_secs(1), "still sending");
        if pair.host.send_frame(b"delta", 1, 1, false).unwrap().viewers > 0 {
            last_sent = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(last_sent.duration_since(changed) < Duration::from_secs(1));
    assert_eq!(pair.host.send_frame(b"key", 2, 2, true).unwrap().viewers, 0);
    // The viewer is handed what was sent before, and then learns that the
    // connection is over.
    let over = loop {
        match take(&mut viewer) {
            Ok(Some(got)) => assert_eq!(got.data, b"delta"),
            Ok(None) => panic!("the viewer was left waiting"),
            Err(err) => break err,
        }
    };
    assert!(matches!(over, Error::Link(LinkError::Closed)), "{over}");
    pair
}

#[test]
fn removing_the_pairing_stops_a_stream() {
    let pair = the_stream_stops_after(|pair| pair.studio_peers.remove("air"));
    assert!(pair.said("no longer paired"));
    // And the device does not get in again.
    assert!(matches!(
        pair.connect(),
        Err(Error::Link(LinkError::NotTrusted))
    ));
}

#[test]
fn taking_the_right_to_view_away_stops_a_stream() {
    let pair =
        the_stream_stops_after(|pair| pair.studio_peers.allow("air", &[Permission::Control]));
    assert!(matches!(pair.connect(), Err(Error::Forbidden)));
}

#[test]
fn a_device_that_was_paired_anew_under_the_same_name_is_not_the_viewer() {
    // Another key under the same name does not take over the stream.
    let pair = the_stream_stops_after(|pair| {
        pair.studio_peers.remove("air");
        let mut store = pair.studio_peers.open();
        let mut other = crate::testing::peer("air", &Identity::generate());
        other.id = pair.air.fingerprint();
        store.add(other).unwrap();
    });
    assert_eq!(pair.host.viewers(), 0);
}

#[test]
fn an_unreadable_list_of_paired_devices_sends_nothing() {
    let good = std::cell::RefCell::new(Vec::new());
    let pair = the_stream_stops_after(|pair| {
        *good.borrow_mut() = std::fs::read(pair.studio_peers.path()).unwrap();
        std::fs::write(pair.studio_peers.path(), "{ not a list").unwrap();
    });
    assert!(pair.said("cannot be read"));
    // Nobody gets in while it cannot be read: the caller is not even
    // answered.
    assert!(pair.connect().is_err());
    assert_eq!(pair.host.viewers(), 0);
    assert_eq!(pair.host.send_frame(b"key", 3, 3, true).unwrap().viewers, 0);

    // Readable again, the paired device is let in again.
    std::fs::write(pair.studio_peers.path(), &*good.borrow()).unwrap();
    let mut viewer = pair.watching();
    assert_eq!(pair.host.send_frame(b"key", 4, 4, true).unwrap().viewers, 1);
    assert_eq!(take(&mut viewer).unwrap().unwrap().data, b"key");
}

#[test]
fn nothing_is_sent_while_the_list_has_not_been_looked_at_for_a_second() {
    let pair = pair();
    let mut viewer = pair.watching();
    assert!(pair.host.key_frame_wanted());
    assert_eq!(pair.host.send_frame(b"key", 0, 0, true).unwrap().viewers, 1);

    // The thread that looks the viewers up is stuck.
    pair.host.pause_recheck(true);
    let stuck = Instant::now();
    let mut sent_last = Duration::ZERO;
    while stuck.elapsed() < Duration::from_millis(1600) {
        if pair.host.send_frame(b"delta", 1, 1, false).unwrap().viewers > 0 {
            sent_last = stuck.elapsed();
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(sent_last > Duration::from_millis(500), "{sent_last:?}");
    assert!(sent_last < Duration::from_millis(1100), "{sent_last:?}");
    // The viewer is still connected; it is only not sent anything.
    assert_eq!(pair.host.viewers(), 1);
    assert_eq!(pair.host.send_frame(b"key", 2, 2, true).unwrap().viewers, 0);
    // Having missed frames, it will need a key frame.
    assert!(pair.host.key_frame_wanted());

    // The look-ups go on again: the stream does, from a key frame.
    pair.host.pause_recheck(false);
    pair.until(|| {
        pair.host.send_frame(b"delta", 3, 3, false).is_ok() && pair.host.key_frame_wanted()
    });
    pair.until(|| {
        pair.host
            .send_frame(b"fresh key", 4, 4, true)
            .unwrap()
            .viewers
            == 1
    });
    let mut last = take(&mut viewer).unwrap().unwrap();
    while last.data != b"fresh key" {
        assert!(last.data == b"key" || last.data == b"delta");
        last = take(&mut viewer).unwrap().unwrap();
    }
    assert!(last.key_frame && last.discontinuity);
}

#[test]
fn a_caller_that_sends_a_byte_at_a_time_is_cut_off_at_the_deadline() {
    let pair = pair();
    pair.host.set_hello_wait(Duration::from_millis(600));
    let began = Instant::now();
    let mut stream = TcpStream::connect(pair.addr).unwrap();
    stream.set_read_timeout(Some(SOON)).unwrap();
    // The version, then the length of the largest handshake message, then
    // its bytes one by one, each well inside any single wait.
    stream.write_all(b"ISC1").unwrap();
    stream.write_all(&[0x02, 0x00]).unwrap();
    let mut slow = stream.try_clone().unwrap();
    let sending = std::thread::spawn(move || {
        for _ in 0..80 {
            std::thread::sleep(Duration::from_millis(40));
            if slow.write_all(&[0]).is_err() {
                break;
            }
        }
    });
    // The host's own version comes back, and then the connection ends.
    let mut bytes = Vec::new();
    let _ = stream.read_to_end(&mut bytes);
    let took = began.elapsed();
    assert_eq!(bytes, b"ISC1");
    assert!(took >= Duration::from_millis(550), "{took:?}");
    assert!(took < Duration::from_millis(1500), "{took:?}");
    sending.join().unwrap();
    // The place it held is free again.
    pair.until(|| pair.host.pending() == 0);
    assert_eq!(pair.host.viewers(), 0);
}

#[test]
fn a_paired_device_that_never_says_what_it_wants_is_cut_off_at_the_same_deadline() {
    let pair = pair();
    pair.host.set_hello_wait(Duration::from_millis(600));
    let began = Instant::now();
    // The handshake is done at once; the request never comes.
    let mut channel = pair.raw(&pair.air).unwrap();
    assert!(began.elapsed() < Duration::from_millis(400));
    assert!(matches!(channel.recv(), Err(LinkError::Closed)));
    let took = began.elapsed();
    assert!(took >= Duration::from_millis(550), "{took:?}");
    assert!(took < Duration::from_millis(1500), "{took:?}");
    pair.until(|| pair.host.pending() == 0);
    assert_eq!(pair.host.viewers(), 0);
}

#[test]
fn a_first_message_that_is_not_a_request_to_watch_is_answered_and_the_caller_let_go() {
    let pair = pair();
    let mut channel = pair.raw(&pair.air).unwrap();
    channel.send(br#"{"type":"teleport","to":"mars"}"#).unwrap();
    let reply: Reply = serde_json::from_slice(&channel.recv().unwrap()).unwrap();
    assert!(matches!(
        reply,
        Reply::Error {
            code: wire::ErrorCode::Unsupported,
            ..
        }
    ));
    assert!(matches!(channel.recv(), Err(LinkError::Closed)));
    assert_eq!(pair.host.viewers(), 0);
}

#[test]
fn callers_that_hold_every_place_do_not_stop_a_stream_that_runs() {
    let pair = pair();
    let mut viewer = pair.watching();
    assert_eq!(pair.host.send_frame(b"key", 0, 0, true).unwrap().viewers, 1);

    pair.host.set_hello_wait(Duration::from_millis(1500));
    // More callers than there are places, none of which says anything.
    let silent: Vec<TcpStream> = (0..crate::host::MAX_PENDING + 4)
        .map(|_| TcpStream::connect(pair.addr).unwrap())
        .collect();
    pair.until(|| pair.host.pending() == crate::host::MAX_PENDING);
    // The viewer that is being served goes on being served.
    for i in 1..20u64 {
        assert_eq!(
            pair.host.send_frame(b"delta", i, i, false).unwrap().viewers,
            1
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    for _ in 0..20 {
        assert!(take(&mut viewer).unwrap().is_some());
    }
    // Another paired device finds no place for now ...
    let book = Identity::generate();
    pair.studio_peers
        .open()
        .add(crate::testing::peer("book", &book))
        .unwrap();
    assert!(pair.connect_as(&book).is_err());
    // ... and gets in once the silent ones have run out of time.
    pair.until(|| pair.host.pending() == 0);
    let _second = pair.connect_as(&book).unwrap();
    pair.until(|| pair.host.viewers() == 2);
    drop(silent);
}

#[test]
fn a_device_that_connects_again_takes_the_place_of_its_earlier_connection() {
    let pair = pair();
    let mut first = pair.watching();
    assert_eq!(pair.host.send_frame(b"key", 0, 0, true).unwrap().viewers, 1);
    assert_eq!(take(&mut first).unwrap().unwrap().data, b"key");

    // The viewer program was stopped and started; what it left behind may
    // not have been noticed yet.
    let mut second = pair.connect().unwrap();
    pair.until(|| pair.said("connected again"));
    pair.until(|| pair.host.viewers() == 1);
    assert!(pair.host.key_frame_wanted());
    assert_eq!(
        pair.host
            .send_frame(b"new key", 1, 1, true)
            .unwrap()
            .viewers,
        1
    );
    assert_eq!(take(&mut second).unwrap().unwrap().data, b"new key");
    assert!(matches!(
        take(&mut first),
        Err(Error::Link(LinkError::Closed))
    ));
}

#[test]
fn the_host_shows_its_picture_to_a_few_devices_and_no_more() {
    let pair = pair();
    let devices: Vec<Identity> = (0..crate::host::MAX_STREAMS + 1)
        .map(|_| Identity::generate())
        .collect();
    for (i, device) in devices.iter().enumerate() {
        pair.studio_peers
            .open()
            .add(crate::testing::peer(&format!("device {i}"), device))
            .unwrap();
    }
    let mut viewers: Vec<Viewer> = devices[..crate::host::MAX_STREAMS]
        .iter()
        .map(|device| pair.connect_as(device).unwrap())
        .collect();
    pair.until(|| pair.host.viewers() == crate::host::MAX_STREAMS);
    match pair.connect_as(&devices[crate::host::MAX_STREAMS]) {
        Err(Error::Refused(why)) => assert!(why.contains("as many devices"), "{why}"),
        other => panic!("unexpected: {:?}", other.err()),
    }
    // All of them get the same frame.
    let sent = pair.host.send_frame(b"key", 0, 0, true).unwrap();
    assert_eq!(sent.viewers as usize, crate::host::MAX_STREAMS);
    for viewer in &mut viewers {
        assert_eq!(take(viewer).unwrap().unwrap().data, b"key");
    }
    // One of them leaves: there is a place again.
    viewers.pop();
    pair.until(|| pair.host.viewers() == crate::host::MAX_STREAMS - 1);
    assert!(pair.connect_as(&devices[crate::host::MAX_STREAMS]).is_ok());
}

#[test]
fn a_picture_that_stands_still_is_not_a_broken_connection() {
    let pair = pair();
    let mut viewer = pair.watching();
    assert_eq!(pair.host.send_frame(b"key", 0, 0, true).unwrap().viewers, 1);
    assert_eq!(take(&mut viewer).unwrap().unwrap().data, b"key");
    // No frame for longer than a viewer goes without hearing from a host.
    let began = Instant::now();
    let nothing = viewer.next_frame(Duration::from_millis(3500));
    assert!(matches!(nothing, Ok(None)), "{:?}", nothing.err());
    assert!(began.elapsed() >= Duration::from_millis(3400));
    assert_eq!(pair.host.viewers(), 1);
    assert_eq!(
        pair.host.send_frame(b"moved", 1, 1, false).unwrap().viewers,
        1
    );
    assert_eq!(take(&mut viewer).unwrap().unwrap().data, b"moved");
}

#[test]
fn a_host_that_says_nothing_is_taken_to_be_gone() {
    // Something that answers like a host and then falls silent, without
    // closing the connection.
    let studio = Identity::generate();
    let air = Identity::generate();
    let air_peers = TempPeers::trusting(&[("studio", &studio)]);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let silent = std::thread::spawn(move || {
        let keys = LocalKeys::new(&studio).unwrap();
        let (stream, _) = listener.accept().unwrap();
        let mut channel = Channel::respond(stream, &keys, |_| true).unwrap();
        channel.recv().unwrap();
        let reply = Reply::Stream {
            version: 1,
            info: info(),
        };
        channel.send(&serde_json::to_vec(&reply).unwrap()).unwrap();
        std::thread::sleep(Duration::from_secs(5));
    });
    let keys = LocalKeys::new(&air).unwrap();
    let mut viewer = Viewer::connect(addr, &keys, &air_peers.open(), None, SOON).unwrap();
    let began = Instant::now();
    let result = viewer.next_frame(Duration::from_secs(30));
    assert!(matches!(result, Err(Error::Silent)), "{:?}", result.err());
    assert!(began.elapsed() >= Duration::from_millis(2900));
    assert!(began.elapsed() < Duration::from_millis(4500));
    drop(viewer);
    silent.join().unwrap();
}

#[test]
fn a_viewer_is_told_when_the_host_closes() {
    let pair = pair();
    let mut viewer = pair.watching();
    assert_eq!(pair.host.send_frame(b"key", 0, 0, true).unwrap().viewers, 1);
    drop(pair);
    assert_eq!(take(&mut viewer).unwrap().unwrap().data, b"key");
    assert!(matches!(take(&mut viewer), Err(Error::Ended)));
}

#[test]
fn a_viewer_that_leaves_is_noticed_without_a_frame_going_out() {
    let pair = pair();
    let viewer = pair.watching();
    drop(viewer);
    // Nothing is sent meanwhile: the regular look at the viewers finds it.
    pair.until(|| pair.host.viewers() == 0);
    assert!(pair.said("stopped showing the picture to air"));
}

#[test]
fn what_a_viewer_says_that_this_version_does_not_know_is_passed_over() {
    let pair = pair();
    let mut channel = pair.raw_watching();
    assert!(pair.host.key_frame_wanted());
    channel.send(br#"{"type":"teleport","to":"mars"}"#).unwrap();
    channel.send(b"not even json").unwrap();
    channel
        .send(br#"{"type":"key_frame","because":"new"}"#)
        .unwrap();
    pair.until(|| pair.host.key_frame_wanted());
    assert_eq!(pair.host.viewers(), 1);
    assert_eq!(pair.host.send_frame(b"key", 0, 0, true).unwrap().viewers, 1);
    let packet = channel.recv().unwrap();
    let (header, at) = wire::decode(&packet).unwrap();
    assert!(header.key_frame);
    assert_eq!(&packet[at], b"key");
}

#[test]
fn a_viewer_that_sends_more_than_a_request_is_cut_off() {
    let pair = pair();
    let mut channel = pair.raw_watching();
    channel.send(&vec![b' '; wire::MAX_ASK_BYTES + 1]).unwrap();
    pair.until(|| pair.host.viewers() == 0);
    assert!(pair.said("stopped showing the picture to air"));
}

#[test]
fn a_viewer_that_begins_a_request_and_does_not_finish_it_is_cut_off() {
    let pair = pair();
    let channel = pair.raw_watching();
    // Two bytes that announce a message of the channel, and nothing after.
    let mut stream = channel.stream().try_clone().unwrap();
    stream.write_all(&[0, 40]).unwrap();
    // Once the two bytes have crossed, the next thing the host does with
    // this viewer finds them. The frame that is due then is held up by no
    // more than the short wait the viewer's message is given.
    let began = Instant::now();
    let mut longest = Duration::ZERO;
    while pair.host.viewers() > 0 {
        assert!(began.elapsed() < Duration::from_secs(1), "still a viewer");
        let sending = Instant::now();
        pair.host.send_frame(b"key", 0, 0, true).unwrap();
        longest = longest.max(sending.elapsed());
    }
    assert!(longest < Duration::from_millis(500), "{longest:?}");
    assert_eq!(pair.host.send_frame(b"key", 0, 0, true).unwrap().viewers, 0);
}

#[test]
fn a_viewer_that_takes_nothing_is_cut_off_and_holds_the_frames_up_only_briefly() {
    let pair = pair();
    // Connected, and never reading.
    let _channel = pair.raw_watching();
    let frame = picture(1 << 20, 1);
    let mut longest = Duration::ZERO;
    let mut sent = 0;
    while pair.host.viewers() > 0 {
        assert!(sent < 4096, "the connection took four gigabytes");
        let began = Instant::now();
        pair.host.send_frame(&frame, 0, 0, sent == 0).unwrap();
        longest = longest.max(began.elapsed());
        sent += 1;
    }
    // The write that found the connection full waited its short time, once.
    assert!(longest >= Duration::from_millis(150), "{longest:?}");
    assert!(longest < Duration::from_millis(1000), "{longest:?}");
    assert!(pair.said("stopped showing the picture to air"));
}
