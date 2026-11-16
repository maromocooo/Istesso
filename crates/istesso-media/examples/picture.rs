//! Tools for trying the picture's path without an encoder or a decoder.
//!
//! ```text
//! picture watch ADDR [--config DIR] [--peer NAME] [--seconds S] [--keep FILE] [--ask-every S]
//!     Connects to a host as a viewer does, takes its frames for S seconds
//!     (10 unless given) and says what came: how many frames, how far apart,
//!     how long from a frame's first bytes to having it whole. With --keep
//!     the frames are written to FILE as they came. With --ask-every a key
//!     frame is asked for every so many seconds.
//! picture replay FILE --bind ADDR [--config DIR] [--fps N]
//!     Shows a stream kept by `watch` to paired devices, again and again,
//!     N frames a second (85 unless given): a host without a compositor and
//!     an encoder, for trying a viewer on one machine.
//! picture pair DIR
//!     Makes DIR/host and DIR/device, two configuration directories of
//!     devices that are paired with each other, for `--config`: a host and
//!     a viewer on one machine that leave the machine's own identity and
//!     pairings alone.
//! ```
//!
//! The device's own identity and list of paired devices are used unless
//! --config says otherwise. Nothing here prints an address or a key.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use istesso_agent::Paths;
use istesso_link::LocalKeys;
use istesso_media::wire::{self, StreamInfo};
use istesso_media::{Host, Viewer};
use istesso_pair::{Hello, Identity, Peer, PeerStore};

type Failure = Box<dyn std::error::Error>;

const MAGIC: &str = "istesso-picture 1";

struct Args(Vec<String>);

impl Args {
    /// The value that follows `--name`, taken out of the arguments.
    fn option(&mut self, name: &str) -> Option<String> {
        let at = self.0.iter().position(|arg| arg == name)?;
        if at + 1 >= self.0.len() {
            return None;
        }
        self.0.remove(at);
        Some(self.0.remove(at))
    }

    fn paths(&mut self) -> Paths {
        match self.option("--config") {
            Some(dir) => Paths::at(PathBuf::from(dir)),
            None => Paths::from_env(),
        }
    }
}

fn identity(paths: &Paths) -> Result<Identity, Failure> {
    if !paths.identity().exists() {
        return Err("this device has no identity yet; pair it first".into());
    }
    Ok(Identity::load_or_create(&paths.identity())?)
}

fn percentile(sorted: &[u64], share: usize) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[(sorted.len() * share).div_ceil(100).max(1) - 1] as f64 / 1e6
}

fn spread(what: &str, mut nanos: Vec<u64>) {
    nanos.sort_unstable();
    println!(
        "{what}: median {:.3} ms, 95% {:.3}, 99% {:.3}, longest {:.3}",
        percentile(&nanos, 50),
        percentile(&nanos, 95),
        percentile(&nanos, 99),
        percentile(&nanos, 100)
    );
}

fn watch(mut args: Args) -> Result<(), Failure> {
    let paths = args.paths();
    let peer = args.option("--peer");
    let seconds: f64 = args.option("--seconds").map_or(Ok(10.0), |s| s.parse())?;
    let ask_every: Option<f64> = args.option("--ask-every").map(|s| s.parse()).transpose()?;
    let mut keep = match args.option("--keep") {
        Some(path) => Some(std::fs::File::create_new(path)?),
        None => None,
    };
    let addr = args
        .0
        .first()
        .ok_or("which host? give its address and port")?;
    let addr = addr.to_socket_addrs()?.next().ok_or("that is no address")?;

    let keys = LocalKeys::new(&identity(&paths)?)?;
    let peers = PeerStore::open(paths.peers())?;
    let expected = match &peer {
        Some(name) => Some(
            peers
                .find(name)?
                .ok_or("no paired device of that name")?
                .id
                .clone(),
        ),
        None => None,
    };
    let began = Instant::now();
    let mut viewer = Viewer::connect(
        addr,
        &keys,
        &peers,
        expected.as_deref(),
        Duration::from_secs(3),
    )?;
    println!(
        "connected to a paired host in {:.1} ms; its picture: {}",
        began.elapsed().as_secs_f64() * 1e3,
        serde_json::to_string(viewer.info())?
    );
    if let Some(file) = &mut keep {
        writeln!(file, "{MAGIC}\n{}", serde_json::to_string(viewer.info())?)?;
    }

    let asker = viewer.asker();
    let (mut frames, mut keys_seen, mut jumps, mut bytes) = (0u64, 0u64, 0u64, 0u64);
    let (mut gaps, mut arriving) = (Vec::new(), Vec::new());
    let (mut last_first, mut last_seq, mut largest) = (None::<u64>, None::<u64>, 0usize);
    let mut asked = Instant::now();
    let mut requests = 0;
    let ended = loop {
        if began.elapsed().as_secs_f64() >= seconds {
            break None;
        }
        if ask_every.is_some_and(|every| asked.elapsed().as_secs_f64() >= every) {
            asked = Instant::now();
            requests += 1;
            asker.ask_for_key_frame();
        }
        let frame = match viewer.next_frame(Duration::from_millis(100)) {
            Ok(Some(frame)) => frame,
            Ok(None) => continue,
            Err(err) => break Some(err),
        };
        frames += 1;
        keys_seen += u64::from(frame.key_frame);
        bytes += frame.data.len() as u64;
        largest = largest.max(frame.data.len());
        if last_seq.is_some_and(|last| frame.seq != last + 1) || frame.discontinuity {
            jumps += 1;
        }
        if let Some(last) = last_first {
            gaps.push(frame.first_ns - last);
        }
        arriving.push(frame.whole_ns - frame.first_ns);
        (last_first, last_seq) = (Some(frame.first_ns), Some(frame.seq));
        if let Some(file) = &mut keep {
            let mut packet = Vec::new();
            let header = wire::Header {
                kind: wire::Kind::Frame,
                key_frame: frame.key_frame,
                discontinuity: false,
                seq: frame.seq,
                pts_ns: frame.pts_ns,
                encoded_ns: frame.encoded_ns,
            };
            wire::encode(&header, frame.data, &mut packet);
            file.write_all(&(packet.len() as u32).to_le_bytes())?;
            file.write_all(&packet)?;
        }
    };
    let took = began.elapsed().as_secs_f64();
    println!(
        "{frames} frames in {took:.1} s ({:.1} a second), {keys_seen} of them key frames; {:.2} Mbit/s, \
         largest frame {largest} bytes; {jumps} places where frames were missing; {requests} key frames asked for",
        frames as f64 / took,
        bytes as f64 * 8.0 / took / 1e6
    );
    spread("from one frame's first bytes to the next one's", gaps);
    spread(
        "from a frame's first bytes to having it whole and opened",
        arriving,
    );
    match ended {
        Some(err) => println!("the stream ended: {err}"),
        None => println!("stopped after the time asked for; the stream was still running"),
    }
    Ok(())
}

