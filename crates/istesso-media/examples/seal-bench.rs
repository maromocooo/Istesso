//! How long it takes to seal a frame for the encrypted channel and to open
//! it again, on this machine.
//!
//! ```sh
//! cargo run --release -p istesso-media --example seal-bench [-- FILE]
//! ```
//!
//! Two ends of a channel are set up as for a stream (the handshake of the
//! control channel), and then frames are passed from one to the other
//! through memory instead of a network: what is timed is the work the
//! encryption adds on each machine, without any waiting for packets.
//!
//!   seal   a frame in the encoder's hands, to the bytes that go to the
//!          socket: the packet header, the copy, ChaCha20-Poly1305
//!   open   those bytes, to the frame in the viewer's hands
//!
//! The sizes are a frame of the text picture a verdict is measured with
//! (7.5 kB), a frame of a hard picture at the bit rate that was decided
//! (45 kB), and one at more than three times that rate (140 kB). With FILE
//! the table is written there instead of the terminal, for a run started as
//! an application (`open`), which has no terminal.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use istesso_link::{Channel, LocalKeys};
use istesso_media::wire::{self, Header, Kind};
use istesso_pair::Identity;

/// A connection for the handshake that turns into a piece of memory.
struct Line {
    tcp: TcpStream,
    memory: Arc<Mutex<VecDeque<u8>>>,
    in_memory: Arc<AtomicBool>,
}

impl Read for Line {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if !self.in_memory.load(Ordering::Relaxed) {
            return self.tcp.read(buf);
        }
        self.memory.lock().unwrap().read(buf)
    }
}

impl Write for Line {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if !self.in_memory.load(Ordering::Relaxed) {
            return self.tcp.write(buf);
        }
        self.memory.lock().unwrap().extend(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn percentile(sorted: &[u64], share: usize) -> u64 {
    sorted[(sorted.len() * share).div_ceil(100).max(1) - 1]
}

fn row(what: &str, mut nanos: Vec<u64>) -> String {
    nanos.sort_unstable();
    let micro = |ns: u64| ns as f64 / 1000.0;
    let mean = nanos.iter().sum::<u64>() as f64 / nanos.len() as f64 / 1000.0;
    format!(
        "  {what:<6}{mean:>10.1}{:>10.1}{:>10.1}{:>10.1}{:>10.1}\n",
        micro(percentile(&nanos, 50)),
        micro(percentile(&nanos, 95)),
        micro(percentile(&nanos, 99)),
        micro(*nanos.last().unwrap()),
    )
}

fn main() {
    let memory = Arc::new(Mutex::new(VecDeque::new()));
    let in_memory = Arc::new(AtomicBool::new(false));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let line = |tcp: TcpStream| Line {
        tcp,
        memory: memory.clone(),
        in_memory: in_memory.clone(),
    };

    let host_keys = LocalKeys::new(&Identity::generate()).unwrap();
    let viewer_keys = LocalKeys::new(&Identity::generate()).unwrap();
    // The system queues the caller until it is taken.
    let calling = TcpStream::connect(addr).unwrap();
    let (answered, _) = listener.accept().unwrap();
    let answering = {
        let line = line(answered);
        std::thread::spawn(move || Channel::respond(line, &host_keys, |_| true).unwrap())
    };
    let mut viewer = Channel::initiate(line(calling), &viewer_keys, |_| Ok(())).unwrap();
    let mut host = answering.join().unwrap();
    in_memory.store(true, Ordering::Relaxed);

    let mut out = String::new();
    out.push_str(
        "Sealing a frame for the encrypted channel and opening it, in microseconds (2000 frames each)\n",
    );
    for len in [7_500usize, 45_000, 140_000] {
        let frame: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_mul(131)).collect();
        let mut packet = Vec::new();
        let (mut seal, mut open) = (Vec::new(), Vec::new());
        for i in 0..2200u64 {
            let header = Header {
                kind: Kind::Frame,
                key_frame: false,
                discontinuity: false,
                seq: i,
                pts_ns: i,
                encoded_ns: i,
            };
            let began = Instant::now();
            wire::encode(&header, &frame, &mut packet);
            host.send(&packet).unwrap();
            let sealed = Instant::now();
            let got = viewer.recv().unwrap();
            let (_, at) = wire::decode(&got).unwrap();
            let opened = Instant::now();
            assert_eq!(got[at].len(), len);
            // The first frames warm the caches up.
            if i >= 200 {
                seal.push((sealed - began).as_nanos() as u64);
                open.push((opened - sealed).as_nanos() as u64);
            }
        }
        out.push_str(&format!(
            "{len} bytes ({} of the channel's messages)\n  {:<6}{:>10}{:>10}{:>10}{:>10}{:>10}\n",
            wire::channel_messages(len),
            "",
            "mean",
            "median",
            "95%",
            "99%",
            "max"
        ));
        out.push_str(&row("seal", seal));
        out.push_str(&row("open", open));
    }
    match std::env::args().nth(1) {
        Some(path) => std::fs::write(path, out).unwrap(),
        None => print!("{out}"),
    }
}
