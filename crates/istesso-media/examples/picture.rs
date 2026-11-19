//! Tools for trying the picture's path without an encoder or a decoder.
//!
//! ```text
//! picture watch ADDR [--config DIR] [--peer NAME] [--seconds S] [--keep FILE] [--ask-every S]
//!     Connects to a host as a viewer does, takes its frames for S seconds
//!     (10 unless given) and says what came: how many frames, how far apart,
//!     how long from a frame's first bytes to having it whole. With --keep
//!     the frames are written to FILE as they came. With --ask-every a key
//!     frame is asked for every so many seconds.
//! picture replay FILE --bind ADDR [--config DIR] [--fps N] [--cursor image|hidden|named]
//!     Shows a stream kept by `watch` to paired devices, again and again,
//!     N frames a second (85 unless given): a host without a compositor and
//!     an encoder, for trying a viewer on one machine. With --cursor it also
//!     says what the cursor is: a made-up image (a magenta triangle of 64
//!     pixels with a white edge, its tip the hot spot), hidden, or one
//!     without an image.
//! picture stay ADDR [--config DIR] [--peer NAME] [--seconds S]
//!     A viewer that lasts: it takes the host's frames as `watch` does, and
//!     when the connection is cut it connects again. Says each thing that
//!     happens to the connection as it happens (connected, cut, the first
//!     key frame after a cut and how long that took, over and why), and
//!     ends when the host said that it is over or after S seconds (20
//!     unless given).
//! picture between LISTEN TO [--silent-after S] [--close-after S]
//!     Stands between a viewer and a host: what connects to LISTEN is
//!     passed on to TO, both ways. For trying what a viewer does when its
//!     connection is cut. A line on standard input tells it what to do with
//!     the connections there are (later ones are passed on as before):
//!     `silent` (they stay open and carry nothing any more, as a network
//!     does that drops a connection which both ends still believe in),
//!     `close` (they are closed). --silent-after and --close-after do the
//!     same once, S seconds after the first connection was made.
//! picture pair DIR
//!     Makes DIR/host and DIR/device, two configuration directories of
//!     devices that are paired with each other, for `--config`: a host and
//!     a viewer on one machine that leave the machine's own identity and
//!     pairings alone.
//! ```
//!
//! The device's own identity and list of paired devices are used unless
//! --config says otherwise, which only a development build (`dev-paths`)
//! accepts. Nothing here prints an address or a key.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use istesso_agent::Paths;
use istesso_link::LocalKeys;
use istesso_media::wire::{self, Cursor, CursorImage, StreamInfo};
use istesso_media::{Event, Host, Lasting, Step, Viewer};
use istesso_pair::{Hello, Identity, Peer};

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

    /// As the library does (`ffi.rs`): only a development build
    /// (`dev-paths`) takes its key and its paired devices from a directory
    /// that whoever starts it names. Any other build uses the device's own,
    /// and refuses the option rather than ignoring it.
    fn paths(&mut self) -> Result<Paths, Failure> {
        match self.option("--config") {
            Some(dir) if istesso_agent::paths::DEV_PATHS => Ok(Paths::at(PathBuf::from(dir))),
            Some(_) => Err(
                "this build keeps its identity and its paired devices where \
                            istesso does: --config needs a build with the dev-paths feature"
                    .into(),
            ),
            None => Ok(Paths::from_env()),
        }
    }
}