fn replay(mut args: Args) -> Result<(), Failure> {
    let paths = args.paths();
    let bind = args
        .option("--bind")
        .ok_or("where to listen? --bind ADDR:PORT")?;
    let fps: f64 = args.option("--fps").map_or(Ok(85.0), |s| s.parse())?;
    let path = args.0.first().ok_or("which file?")?;
    let mut file = BufReader::new(std::fs::File::open(path)?);
    let (mut magic, mut info) = (String::new(), String::new());
    file.read_line(&mut magic)?;
    file.read_line(&mut info)?;
    if magic.trim_end() != MAGIC {
        return Err("that file was not written by `picture watch --keep`".into());
    }
    let info: StreamInfo = serde_json::from_str(&info)?;
    let mut frames = Vec::new();
    let mut length = [0u8; 4];
    while file.read_exact(&mut length).is_ok() {
        let mut packet = vec![0u8; u32::from_le_bytes(length) as usize];
        file.read_exact(&mut packet)?;
        let (header, at) = wire::decode(&packet)?;
        frames.push((header, packet[at].to_vec()));
    }
    // A viewer can only begin with a key frame, so the stream begins with one.
    let first = frames.iter().position(|(header, _)| header.key_frame);
    let frames = &frames[first.ok_or("there is no key frame in that file")?..];

    let log = Arc::new(|line: String| eprintln!("istesso-media: {line}"));
    let addr = bind.to_socket_addrs()?.next().ok_or("that is no address")?;
    let host = Host::listen(addr, &identity(&paths)?, paths.peers(), info, log)?;
    println!(
        "showing {} frames, {fps} a second, on port {}",
        frames.len(),
        host.local_addr().port()
    );
    let began = Instant::now();
    let mut shown = 0u64;
    let mut at = 0;
    loop {
        // Somebody needs a key frame: the file's first frame is one.
        if host.key_frame_wanted() || at == frames.len() {
            at = 0;
        }
        let (header, frame) = &frames[at];
        // The frames keep their place in the file as their time stamp, so
        // that a viewer's log can be set against the file.
        host.send_frame(
            frame,
            header.pts_ns,
            istesso_media::clock::now_ns(),
            header.key_frame,
        )?;
        at += 1;
        shown += 1;
        let due = began + Duration::from_secs_f64(shown as f64 / fps);
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
    }
}

fn pair(args: Args) -> Result<(), Failure> {
    let root = PathBuf::from(args.0.first().ok_or("which directory?")?);
    let (host, device) = (Paths::at(root.join("host")), Paths::at(root.join("device")));
    let host_identity = Identity::load_or_create(&host.identity())?;
    let device_identity = Identity::load_or_create(&device.identity())?;
    for (paths, name, other) in [
        (&host, "device", &device_identity),
        (&device, "host", &host_identity),
    ] {
        PeerStore::open(paths.peers())?.add(Peer::from_hello(&Hello {
            device_name: name.into(),
            public_key: other.public_key(),
        }))?;
    }
    println!("made host and device below the directory given, paired with each other");
    Ok(())
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let command = if args.is_empty() {
        String::new()
    } else {
        args.remove(0)
    };
    let result = match command.as_str() {
        "watch" => watch(Args(args)),
        "replay" => replay(Args(args)),
        "pair" => pair(Args(args)),
        _ => {
            eprintln!("usage: picture watch ADDR [...] | picture replay FILE --bind ADDR [...] | picture pair DIR");
            std::process::exit(2);
        }
    };
    if let Err(err) = result {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
