//! A host and its viewers on this machine's loopback: what arrives, who is
//! let in, and what ends a stream.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use istesso_link::{Channel, Error as LinkError, LocalKeys};
use istesso_pair::{Identity, Permission};

use crate::rejoin::{Lasting, Step, AGAIN_WITHIN, RETRY};
use crate::testing::TempPeers;
use crate::viewer::SILENT_AFTER;
use crate::wire::{self, Ask, Chroma, Codec, Colour, Cursor, CursorImage, Reply, StreamInfo};
use crate::{Admit, Error, Event, Host, Viewer};

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

    /// Whether the host has said something with `part` in it. A stream is
    /// over a moment before the host says so: wait for this with `until`.
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
        self.raw_watching_as(&self.air)
    }

    fn raw_watching_as(&self, identity: &Identity) -> Channel<TcpStream> {
        let before = self.host.viewers();
        let mut channel = self.raw(identity).unwrap();
        channel
            .send(&serde_json::to_vec(&Ask::Watch { version: 1 }).unwrap())
            .unwrap();
        let reply: Reply = serde_json::from_slice(&channel.recv().unwrap()).unwrap();
        assert!(matches!(reply, Reply::Stream { .. }));
        self.until(|| self.host.viewers() == before + 1);
        channel
    }

    /// Pairs another device with the host, and tells the devices' side of
    /// the tests nothing: they all trust the host already.
    fn another_device(&self, name: &str) -> Identity {
        let device = Identity::generate();
        self.studio_peers
            .open()
            .add(crate::testing::peer(name, &device))
            .unwrap();
        device
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
    pair.until(|| pair.said("showing the picture to air"));
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
    pair.until(|| pair.said("refused to show the picture to air"));

    pair.studio_peers.allow("air", &[Permission::View]);
    assert!(pair.connect().is_ok());
}

/// The device as the one that alone is shown the picture.
fn only(name: &str, identity: &Identity) -> Admit {
    let peer = crate::testing::peer(name, identity);
    Admit::Only {
        id: peer.id,
        public_key: peer.public_key,
    }
}

#[test]
fn a_host_that_was_never_told_shows_its_picture_to_every_paired_device_that_may_look() {
    let pair = pair();
    let pad = pair.another_device("pad");
    let _air = pair.watching();
    let _pad = pair.connect_as(&pad).unwrap();
    pair.until(|| pair.host.viewers() == 2);
    assert_eq!(pair.host.send_frame(b"key", 0, 0, true).unwrap().viewers, 2);
}

#[test]
fn while_one_device_is_named_another_paired_device_is_refused() {
    let pair = pair();
    let pad = pair.another_device("pad");
    pair.host.admit(only("air", &pair.air));

    // Paired, allowed to look, and not the one: refused, and told so.
    assert!(matches!(pair.connect_as(&pad), Err(Error::NotShown)));
    pair.until(|| pair.said("refused to show the picture to pad"));
    assert_eq!(pair.host.viewers(), 0);
    // The named device is shown the picture, and only it.
    let mut air = pair.watching();
    assert_eq!(pair.host.send_frame(b"key", 0, 0, true).unwrap().viewers, 1);
    assert_eq!(take(&mut air).unwrap().unwrap().data, b"key");
    assert!(matches!(pair.connect_as(&pad), Err(Error::NotShown)));
    assert_eq!(pair.host.viewers(), 1);

    // The id alone is not the device: the key has to be its own as well.
    let Admit::Only { id, .. } = only("air", &pair.air) else {
        unreachable!()
    };
    let Admit::Only { public_key, .. } = only("pad", &pad) else {
        unreachable!()
    };
    pair.host.admit(Admit::Only { id, public_key });
    assert_eq!(pair.host.viewers(), 0);
    assert!(matches!(pair.connect(), Err(Error::NotShown)));
    assert!(matches!(pair.connect_as(&pad), Err(Error::NotShown)));
}

#[test]
fn naming_another_device_cuts_the_one_before_it_at_once() {
    let pair = pair();
    let pad = pair.another_device("pad");
    pair.host.admit(only("air", &pair.air));
    let mut air = pair.watching();
    assert_eq!(pair.host.send_frame(b"key", 0, 0, true).unwrap().viewers, 1);
    assert_eq!(take(&mut air).unwrap().unwrap().data, b"key");

    // The session moves to the other device. No frame that is handed in
    // after that goes to the first one: not the very next one either.
    pair.host.admit(only("pad", &pad));
    assert_eq!(pair.host.viewers(), 0);
    assert_eq!(
        pair.host.send_frame(b"delta", 1, 1, false).unwrap().viewers,
        0
    );
    pair.until(|| pair.said("stopped showing the picture to air"));
    let over = match take(&mut air) {
        Ok(Some(got)) => panic!("the first device was sent {:?}", got.data),
        Ok(None) => panic!("the first device was left waiting"),
        Err(err) => err,
    };
    assert!(matches!(over, Error::NotShown), "{over}");
    // It does not get in again, and the other one does.
    assert!(matches!(pair.connect(), Err(Error::NotShown)));
    let mut second = pair.connect_as(&pad).unwrap();
    pair.until(|| pair.host.viewers() == 1);
    assert_eq!(pair.host.send_frame(b"key", 2, 2, true).unwrap().viewers, 1);
    assert_eq!(take(&mut second).unwrap().unwrap().data, b"key");
}

#[test]
fn nobody_cuts_everybody_and_lets_nobody_in() {
    let pair = pair();
    let pad = pair.another_device("pad");
    let mut air = pair.watching();
    let mut second = pair.connect_as(&pad).unwrap();
    pair.until(|| pair.host.viewers() == 2);
    assert_eq!(pair.host.send_frame(b"key", 0, 0, true).unwrap().viewers, 2);

    pair.host.admit(Admit::Nobody);
    assert_eq!(pair.host.viewers(), 0);
    assert_eq!(pair.host.send_frame(b"key", 1, 1, true).unwrap().viewers, 0);
    for viewer in [&mut air, &mut second] {
        let over = loop {
            match take(viewer) {
                Ok(Some(got)) => assert_eq!(got.data, b"key"),
                Ok(None) => panic!("a viewer was left waiting"),
                Err(err) => break err,
            }
        };
        assert!(matches!(over, Error::NotShown), "{over}");
    }
    assert!(matches!(pair.connect(), Err(Error::NotShown)));
    assert!(matches!(pair.connect_as(&pad), Err(Error::NotShown)));

    // A host for a session begins like that, before anybody can connect.
    let studio = Identity::generate();
    let peers = TempPeers::trusting(&[("air", &pair.air)]);
    let closed = Host::listen_admitting(
        "127.0.0.1:0".parse().unwrap(),
        &studio,
        peers.path(),
        info(),
        Arc::new(|_| {}),
        Admit::Nobody,
    )
    .unwrap();
    let keys = LocalKeys::new(&pair.air).unwrap();
    let trusting = TempPeers::trusting(&[("studio", &studio)]);
    let refused = Viewer::connect(closed.local_addr(), &keys, &trusting.open(), None, SOON);
    assert!(matches!(refused, Err(Error::NotShown)));
    // Told otherwise, it is as a host that was never told.
    closed.admit(Admit::Paired);
    assert!(Viewer::connect(closed.local_addr(), &keys, &trusting.open(), None, SOON).is_ok());
}