fn identity(paths: &Paths) -> Result<Identity, Failure> {
    paths
        .keeping()
        .identity_if_any()?
        .ok_or_else(|| "this device has no identity yet; pair it first".into())
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
    let paths = args.paths()?;
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
    let peers = paths.keeping().peers()?;
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
        // The first frame of a viewer that came while the stream ran says
        // that frames went before it; that is no gap.
        if last_seq.is_some_and(|last| frame.seq != last + 1 || frame.discontinuity) {
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
    let paths = args.paths()?;
    let bind = args
        .option("--bind")
        .ok_or("where to listen? --bind ADDR:PORT")?;
    let fps: f64 = args.option("--fps").map_or(Ok(85.0), |s| s.parse())?;
    let cursor = args.option("--cursor");
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
    let host = Host::listen(addr, &identity(&paths)?, paths.keeping(), info, log)?;
    // A cursor that cannot be mistaken for anything on the screen: a triangle with its
    // tip at the top left, where the hot spot is.
    let mut triangle = vec![0u8; 64 * 64 * 4];
    for (i, pixel) in triangle.chunks_exact_mut(4).enumerate() {
        let (x, y) = (i % 64, i / 64);
        if x <= y && y < 60 {
            let edge = x == 0 || x + 2 > y || y > 57;
            pixel.copy_from_slice(if edge {
                &[255, 255, 255, 255]
            } else {
                &[255, 0, 255, 255]
            });
        }
    }
    match cursor.as_deref() {
        None => {}
        Some("hidden") => host.set_cursor(&Cursor::Hidden)?,
        Some("named") => host.set_cursor(&Cursor::Named("default"))?,
        Some(_) => host.set_cursor(&Cursor::Image(CursorImage {
            width: 64,
            height: 64,
            hotspot_x: 0,
            hotspot_y: 0,
            scale: 1,
            rgba: &triangle,
        }))?,
    }
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
    let host_identity = host.keeping().identity()?;
    let device_identity = device.keeping().identity()?;
    for (paths, name, other) in [
        (&host, "device", &device_identity),
        (&device, "host", &host_identity),
    ] {
        paths.keeping().peers()?.add(Peer::from_hello(&Hello {
            device_name: name.into(),
            public_key: other.public_key(),
        }))?;
    }
    println!("made host and device below the directory given, paired with each other");
    Ok(())
}

fn stay(mut args: Args) -> Result<(), Failure> {
    let paths = args.paths()?;
    let peer = args.option("--peer");
    let seconds: f64 = args.option("--seconds").map_or(Ok(20.0), |s| s.parse())?;
    let addr = args
        .0
        .first()
        .ok_or("which host? give its address and port")?;
    let addr = addr.to_socket_addrs()?.next().ok_or("that is no address")?;

    let mut viewer = Lasting::new(
        Box::new(move |began_with, within| {
            // Read anew for every connection: the pairing may have changed.
            // Never made here: a device without an identity has nothing to connect with.
            let identity = paths
                .keeping()
                .identity_if_any()?
                .ok_or(istesso_media::Error::NoIdentity)?;
            let keys = LocalKeys::new(&identity)?;
            let peers = paths.keeping().peers()?;
            let expected = match (began_with, &peer) {
                (Some(id), _) => Some(id.to_string()),
                (None, Some(name)) => Some(
                    peers
                        .find(name)?
                        .ok_or(istesso_link::Error::UnknownPeer)?
                        .id
                        .clone(),
                ),
                (None, None) => None,
            };
            Viewer::connect(addr, &keys, &peers, expected.as_deref(), within)
        }),
        None,
    );
    let began = Instant::now();
    let at = |began: Instant| format!("{:7.2} s", began.elapsed().as_secs_f64());
    let (mut frames, mut since_cut, mut waiting_for_key) = (0u64, None::<Instant>, false);
    let over = loop {
        if began.elapsed().as_secs_f64() >= seconds {
            break None;
        }
        match viewer.next(Duration::from_millis(100)) {
            Ok(None) => {}
            Ok(Some(Step::Event(Event::Frame(frame)))) => {
                frames += 1;
                if waiting_for_key && frame.key_frame {
                    waiting_for_key = false;
                    match since_cut.take() {
                        Some(cut) => println!(
                            "{}  the first key frame after the cut, {:.0} ms after it was noticed",
                            at(began),
                            cut.elapsed().as_secs_f64() * 1e3
                        ),
                        None => println!("{}  the first key frame", at(began)),
                    }
                }
            }
            Ok(Some(Step::Event(Event::Cursor(_)))) => {}
            Ok(Some(Step::Cut)) => {
                since_cut = Some(Instant::now());
                println!("{}  cut after {frames} frames; connecting again", at(began));
            }
            Ok(Some(Step::Connected { again, silent_for })) => {
                waiting_for_key = true;
                if again {
                    println!(
                        "{}  connected again; the host had not been heard for {:.0} ms",
                        at(began),
                        silent_for.as_secs_f64() * 1e3
                    );
                } else {
                    println!("{}  connected to a paired host", at(began));
                }
            }
            Err(err) => break Some(err),
        }
    };
    match over {
        Some(err) => println!("{}  over, and not tried again: {err}", at(began)),
        None => println!(
            "{}  stopped after the time asked for; {frames} frames, {} cuts",
            at(began),
            viewer.cuts()
        ),
    }
    Ok(())
}

/// What `between` does with the connections there are.
#[derive(Default)]
struct Passing {
    /// Connections numbered below this carry nothing any more.
    silent_below: std::sync::atomic::AtomicU64,
    /// Connections numbered below this are closed.
    closed_below: std::sync::atomic::AtomicU64,
    made: std::sync::atomic::AtomicU64,
}

fn pass_on(
    mut from: std::net::TcpStream,
    mut to: std::net::TcpStream,
    number: u64,
    passing: &Passing,
) {
    use std::sync::atomic::Ordering::SeqCst;
    let _ = from.set_read_timeout(Some(Duration::from_millis(20)));
    let mut buffer = [0u8; 65536];
    loop {
        if number < passing.closed_below.load(SeqCst) {
            let _ = from.shutdown(std::net::Shutdown::Both);
            let _ = to.shutdown(std::net::Shutdown::Both);
            return;
        }
        match from.read(&mut buffer) {
            Ok(0) => break,
            // Read, and gone: the sender is not held up, and nothing arrives.
            Ok(_) if number < passing.silent_below.load(SeqCst) => {}
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
    // One end has closed. The other is told, unless this connection carries nothing any
    // more: then it is left as it is, as a network would leave it.
    if number >= passing.silent_below.load(SeqCst) {
        let _ = to.shutdown(std::net::Shutdown::Both);
    }
}

fn between(mut args: Args) -> Result<(), Failure> {
    use std::sync::atomic::Ordering::SeqCst;
    let silent_after: Option<f64> = args
        .option("--silent-after")
        .map(|s| s.parse())
        .transpose()?;
    let close_after: Option<f64> = args
        .option("--close-after")
        .map(|s| s.parse())
        .transpose()?;
    let [listen, to] = &args.0[..] else {
        return Err("where to listen, and where to pass connections on to?".into());
    };
    let to = to.to_socket_addrs()?.next().ok_or("that is no address")?;
    let listener = std::net::TcpListener::bind(listen.as_str())?;
    let passing = Arc::new(Passing::default());
    let began = Instant::now();
    let at = move || format!("{:7.2} s", began.elapsed().as_secs_f64());

    // What it is told on standard input.
    {
        let passing = passing.clone();
        std::thread::spawn(move || {
            for line in std::io::stdin().lines().map_while(Result::ok) {
                let made = passing.made.load(SeqCst);
                match line.trim() {
                    "silent" => {
                        passing.silent_below.store(made, SeqCst);
                        println!(
                            "{}  the {made} connection(s) so far carry nothing any more",
                            at()
                        );
                    }
                    "close" => {
                        passing.closed_below.store(made, SeqCst);
                        println!("{}  the {made} connection(s) so far are closed", at());
                    }
                    "" => {}
                    other => eprintln!("note: {other:?} is not something this program can be told"),
                }
            }
        });
    }
    println!("passing connections on; `silent` or `close` on standard input");
    for near in listener.incoming() {
        let near = near?;
        let number = passing.made.fetch_add(1, SeqCst);
        println!("{}  connection {}", at(), number + 1);
        if number == 0 {
            for (after, silent) in [(silent_after, true), (close_after, false)] {
                let Some(after) = after else { continue };
                let passing = passing.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs_f64(after));
                    let made = passing.made.load(SeqCst);
                    if silent {
                        passing.silent_below.store(made, SeqCst);
                        println!(
                            "{}  the {made} connection(s) so far carry nothing any more",
                            at()
                        );
                    } else {
                        passing.closed_below.store(made, SeqCst);
                        println!("{}  the {made} connection(s) so far are closed", at());
                    }
                });
            }
        }
        let Ok(far) = std::net::TcpStream::connect_timeout(&to, Duration::from_secs(1)) else {
            println!("{}  nothing answers where it is to be passed on to", at());
            continue;
        };
        let _ = (near.set_nodelay(true), far.set_nodelay(true));
        for (from, to) in [(near.try_clone()?, far.try_clone()?), (far, near)] {
            let passing = passing.clone();
            std::thread::spawn(move || pass_on(from, to, number, &passing));
        }
    }
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
        "stay" => stay(Args(args)),
        "between" => between(Args(args)),
        "pair" => pair(Args(args)),
        _ => {
            eprintln!(
                "usage: picture watch ADDR [...] | picture replay FILE --bind ADDR [...] | picture stay ADDR [...] | \
                 picture between LISTEN TO [...] | picture pair DIR"
            );
            std::process::exit(2);
        }
    };
    if let Err(err) = result {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