#[test]
fn the_named_device_still_loses_the_picture_when_it_is_unpaired() {
    let pair = pair();
    pair.host.admit(only("air", &pair.air));
    let mut viewer = pair.watching();
    assert_eq!(pair.host.send_frame(b"key", 0, 0, true).unwrap().viewers, 1);
    assert_eq!(take(&mut viewer).unwrap().unwrap().data, b"key");

    // Being named does not stand in for being paired: the list is looked
    // at as before, and the stream stops within its second.
    pair.studio_peers.remove("air");
    let removed = Instant::now();
    while pair.host.viewers() > 0 {
        assert!(removed.elapsed() < Duration::from_secs(1), "still sending");
        let _ = pair.host.send_frame(b"delta", 1, 1, false).unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    pair.until(|| pair.said("no longer paired"));
    assert!(matches!(
        pair.connect(),
        Err(Error::Link(LinkError::NotTrusted))
    ));
    // Nor does losing the right to look.
    let pad = pair.another_device("pad");
    pair.host.admit(only("pad", &pad));
    pair.studio_peers.allow("pad", &[Permission::Control]);
    assert!(matches!(pair.connect_as(&pad), Err(Error::Forbidden)));
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
    pair.until(|| pair.said("no longer paired"));
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
    pair.until(|| pair.said("cannot be read"));
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

    // The look-ups go on again: the stream does, from a key frame, which
    // the viewer needs now that it has missed frames.
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
        Err(Error::Busy) => {}
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
    pair.until(|| pair.connect_as(&devices[crate::host::MAX_STREAMS]).is_ok());
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
    // A frame that comes in the very moment the viewer is being told that the picture
    // stands still goes on without it, and the next one is a key frame; either way the
    // picture moves again.
    pair.until(|| pair.host.send_frame(b"moved", 1, 1, true).unwrap().viewers == 1);
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
    assert!(began.elapsed() >= SILENT_AFTER - Duration::from_millis(100));
    assert!(began.elapsed() < SILENT_AFTER + Duration::from_millis(1500));
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
    pair.until(|| pair.said("stopped showing the picture to air"));
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
    pair.until(|| pair.said("stopped showing the picture to air"));
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

/// A viewer that receives on a thread of its own and writes down when each
/// frame came.
struct Receiving {
    arrivals: Arc<Mutex<Vec<Instant>>>,
    over: Arc<Mutex<Option<String>>>,
    thread: std::thread::JoinHandle<()>,
}

fn receive(mut viewer: Viewer) -> Receiving {
    let arrivals = Arc::new(Mutex::new(Vec::new()));
    let over = Arc::new(Mutex::new(None));
    let thread = {
        let (arrivals, over) = (arrivals.clone(), over.clone());
        std::thread::spawn(move || loop {
            match viewer.next_frame(Duration::from_millis(50)) {
                Ok(Some(_)) => arrivals.lock().unwrap().push(Instant::now()),
                Ok(None) if Arc::strong_count(&arrivals) == 1 => return,
                Ok(None) => {}
                Err(err) => {
                    *over.lock().unwrap() = Some(err.to_string());
                    return;
                }
            }
        })
    };
    Receiving {
        arrivals,
        over,
        thread,
    }
}

impl Receiving {
    fn count(&self) -> usize {
        self.arrivals.lock().unwrap().len()
    }

    fn last(&self) -> Option<Instant> {
        self.arrivals.lock().unwrap().last().copied()
    }

    fn is_over(&self) -> bool {
        self.over.lock().unwrap().is_some()
    }

    /// Stops receiving and returns when the frames came.
    fn finish(self) -> Vec<Instant> {
        let Receiving {
            arrivals, thread, ..
        } = self;
        let times = arrivals.lock().unwrap().clone();
        drop(arrivals);
        thread.join().unwrap();
        times
    }
}

/// Takes what is sent a byte at a time: slowly, but never so
/// slowly that any single wait for it would run out.
fn trickle(channel: &Channel<TcpStream>) {
    let mut stream = channel.stream().try_clone().unwrap();
    std::thread::spawn(move || {
        let began = Instant::now();
        let mut byte = [0u8; 1];
        // Not for ever, for the sake of the tests that run after this one.
        while began.elapsed() < Duration::from_secs(30) && stream.read_exact(&mut byte).is_ok() {
            std::thread::sleep(Duration::from_millis(1));
        }
    });
}

#[test]
fn a_viewer_that_takes_nothing_is_cut_off_and_holds_no_frame_up() {
    let pair = pair();
    // Connected, and never reading.
    let _channel = pair.raw_watching();
    let frame = picture(1 << 18, 1);
    let began = Instant::now();
    let (mut longest, mut sent) = (Duration::ZERO, 0);
    // When a frame's rest was left waiting for the connection.
    let mut left_waiting: Vec<Instant> = Vec::new();
    while pair.host.viewers() > 0 {
        assert!(began.elapsed() < Duration::from_secs(20), "still a viewer");
        let sending = Instant::now();
        let result = pair.host.send_frame(&frame, 0, 0, true).unwrap();
        longest = longest.max(sending.elapsed());
        if result.queued > 0 {
            // Its connection is full: the rest of this frame has until the
            // deadline, and nothing more is sealed for it meanwhile (the
            // host checks that itself in a debug build, which this is).
            //
            // It may happen more than once. A system can take the rest
            // after all without the viewer having read anything, by giving
            // the connection more room; then the frame is out, within its
            // time, and the next one is begun. What may not happen is that
            // a frame's rest outlives its deadline and the viewer stays.
            if let Some(before) = left_waiting.last() {
                let apart = before.elapsed();
                assert!(
                    apart < Duration::from_millis(350),
                    "a frame's rest waited {apart:?} and the viewer was kept"
                );
            }
            left_waiting.push(Instant::now());
        }
        sent += result.viewers;
        std::thread::sleep(Duration::from_millis(2));
    }
    // No frame waited for the viewer. The frame whose rest the connection
    // never took was given its time, once, and then the viewer was let go.
    assert!(longest < Duration::from_millis(150), "{longest:?}");
    let given = left_waiting
        .last()
        .expect("the connection never filled")
        .elapsed();
    assert!(given >= Duration::from_millis(180), "{given:?}");
    assert!(given < Duration::from_millis(700), "{given:?}");
    assert!(sent >= 1);
    pair.until(|| pair.said("stopped showing the picture to air"));
}

#[test]
fn a_frame_whose_rest_gets_out_in_time_is_followed_by_the_next_with_a_deadline_of_its_own() {
    let pair = pair();
    let channel = pair.raw_watching();
    let frame = picture(1 << 18, 1);
    // The viewer reads nothing until a frame's rest waits for its connection ...
    let began = Instant::now();
    loop {
        assert!(began.elapsed() < Duration::from_secs(10), "it never filled");
        assert_eq!(pair.host.viewers(), 1, "the viewer was let go too early");
        if pair.host.send_frame(&frame, 0, 0, true).unwrap().queued > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    // ... and then takes everything there is, well within that frame's time: the rest
    // gets out. (A system that gives a connection more room does the same without the
    // viewer having read anything.)
    let mut stream = channel.stream();
    stream
        .set_read_timeout(Some(Duration::from_millis(20)))
        .unwrap();
    let mut buffer = vec![0u8; 1 << 16];
    let drained = Instant::now();
    while drained.elapsed() < Duration::from_millis(100) {
        let _ = stream.read(&mut buffer);
    }
    // The viewer is still shown the picture, and the next frame is begun.
    assert_eq!(pair.host.viewers(), 1);
    // It stops reading again. Frames go on as before. One whose rest waits now has a
    // time of its own, counted from when it began to wait, and when that is over the
    // viewer is let go. As in the test above, a rest may get out after all (the
    // connection is given more room, or what the viewer has not read yet makes room on
    // its way): then it is the next frame that waits, and the last one is the one that
    // counts.
    let began = Instant::now();
    let mut left_waiting: Vec<Instant> = Vec::new();
    while pair.host.viewers() > 0 {
        assert!(
            began.elapsed() < Duration::from_secs(20),
            "still a viewer, long after it stopped reading again ({} frames were left waiting)",
            left_waiting.len()
        );
        if pair.host.send_frame(&frame, 0, 0, true).unwrap().queued > 0 {
            if let Some(before) = left_waiting.last() {
                let apart = before.elapsed();
                assert!(
                    apart < Duration::from_millis(350),
                    "a frame's rest waited {apart:?} and the viewer was kept"
                );
            }
            left_waiting.push(Instant::now());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let given = left_waiting
        .last()
        .expect("the connection never filled again")
        .elapsed();
    assert!(given >= Duration::from_millis(180), "{given:?}");
    assert!(given < Duration::from_millis(700), "{given:?}");
}

#[test]
fn a_viewer_that_takes_a_byte_at_a_time_is_cut_off_and_costs_the_others_nothing() {
    let pair = pair();
    let book = pair.another_device("book");
    let good = receive(pair.connect_as(&book).unwrap());
    pair.until(|| pair.host.viewers() == 1);
    let slow = pair.raw_watching();
    trickle(&slow);

    // A frame every ten milliseconds, each one a key frame, so that both
    // viewers can be sent every one of them.
    let frame = picture(1 << 16, 2);
    let began = Instant::now();
    let (mut longest, mut full_at, mut gone_after, mut frames) = (Duration::ZERO, None, None, 0);
    while began.elapsed() < Duration::from_secs(20) {
        let sending = Instant::now();
        let result = pair.host.send_frame(&frame, 0, 0, true).unwrap();
        longest = longest.max(sending.elapsed());
        frames += 1;
        if result.queued > 0 && full_at.is_none() {
            full_at = Some(Instant::now());
        }
        if let (Some(full), None, 1) = (full_at, gone_after, pair.host.viewers()) {
            gone_after = Some(full.elapsed());
        }
        // A little longer, to see the other viewer carry on undisturbed.
        if gone_after
            .is_some_and(|after| full_at.unwrap().elapsed() > after + Duration::from_millis(300))
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // The slow viewer took a little all the time, and still had only the
    // one deadline for its frame.
    let gone_after = gone_after.expect("the slow viewer was never cut off");
    assert!(gone_after >= Duration::from_millis(150), "{gone_after:?}");
    assert!(gone_after < Duration::from_millis(700), "{gone_after:?}");
    // No frame waited for it ...
    assert!(longest < Duration::from_millis(150), "{longest:?}");
    // ... and the other viewer got every frame, evenly.
    pair.until(|| good.count() == frames);
    assert!(!good.is_over());
    let arrivals = good.finish();
    let widest = arrivals
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .max()
        .unwrap();
    assert!(widest < Duration::from_millis(190), "{widest:?}");
}

#[test]
fn removing_the_pairing_stops_a_stream_while_another_viewer_takes_nothing() {
    let pair = pair();
    let book = pair.another_device("book");
    let good = receive(pair.connect().unwrap());
    pair.until(|| pair.host.viewers() == 1);
    // Connected, and never reading.
    let _slow = pair.raw_watching_as(&book);

    let frame = picture(1 << 18, 3);
    let began = Instant::now();
    let mut removed = None;
    while !good.is_over() {
        assert!(began.elapsed() < Duration::from_secs(20), "still running");
        let result = pair.host.send_frame(&frame, 0, 0, true).unwrap();
        if result.queued > 0 && removed.is_none() {
            // The slow viewer's connection is full and its frame is being
            // waited for, right now: the pairing of the other is removed.
            assert_eq!(pair.host.viewers(), 2);
            pair.studio_peers.remove("air");
            removed = Some(Instant::now());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let removed = removed.expect("the slow viewer's connection never filled");
    let last = good.last().expect("no frame arrived");
    // Looked up every quarter of a second: well inside the second allowed.
    assert!(
        last.saturating_duration_since(removed) < Duration::from_millis(600),
        "{:?}",
        last.saturating_duration_since(removed)
    );
    pair.until(|| pair.said("stopped showing the picture to air: it is no longer paired"));
    good.finish();
}

#[test]
fn a_viewer_that_lost_its_right_gets_nothing_more_however_many_devices_connect() {
    let pair = pair();
    let book = pair.another_device("book");
    let air = receive(pair.connect().unwrap());
    pair.until(|| pair.host.viewers() == 1);
    pair.until(|| pair.host.send_frame(b"key", 0, 0, true).unwrap().viewers == 1);
    pair.until(|| air.count() >= 1);

    // The thread that looks the viewers up is stuck, which is what the
    // limit on the age of a look-up is for; and the right is taken away.
    pair.host.pause_recheck(true);
    pair.studio_peers.allow("air", &[Permission::Control]);
    let taken = Instant::now();
    // Another device connects again and again. The list is read for each
    // of its calls: that says something about that device, and nothing
    // about the one that is already being sent to.
    let mut admitted = 0;
    while taken.elapsed() < Duration::from_millis(2500) {
        if let Ok(viewer) = pair.connect_as(&book) {
            admitted += 1;
            drop(viewer);
        }
        pair.host.send_frame(b"key", 1, 1, true).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(admitted >= 10, "only {admitted} calls were taken");
    let last = air.last().unwrap().saturating_duration_since(taken);
    assert!(last < Duration::from_millis(1100), "{last:?}");
    air.finish();
}

// ---- The cursor ----

fn arrow(pixels: &[u8]) -> Cursor<'_> {
    Cursor::Image(CursorImage {
        width: 24,
        height: 16,
        hotspot_x: 3,
        hotspot_y: 5,
        scale: 1,
        rgba: pixels,
    })
}

/// What a viewer was handed, in the order it came: `f` for a frame, `c` for a cursor.
fn events(viewer: &mut Viewer, count: usize) -> (String, Vec<Vec<u8>>) {
    let (mut order, mut cursors) = (String::new(), Vec::new());
    while order.len() < count {
        match viewer.next(SOON).unwrap().expect("something in time") {
            Event::Frame(_) => order.push('f'),
            Event::Cursor(cursor) => {
                order.push('c');
                cursors.push(match cursor {
                    Cursor::Hidden => b"hidden".to_vec(),
                    Cursor::Named(name) => name.as_bytes().to_vec(),
                    Cursor::Image(image) => image.rgba.to_vec(),
                });
            }
        }
    }
    (order, cursors)
}

#[test]
fn the_cursor_reaches_a_viewer_and_a_viewer_that_comes_later() {
    let pair = pair();
    let pixels = picture(24 * 16 * 4, 7);
    // Set before anybody watches.
    pair.host.set_cursor(&arrow(&pixels)).unwrap();
    let mut viewer = pair.watching();
    assert!(viewer.cursor().is_none());
    // The viewer is told what the cursor is without a frame having to go out.
    let (order, cursors) = events(&mut viewer, 1);
    assert_eq!(order, "c");
    assert_eq!(cursors[0], pixels);
    assert_eq!(viewer.cursor(), Some(arrow(&pixels)));

    // The application hides the pointer, and shows one there is no image of.
    std::thread::sleep(Duration::from_millis(30));
    pair.host.set_cursor(&Cursor::Hidden).unwrap();
    assert_eq!(events(&mut viewer, 1).1[0], b"hidden");
    assert_eq!(viewer.cursor(), Some(Cursor::Hidden));
    std::thread::sleep(Duration::from_millis(30));
    pair.host.set_cursor(&Cursor::Named("default")).unwrap();
    assert_eq!(events(&mut viewer, 1).1[0], b"default");

    // Frames go on as before, and a viewer that only wants frames gets only frames; what
    // the cursor is, is kept for it. (A frame that comes in the very moment the viewer's
    // own thread is telling it the cursor goes on without that viewer, by design; so the
    // test lets that thread finish before it hands a frame over.)
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(pair.host.send_frame(b"key", 0, 0, true).unwrap().viewers, 1);
    assert_eq!(take(&mut viewer).unwrap().unwrap().data, b"key");
    std::thread::sleep(Duration::from_millis(60));
    pair.host.set_cursor(&arrow(&pixels)).unwrap();
    pair.until(|| {
        matches!(viewer.next_frame(Duration::from_millis(20)), Ok(None))
            && viewer.cursor() == Some(arrow(&pixels))
    });
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(
        pair.host.send_frame(b"next", 1, 1, false).unwrap().viewers,
        1
    );
    assert_eq!(take(&mut viewer).unwrap().unwrap().data, b"next");
    assert_eq!(viewer.cursor(), Some(arrow(&pixels)));
}

#[test]
fn a_cursor_that_may_not_travel_is_refused_and_reaches_nobody() {
    let pair = pair();
    let mut viewer = pair.watching();
    let pixels = vec![0u8; 257 * 257 * 4];
    let image = |width: u16, height: u16, hotspot: (u16, u16), len: usize| {
        Cursor::Image(CursorImage {
            width,
            height,
            hotspot_x: hotspot.0,
            hotspot_y: hotspot.1,
            scale: 1,
            rgba: &pixels[..len],
        })
    };
    let refused = [
        // Larger than any cursor may be.
        image(257, 16, (0, 0), 257 * 16 * 4),
        image(16, 257, (0, 0), 257 * 16 * 4),
        // Of no size.
        image(0, 16, (0, 0), 0),
        image(16, 0, (0, 0), 0),
        // Not as many bytes as its size says.
        image(16, 16, (0, 0), 16 * 16 * 4 - 1),
        image(16, 16, (0, 0), 16 * 16 * 4 + 4),
        // Its hot spot outside it.
        image(16, 16, (16, 0), 16 * 16 * 4),
        image(16, 16, (0, 16), 16 * 16 * 4),
    ];
    for cursor in &refused {
        let result = pair.host.set_cursor(cursor);
        assert!(
            matches!(result, Err(Error::Wire(wire::WireError::Cursor))),
            "{:?}",
            result.err()
        );
    }
    // Nothing of it went out: the next thing the viewer gets is the frame.
    assert_eq!(pair.host.send_frame(b"key", 0, 0, true).unwrap().viewers, 1);
    assert_eq!(events(&mut viewer, 1).0, "f");
    assert!(viewer.cursor().is_none());
    // One that fits is taken.
    pair.host
        .set_cursor(&image(16, 16, (15, 15), 16 * 16 * 4))
        .unwrap();
    assert_eq!(events(&mut viewer, 1).0, "c");
}

#[test]
fn a_viewer_passes_over_a_cursor_that_does_not_add_up_and_kinds_it_does_not_know() {
    // Something that answers like a host and then sends what no host of this version
    // would: cursors whose numbers do not fit, and a kind of packet from a later version.
    let studio = Identity::generate();
    let air = Identity::generate();
    let air_peers = TempPeers::trusting(&[("studio", &studio)]);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let odd = std::thread::spawn(move || {
        let keys = LocalKeys::new(&studio).unwrap();
        let (stream, _) = listener.accept().unwrap();
        let mut channel = Channel::respond(stream, &keys, |_| true).unwrap();
        channel.recv().unwrap();
        let reply = Reply::Stream {
            version: 1,
            info: info(),
        };
        channel.send(&serde_json::to_vec(&reply).unwrap()).unwrap();

        let mut good = Vec::new();
        wire::encode_cursor(&Cursor::Named("default"), 0, &mut good).unwrap();
        let body = |change: &dyn Fn(&mut Vec<u8>)| {
            let mut packet = good.clone();
            change(&mut packet);
            // The header's length of what follows has to be right, or the packet as a
            // whole is refused before the cursor is looked at.
            let len = (packet.len() - wire::HEADER_LEN) as u32;
            packet[32..36].copy_from_slice(&len.to_le_bytes());
            packet
        };
        let at = wire::HEADER_LEN;
        let packets = [
            // An image of 300 by 300 that brings no pixels at all.
            body(&|p| {
                p[at] = 2;
                p[at + 4..at + 6].copy_from_slice(&300u16.to_le_bytes());
                p[at + 6..at + 8].copy_from_slice(&300u16.to_le_bytes());
                p.truncate(at + 16);
            }),
            // An image of 16 by 16 with half its pixels.
            body(&|p| {
                p[at] = 2;
                p[at + 4..at + 6].copy_from_slice(&16u16.to_le_bytes());
                p[at + 6..at + 8].copy_from_slice(&16u16.to_le_bytes());
                p[at + 12..at + 14].copy_from_slice(&1u16.to_le_bytes());
                p.truncate(at + 16);
                p.extend_from_slice(&[0; 16 * 8 * 4]);
            }),
            // An image whose hot spot is outside it.
            body(&|p| {
                p[at] = 2;
                p[at + 4..at + 6].copy_from_slice(&4u16.to_le_bytes());
                p[at + 6..at + 8].copy_from_slice(&4u16.to_le_bytes());
                p[at + 8..at + 10].copy_from_slice(&4u16.to_le_bytes());
                p[at + 12..at + 14].copy_from_slice(&1u16.to_le_bytes());
                p.truncate(at + 16);
                p.extend_from_slice(&[0; 4 * 4 * 4]);
            }),
            // Cut off inside its fixed part.
            body(&|p| p.truncate(at + 7)),
            // A kind of packet this version does not know.
            body(&|p| p[1] = 9),
        ];
        for packet in &packets {
            channel.send(packet).unwrap();
        }
        let mut frame = Vec::new();
        let header = wire::Header {
            kind: wire::Kind::Frame,
            key_frame: true,
            discontinuity: false,
            seq: 0,
            pts_ns: 0,
            encoded_ns: 0,
        };
        wire::encode(&header, b"key", &mut frame);
        channel.send(&frame).unwrap();
        channel.send(&good).unwrap();
        std::thread::sleep(Duration::from_millis(500));
    });
    let keys = LocalKeys::new(&air).unwrap();
    let mut viewer = Viewer::connect(addr, &keys, &air_peers.open(), None, SOON).unwrap();
    // None of the odd ones is handed over, none of them ends the stream, and the
    // cursor stays what it was: unknown.
    match viewer.next(SOON).unwrap().unwrap() {
        Event::Frame(frame) => assert_eq!(frame.data, b"key"),
        other => panic!("unexpected: {other:?}"),
    }
    assert!(viewer.cursor().is_none());
    assert_eq!(
        viewer.next(SOON).unwrap(),
        Some(Event::Cursor(Cursor::Named("default")))
    );
    drop(viewer);
    odd.join().unwrap();
}

#[test]
fn a_cursor_that_changes_all_the_time_does_not_flood_the_connection_or_hold_frames_up() {
    let pair = pair();
    let mut viewer = pair.watching();
    assert_eq!(pair.host.send_frame(b"key", 0, 0, true).unwrap().viewers, 1);
    // The viewer takes what comes, as a viewer does, and counts: frames, cursors, and
    // the number written into the last cursor it was told.
    let done = Arc::new(Mutex::new(None::<u32>));
    let receiving = {
        let done = done.clone();
        std::thread::spawn(move || {
            let (mut frames, mut cursors, mut last) = (0usize, 0u32, 0u32);
            loop {
                match viewer.next(Duration::from_millis(20)).unwrap() {
                    Some(Event::Frame(_)) => frames += 1,
                    Some(Event::Cursor(Cursor::Image(image))) => {
                        cursors += 1;
                        let number = u32::from_le_bytes(image.rgba[..4].try_into().unwrap());
                        // Only ever a newer one.
                        assert!(number > last, "{number} after {last}");
                        last = number;
                    }
                    Some(Event::Cursor(other)) => panic!("unexpected: {other:?}"),
                    None => {}
                }
                if done.lock().unwrap().is_some_and(|set| set == last) {
                    return (frames, cursors, last);
                }
            }
        })
    };
    // For a second, an application sets a new cursor twenty times for every frame that
    // goes out.
    let began = Instant::now();
    let (mut set, mut frames, mut longest) = (0u32, 1usize, Duration::ZERO);
    let mut pixels = vec![0u8; 64 * 64 * 4];
    while began.elapsed() < Duration::from_secs(1) {
        for _ in 0..20 {
            set += 1;
            pixels[..4].copy_from_slice(&set.to_le_bytes());
            let cursor = Cursor::Image(CursorImage {
                width: 64,
                height: 64,
                hotspot_x: 0,
                hotspot_y: 0,
                scale: 1,
                rgba: &pixels,
            });
            pair.host.set_cursor(&cursor).unwrap();
        }
        let sending = Instant::now();
        let sent = pair.host.send_frame(b"delta", 0, 0, false).unwrap();
        assert_eq!(sent.viewers, 1, "frame {frames}");
        longest = longest.max(sending.elapsed());
        frames += 1;
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(set >= 400, "only {set} cursors were set");
    *done.lock().unwrap() = Some(set);
    // Every frame arrives; the viewer ends with the cursor that was set last; and it was
    // told the cursor far less often than it was set: at most once in a frame's interval
    // (85 a second here).
    let (got_frames, got_cursors, last) = receiving.join().unwrap();
    assert_eq!(got_frames, frames);
    assert_eq!(last, set);
    assert!(got_cursors <= 100, "{got_cursors} cursors in a second");
    assert!(got_cursors < set / 4, "{got_cursors} of {set}");
    assert!(longest < Duration::from_millis(100), "{longest:?}");
}

#[test]
fn a_viewer_that_takes_nothing_is_not_sent_cursor_after_cursor() {
    let pair = pair();
    // Connected, and never reading.
    let _channel = pair.raw_watching();
    let pixels = vec![9u8; 256 * 256 * 4];
    let large = Cursor::Image(CursorImage {
        width: 256,
        height: 256,
        hotspot_x: 0,
        hotspot_y: 0,
        scale: 1,
        rgba: &pixels,
    });
    // As large as a cursor may be, again and again: once the connection is full, the
    // viewer has the one deadline every packet has, and is cut off.
    let began = Instant::now();
    while pair.host.viewers() > 0 {
        assert!(began.elapsed() < Duration::from_secs(20), "still a viewer");
        let setting = Instant::now();
        pair.host.set_cursor(&large).unwrap();
        assert!(setting.elapsed() < Duration::from_millis(100));
        std::thread::sleep(Duration::from_millis(1));
    }
    pair.until(|| pair.said("stopped showing the picture to air"));
}

#[test]
fn a_frame_does_not_wait_for_a_viewer_whose_own_thread_holds_its_connection() {
    let pair = pair();
    let book = pair.another_device("book");
    let good = receive(pair.connect_as(&book).unwrap());
    pair.until(|| pair.host.viewers() == 1);
    let held = receive(pair.connect().unwrap());
    pair.until(|| pair.host.viewers() == 2);
    assert!(pair.host.key_frame_wanted());

    // A frame every five milliseconds, each a key frame so that both can take every one.
    // After a while the thread of one viewer takes that viewer's connection for fifty
    // milliseconds without having said that it would be long, which no code does.
    let began = Instant::now();
    let (mut longest, mut frames, mut missed, mut asked) = (Duration::ZERO, 0, 0, false);
    let mut handed = Vec::new();
    while began.elapsed() < Duration::from_millis(400) {
        if !asked && began.elapsed() >= Duration::from_millis(100) {
            pair.host.hold_the_line_of("air", Duration::from_millis(50));
            asked = true;
        }
        let sending = Instant::now();
        handed.push(sending);
        let sent = pair.host.send_frame(b"key", 0, 0, true).unwrap();
        longest = longest.max(sending.elapsed());
        frames += 1;
        missed += usize::from(sent.viewers < 2);
        std::thread::sleep(Duration::from_millis(5));
    }
    // No frame waited for the connection that was held: each gave it a fraction of a
    // millisecond and went on.
    assert!(longest < Duration::from_millis(20), "{longest:?}");
    // The viewer whose connection was held missed frames meanwhile, and only that one.
    assert!(missed >= 2, "{missed} frames went past the held viewer");
    pair.until(|| good.count() == frames);
    pair.until(|| held.count() == frames - missed);
    // The other viewer got its frames as far apart as they were handed over (this
    // test's own pauses, which a busy machine stretches), and no further.
    let widest = |times: &[Instant]| {
        times
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .max()
            .unwrap()
    };
    let (arrived, sent) = (widest(&good.finish()), widest(&handed));
    assert!(
        arrived < sent + Duration::from_millis(20),
        "{arrived:?} apart, handed over {sent:?} apart"
    );
    // Having missed frames, it began again with a key frame, which the host asked for.
    assert!(pair.host.key_frame_wanted());
    held.finish();
}

#[test]
fn a_viewer_that_lost_its_right_is_not_told_the_cursor_either() {
    let pair = pair();
    let mut viewer = pair.watching();
    let mut pixels = vec![0u8; 16 * 16 * 4];
    let cursor = |pixels: &[u8]| -> Result<(), Error> {
        pair.host.set_cursor(&Cursor::Image(CursorImage {
            width: 16,
            height: 16,
            hotspot_x: 0,
            hotspot_y: 0,
            scale: 1,
            rgba: pixels,
        }))
    };
    cursor(&pixels).unwrap();
    assert_eq!(events(&mut viewer, 1).0, "c");

    // The thread that looks the viewers up is stuck, and the right is taken away. The
    // application goes on changing its cursor; no frame goes out.
    pair.host.pause_recheck(true);
    pair.studio_peers.allow("air", &[Permission::Control]);
    let taken = Instant::now();
    let told = Arc::new(Mutex::new(Vec::new()));
    let receiving = {
        let told = told.clone();
        std::thread::spawn(move || loop {
            match viewer.next(Duration::from_millis(50)) {
                Ok(Some(Event::Cursor(_))) => told.lock().unwrap().push(Instant::now()),
                Ok(_) if Arc::strong_count(&told) == 1 => return,
                Ok(_) => {}
                Err(_) => return,
            }
        })
    };
    let mut number = 0u32;
    while taken.elapsed() < Duration::from_millis(2500) {
        number += 1;
        pixels[..4].copy_from_slice(&number.to_le_bytes());
        cursor(&pixels).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
    let times = told.lock().unwrap().clone();
    drop(told);
    receiving.join().unwrap();
    // It was told for as long as its last look-up was good for, and not after.
    let last = times
        .last()
        .expect("never told")
        .saturating_duration_since(taken);
    assert!(last > Duration::from_millis(300), "{last:?}");
    assert!(last < Duration::from_millis(1100), "{last:?}");
}

#[test]
fn nobody_is_told_the_cursor_while_the_list_of_paired_devices_cannot_be_read() {
    let pair = pair();
    let mut viewer = pair.watching();
    pair.host.set_cursor(&Cursor::Named("before")).unwrap();
    assert_eq!(events(&mut viewer, 1).1[0], b"before");

    std::fs::write(pair.studio_peers.path(), "{ not a list").unwrap();
    pair.until(|| pair.host.viewers() == 0);
    // The cursor changes; there is nobody it may be told to, and nobody gets in.
    pair.host.set_cursor(&Cursor::Named("after")).unwrap();
    assert!(pair.connect().is_err());
    let over = loop {
        match viewer.next(SOON) {
            Ok(Some(Event::Cursor(cursor))) => assert_ne!(cursor, Cursor::Named("after")),
            Ok(Some(Event::Frame(_))) => panic!("a frame"),
            Ok(None) => panic!("the viewer was left waiting"),
            Err(err) => break err,
        }
    };
    assert!(matches!(over, Error::Link(LinkError::Closed)), "{over}");
}

#[test]
fn a_picture_that_is_no_longer_offered_reaches_nobody_until_it_is_offered_again() {
    let pair = pair();
    let mut viewer = pair.watching();
    assert_eq!(pair.host.send_frame(b"key", 0, 0, true).unwrap().viewers, 1);
    assert_eq!(take(&mut viewer).unwrap().unwrap().data, b"key");

    // The session is shown to nobody for now: the viewer is let go at once ...
    let began = Instant::now();
    pair.host.offer(false);
    assert_eq!(pair.host.viewers(), 0);
    assert!(began.elapsed() < Duration::from_millis(100));
    assert_eq!(pair.host.send_frame(b"key", 1, 1, true).unwrap().viewers, 0);
    // ... and told why, so that it does not knock again ...
    assert!(matches!(take(&mut viewer), Err(Error::SetAside)));
    // ... and nobody gets in, paired or not; a caller is not even answered.
    assert!(pair.connect().is_err());
    assert_eq!(pair.host.viewers(), 0);
    pair.until(|| pair.host.pending() == 0);

    // Offered again, the paired device is taken again and begins with a key frame.
    pair.host.offer(true);
    let mut viewer = pair.watching();
    assert!(pair.host.key_frame_wanted());
    assert_eq!(
        pair.host.send_frame(b"again", 2, 2, true).unwrap().viewers,
        1
    );
    assert_eq!(take(&mut viewer).unwrap().unwrap().data, b"again");
}

// ---------------------------------------------------------------------------
// A connection that is cut, and the viewer that comes back
// ---------------------------------------------------------------------------

/// Stands between a viewer and a host on the loopback and passes on what
/// they say, until it is told to fall silent: then the connections there
/// are stay open and carry nothing, in either direction, as a network does
/// that drops a connection which both ends still believe in.
struct Between {
    addr: SocketAddr,
    state: Arc<BetweenState>,
}

struct BetweenState {
    /// Where the connections that come in are passed on to.
    to: Mutex<SocketAddr>,
    /// Connections numbered below this carry nothing any more.
    silent_below: std::sync::atomic::AtomicU64,
    accepted: std::sync::atomic::AtomicU64,
    stop: std::sync::atomic::AtomicBool,
}

impl Between {
    fn new(to: SocketAddr) -> Self {
        use std::sync::atomic::Ordering::SeqCst;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(BetweenState {
            to: Mutex::new(to),
            silent_below: 0.into(),
            accepted: 0.into(),
            stop: false.into(),
        });
        let shared = state.clone();
        std::thread::spawn(move || {
            while !shared.stop.load(SeqCst) {
                let Ok((near, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                let number = shared.accepted.fetch_add(1, SeqCst);
                let Ok(far) = TcpStream::connect(*shared.to.lock().unwrap()) else {
                    continue;
                };
                for (from, to) in [
                    (near.try_clone().unwrap(), far.try_clone().unwrap()),
                    (far, near),
                ] {
                    let shared = shared.clone();
                    std::thread::spawn(move || pass_on(from, to, number, &shared));
                }
            }
        });
        Between { addr, state }
    }

    /// Every connection made so far carries nothing from now on. Later
    /// ones are passed on as before.
    fn fall_silent(&self) {
        use std::sync::atomic::Ordering::SeqCst;
        let so_far = self.state.accepted.load(SeqCst);
        self.state.silent_below.store(so_far, SeqCst);
    }

    /// Connections that come in from now on reach another host.
    fn lead_to(&self, to: SocketAddr) {
        *self.state.to.lock().unwrap() = to;
    }

    fn connections(&self) -> u64 {
        self.state
            .accepted
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Drop for Between {
    fn drop(&mut self) {
        self.state
            .stop
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

fn pass_on(mut from: TcpStream, mut to: TcpStream, number: u64, state: &BetweenState) {
    use std::sync::atomic::Ordering::SeqCst;
    from.set_nonblocking(false).unwrap();
    from.set_read_timeout(Some(Duration::from_millis(10)))
        .unwrap();
    let mut buffer = [0u8; 16384];
    while !state.stop.load(SeqCst) {
        match from.read(&mut buffer) {
            Ok(0) => break,
            // Read, and gone: the sender is not held up, and nothing arrives.
            Ok(_) if number < state.silent_below.load(SeqCst) => {}
            Ok(n) => {
                if to.write_all(&buffer[..n]).is_err() {
                    break;
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }
    // A connection that fell silent is left as it is until the test is over:
    // closing it would tell the two ends.
    if number >= state.silent_below.load(SeqCst) {
        let _ = to.shutdown(std::net::Shutdown::Both);
    }
}

/// What the person asked after seeing the system drop a program's first tries in
/// the first milliseconds after it was rebuilt (and the same will happen after
/// every update): the first attempts fail at once, as a local refusal does, and
/// then the way is open. The viewer must not sit out a long wait: it is connected
/// at the first attempt that is made once the way is open.
#[test]
fn when_the_first_attempts_fail_at_once_the_viewer_is_connected_as_soon_as_the_way_opens() {
    let pair = pair();
    let keys = LocalKeys::new(&pair.air).unwrap();
    let list = pair.air_peers.path();
    let addr = pair.addr;
    const FAIL: u32 = 3;
    let attempts = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Instant>::new()));
    let noted = attempts.clone();
    let mut lasting = Lasting::new(
        Box::new(move |expected, within| {
            let mut attempts = noted.lock().unwrap();
            attempts.push(Instant::now());
            if attempts.len() as u32 <= FAIL {
                // At once, without having reached anybody.
                return Err(Error::Link(istesso_link::Error::Io(
                    std::io::Error::from_raw_os_error(libc::EHOSTUNREACH),
                )));
            }
            drop(attempts);
            let peers = istesso_pair::PeerStore::open(&list)?;
            Viewer::connect(addr, &keys, &peers, expected, within)
        }),
        None,
    );
    let began = Instant::now();
    let connected = loop {
        assert!(began.elapsed() < SOON, "never connected");
        match lasting.next(Duration::from_millis(20)).unwrap() {
            Some(Step::Connected { again, .. }) => {
                assert!(!again);
                break began.elapsed();
            }
            Some(Step::Cut) => panic!("a first connection that failed is not a cut"),
            _ => {
                // While it is away the viewer says in what way.
                if !attempts.lock().unwrap().is_empty() {
                    assert_eq!(lasting.last_failure(), Some(crate::Failure::Unreachable));
                }
            }
        }
    };
    assert_eq!(lasting.last_failure(), None);
    assert_eq!(lasting.cuts(), 0);
    let attempts = attempts.lock().unwrap().clone();
    assert_eq!(attempts.len() as u32, FAIL + 1);
    // The first attempt is made at once, and so is the second: a viewer answers a
    // failure at once, once. After that, each attempt that fails is followed by the
    // next after the viewer's ordinary pause (a quarter of a second), no longer.
    let first = attempts[0] - began;
    assert!(first < Duration::from_millis(100), "{first:?}");
    let second = attempts[1] - attempts[0];
    assert!(second < Duration::from_millis(50), "{second:?}");
    for pair in attempts[1..].windows(2) {
        let apart = pair[1] - pair[0];
        assert!(
            apart >= crate::rejoin::RETRY - Duration::from_millis(5)
                && apart < crate::rejoin::RETRY + Duration::from_millis(150),
            "{apart:?}"
        );
    }
    // Connected at the fourth attempt, the one that finds the way open: two pauses
    // after the start, and what a connection takes (the handshake, unoptimised
    // here; a third of a second is ample for it).
    let bound = crate::rejoin::RETRY * (FAIL - 1) + Duration::from_millis(350);
    assert!(connected < bound, "{connected:?}");
}

impl Pair {
    /// A viewer of the device "air" that connects through `addr` and
    /// connects again when it is cut.
    fn lasting_through(&self, addr: SocketAddr) -> Lasting {
        let keys = LocalKeys::new(&self.air).unwrap();
        let list = self.air_peers.path();
        Lasting::new(
            Box::new(move |expected, within| {
                // The list of paired devices is read for every connection, as a
                // program does.
                let peers = istesso_pair::PeerStore::open(&list)?;
                Viewer::connect(addr, &keys, &peers, expected, within)
            }),
            None,
        )
    }

    fn lasting(&self) -> Lasting {
        self.lasting_through(self.addr)
    }
}

/// What a lasting viewer handed over, kept beyond the next call.
#[derive(Debug, PartialEq, Eq)]
enum Stepped {
    Frame { data: Vec<u8>, key_frame: bool },
    Cursor,
    Cut,
    Connected { again: bool, silent_for: Duration },
}

/// The next thing a lasting viewer hands over within a few seconds.
fn step(viewer: &mut Lasting) -> crate::Result<Stepped> {
    let began = Instant::now();
    loop {
        assert!(began.elapsed() < SOON, "waited too long");
        return Ok(match viewer.next(Duration::from_millis(50))? {
            None => continue,
            Some(Step::Event(Event::Frame(frame))) => Stepped::Frame {
                data: frame.data.to_vec(),
                key_frame: frame.key_frame,
            },
            Some(Step::Event(Event::Cursor(_))) => Stepped::Cursor,
            Some(Step::Cut) => Stepped::Cut,
            Some(Step::Connected { again, silent_for }) => Stepped::Connected { again, silent_for },
        });
    }
}

/// A lasting viewer that is connected and has been shown the first key frame.
fn lasting_and_shown(pair: &Pair, viewer: &mut Lasting) {
    assert!(matches!(
        step(viewer).unwrap(),
        Stepped::Connected { again: false, .. }
    ));
    pair.until(|| pair.host.viewers() == 1);
    assert_eq!(pair.host.send_frame(b"key", 0, 0, true).unwrap().viewers, 1);
    assert_eq!(
        step(viewer).unwrap(),
        Stepped::Frame {
            data: b"key".to_vec(),
            key_frame: true
        }
    );
}

#[test]
fn a_connection_that_falls_silent_is_taken_up_again_and_begins_with_a_key_frame() {
    let pair = pair();
    let between = Between::new(pair.addr);
    let mut viewer = pair.lasting_through(between.addr);
    lasting_and_shown(&pair, &mut viewer);

    // The network stops delivering, and tells nobody.
    let silent_from = Instant::now();
    between.fall_silent();
    // Frames the host sends meanwhile go nowhere.
    assert_eq!(
        pair.host.send_frame(b"lost", 1, 1, false).unwrap().viewers,
        1
    );

    assert_eq!(step(&mut viewer).unwrap(), Stepped::Cut);
    let noticed = silent_from.elapsed();
    // A second of silence, not less and not much more.
    assert!(
        noticed >= SILENT_AFTER - Duration::from_millis(100),
        "{noticed:?}"
    );
    assert!(
        noticed < SILENT_AFTER + Duration::from_secs(1),
        "{noticed:?}"
    );

    // It connects again at once, and is the same device to the host: the
    // connection that went silent is closed there, and one stream is left.
    let Stepped::Connected { again, silent_for } = step(&mut viewer).unwrap() else {
        panic!("not connected again");
    };
    assert!(again);
    assert!(silent_for >= SILENT_AFTER - Duration::from_millis(100));
    assert!(silent_from.elapsed() < SILENT_AFTER + Duration::from_secs(1) + AGAIN_WITHIN);
    assert_eq!(between.connections(), 2);
    assert_eq!(viewer.cuts(), 1);
    pair.until(|| pair.host.viewers() == 1);

    // It is sent nothing but a key frame first, and the host has been told
    // to make one.
    pair.until(|| pair.host.key_frame_wanted());
    assert_eq!(
        pair.host.send_frame(b"delta", 2, 2, false).unwrap().viewers,
        0
    );
    assert_eq!(
        pair.host
            .send_frame(b"key again", 3, 3, true)
            .unwrap()
            .viewers,
        1
    );
    assert_eq!(
        step(&mut viewer).unwrap(),
        Stepped::Frame {
            data: b"key again".to_vec(),
            key_frame: true
        }
    );
}

#[test]
fn a_picture_that_stands_still_is_not_a_cut() {
    let pair = pair();
    let mut viewer = pair.lasting();
    lasting_and_shown(&pair, &mut viewer);
    // Nothing is sent for well over the second: the host says twice a
    // second that it has nothing, and the viewer stays.
    let began = Instant::now();
    while began.elapsed() < SILENT_AFTER * 2 {
        assert!(viewer.next(Duration::from_millis(50)).unwrap().is_none());
    }
    assert_eq!(viewer.cuts(), 0);
    assert_eq!(pair.host.viewers(), 1);
}

#[test]
fn a_connection_that_is_closed_is_taken_up_again_without_waiting() {
    let pair = pair();
    let mut viewer = pair.lasting();
    lasting_and_shown(&pair, &mut viewer);

    // (A connection that has only just been made is not made again at once.)
    std::thread::sleep(RETRY + Duration::from_millis(50));
    // The same device connects from elsewhere (the program was started a
    // second time, say): the host closes the earlier connection, and says
    // nothing on it.
    let other = pair.connect().unwrap();
    let closed_at = Instant::now();
    assert_eq!(step(&mut viewer).unwrap(), Stepped::Cut);
    assert!(matches!(
        step(&mut viewer).unwrap(),
        Stepped::Connected { again: true, .. }
    ));
    // Well within the quarter of a second it has when the failure shows.
    assert!(closed_at.elapsed() < RETRY, "{:?}", closed_at.elapsed());
    drop(other);
}

#[test]
fn a_viewer_whose_picture_goes_to_another_device_does_not_come_back() {
    let pair = pair();
    let pad = pair.another_device("pad");
    let between = Between::new(pair.addr);
    pair.host.admit(only("air", &pair.air));
    let mut viewer = pair.lasting_through(between.addr);
    lasting_and_shown(&pair, &mut viewer);

    pair.host.admit(only("pad", &pad));
    // Told so on its connection, and that is the end of it.
    assert!(matches!(step(&mut viewer), Err(Error::NotShown)));
    assert!(viewer.next(Duration::from_millis(600)).is_err());
    assert_eq!(between.connections(), 1, "it knocked again");
    assert_eq!(viewer.cuts(), 0);
}

#[test]
fn a_viewer_that_missed_the_word_asks_once_and_is_told_then() {
    let pair = pair();
    let pad = pair.another_device("pad");
    let between = Between::new(pair.addr);
    pair.host.admit(only("air", &pair.air));
    let mut viewer = pair.lasting_through(between.addr);
    lasting_and_shown(&pair, &mut viewer);

    // The connection goes silent just before the picture moves on, so the
    // host's last word does not arrive.
    between.fall_silent();
    pair.host.admit(only("pad", &pad));
    assert_eq!(step(&mut viewer).unwrap(), Stepped::Cut);
    assert!(matches!(step(&mut viewer), Err(Error::NotShown)));
    assert!(viewer.next(Duration::from_millis(600)).is_err());
    assert_eq!(between.connections(), 2, "one question, and no more");
}

#[test]
fn a_viewer_whose_picture_is_put_aside_does_not_come_back() {
    let pair = pair();
    let between = Between::new(pair.addr);
    let mut viewer = pair.lasting_through(between.addr);
    lasting_and_shown(&pair, &mut viewer);

    pair.host.offer(false);
    assert!(matches!(step(&mut viewer), Err(Error::SetAside)));
    assert!(viewer.next(Duration::from_millis(600)).is_err());
    assert_eq!(between.connections(), 1, "it knocked again");
}

#[test]
fn a_viewer_whose_host_stops_does_not_come_back() {
    let pair = pair();
    let between = Between::new(pair.addr);
    let mut viewer = pair.lasting_through(between.addr);
    lasting_and_shown(&pair, &mut viewer);

    let Pair { host, .. } = pair;
    drop(host);
    assert!(matches!(step(&mut viewer), Err(Error::Ended)));
    assert!(viewer.next(Duration::from_millis(600)).is_err());
    assert_eq!(between.connections(), 1, "it knocked again");
}

#[test]
fn an_unpaired_viewer_is_told_nothing_asks_once_and_stops() {
    let pair = pair();
    let between = Between::new(pair.addr);
    let mut viewer = pair.lasting_through(between.addr);
    lasting_and_shown(&pair, &mut viewer);

    pair.studio_peers.remove("air");
    // The host says nothing to a device that is not paired any more: for
    // the viewer the connection just ends.
    assert_eq!(step(&mut viewer).unwrap(), Stepped::Cut);
    let refused = step(&mut viewer).unwrap_err();
    assert!(
        matches!(refused, Error::Link(LinkError::NotTrusted)),
        "{refused}"
    );
    assert!(viewer.next(Duration::from_millis(600)).is_err());
    assert_eq!(between.connections(), 2, "one question, and no more");
}

#[test]
fn a_viewer_that_may_not_look_any_more_asks_once_and_stops() {
    let pair = pair();
    let between = Between::new(pair.addr);
    let mut viewer = pair.lasting_through(between.addr);
    lasting_and_shown(&pair, &mut viewer);

    pair.studio_peers.allow("air", &[Permission::Audio]);
    assert_eq!(step(&mut viewer).unwrap(), Stepped::Cut);
    assert!(matches!(step(&mut viewer), Err(Error::Forbidden)));
    assert_eq!(between.connections(), 2, "one question, and no more");
}

#[test]
fn another_paired_device_at_the_same_address_is_not_the_picture() {
    let pair = pair();
    let between = Between::new(pair.addr);
    let mut viewer = pair.lasting_through(between.addr);
    lasting_and_shown(&pair, &mut viewer);

    // A second host that the device is paired with as well takes over the
    // address (the network handed it out again, say).
    let den = Identity::generate();
    pair.air_peers
        .open()
        .add(crate::testing::peer("den", &den))
        .unwrap();
    let den_peers = TempPeers::trusting(&[("air", &pair.air)]);
    let other = Host::listen(
        "127.0.0.1:0".parse().unwrap(),
        &den,
        den_peers.path(),
        info(),
        Arc::new(|_| {}),
    )
    .unwrap();
    between.lead_to(other.local_addr());
    between.fall_silent();

    assert_eq!(step(&mut viewer).unwrap(), Stepped::Cut);
    let refused = step(&mut viewer).unwrap_err();
    assert!(
        matches!(refused, Error::Link(LinkError::WrongPeer { .. })),
        "{refused}"
    );
    assert_eq!(other.viewers(), 0);
}

#[test]
fn a_host_that_is_away_is_asked_again_until_it_is_back() {
    let pair = pair();
    let between = Between::new(pair.addr);
    let mut viewer = pair.lasting_through(between.addr);
    lasting_and_shown(&pair, &mut viewer);

    // Nothing answers where the connections are passed on to, for a while.
    let nowhere = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    between.lead_to(nowhere);
    between.fall_silent();
    assert_eq!(step(&mut viewer).unwrap(), Stepped::Cut);
    let began = Instant::now();
    while began.elapsed() < Duration::from_millis(900) {
        assert!(viewer.next(Duration::from_millis(50)).unwrap().is_none());
    }
    // A few attempts in that time, a quarter of a second apart.
    let tried = between.connections() - 1;
    assert!((3..=5).contains(&tried), "{tried} attempts");

    between.lead_to(pair.addr);
    assert!(matches!(
        step(&mut viewer).unwrap(),
        Stepped::Connected { again: true, .. }
    ));
    assert_eq!(viewer.cuts(), 1);
}
