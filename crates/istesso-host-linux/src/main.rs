//! The Linux host as a program (docs/adr/0019-own-host.md).
//!
//! Started without arguments it is the runner of one session and speaks the
//! lines of ADR 0010 on standard input and output. Started as
//! `istesso-host-linux measure ...` it takes the arguments, the words and
//! the frame log of `scripts/m1-step3-host.py`, for the measuring tools.
//!
//! The threads, and which of them are on a frame's or an input's path:
//!
//! * this one: reads what the program is told, decides, answers. Not on a path.
//! * input: waits on the agent's socket and hands each event to the
//!   compositor. On the input's path.
//! * the compositor element's streaming thread and the encoder's thread:
//!   GStreamer's; a frame's path. This program's code runs on them in the
//!   gate and where an encoded frame is handed to the library.
//! * bus, standard input, frame log, sending again, the look at who watches:
//!   each a thread of its own, none on a path.

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gstreamer::prelude::*;
use istesso_agent::Paths;
use istesso_host_linux::contract::{self, Command, Display, Event};
use istesso_host_linux::encoder::{Chroma, Colour, Settings};
use istesso_host_linux::framelog::{line, FrameLog, Writer};
use istesso_host_linux::input::{self, InputSocket};
use istesso_host_linux::options::{self, CpuLatency, Measure, Sched, Switches};
use istesso_host_linux::picture::Who;
use istesso_host_linux::session::{self, Begin, Door, Flow, Session, ThroughDoor, Tongue};
use istesso_host_linux::stage::{CursorSeen, Handed, Heard, Note, Plan, Stage};
use istesso_host_linux::{cursor, render, wake};
use istesso_media::{clock, wire, Admit, Host, StreamInfo};
use istesso_pair::Identity;
use istesso_proto::InputEvent;

/// How often the session looks whether a viewer wants a key frame.
const TICK: Duration = Duration::from_millis(4);
/// How long a runner's compositor has to say how an application gets into it.
const READY_WITHIN: Duration = Duration::from_secs(10);

static STOP: AtomicBool = AtomicBool::new(false);
static KEY_FRAME: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(signal: libc::c_int) {
    // Only what a signal handler may do: set a flag.
    if signal == libc::SIGUSR1 {
        KEY_FRAME.store(true, Ordering::SeqCst);
    } else {
        STOP.store(true, Ordering::SeqCst);
    }
}

/// The library that carries the picture, as the session's door.
struct MediaDoor(Host);

impl Door for MediaDoor {
    fn offer(&self, offered: bool) {
        self.0.offer(offered);
    }

    fn admit(&self, who: &Who) -> bool {
        self.0.admit(match who {
            Who::Paired => Admit::Paired,
            Who::Only { id, public_key } => Admit::Only {
                id: id.clone(),
                public_key: public_key.clone(),
            },
            Who::Nobody => Admit::Nobody,
        });
        true
    }

    fn key_frame_wanted(&self) -> bool {
        self.0.key_frame_wanted()
    }

    fn viewers(&self) -> usize {
        self.0.viewers()
    }

    fn cursor(&self, cursor: &CursorSeen) -> bool {
        let small = |number: &u32| u16::try_from(*number).ok();
        let cursor = match cursor {
            CursorSeen::Hidden => Some(wire::Cursor::Hidden),
            CursorSeen::Named(name) => Some(wire::Cursor::Named(name)),
            CursorSeen::Image {
                width,
                height,
                hotspot_x,
                hotspot_y,
                scale,
                pixels,
            } => (|| {
                Some(wire::Cursor::Image(wire::CursorImage {
                    width: small(width)?,
                    height: small(height)?,
                    hotspot_x: small(hotspot_x)?,
                    hotspot_y: small(hotspot_y)?,
                    scale: small(scale)?,
                    rgba: pixels,
                }))
            })(),
        };
        // The library checks the cursor again and refuses one that may not travel.
        cursor.is_some_and(|cursor| self.0.set_cursor(&cursor).is_ok())
    }

    fn frame(&self, data: &[u8], pts_ns: u64, encoded_ns: u64, key: bool) -> Option<Handed> {
        match self.0.send_frame(data, pts_ns, encoded_ns, key) {
            Ok(sent) => Some(Handed {
                seq: sent.seq,
                viewers: sent.viewers,
                queued: sent.queued,
                messages: sent.messages,
            }),
            Err(error) => {
                eprintln!("error: the frame was not sent: {error}");
                None
            }
        }
    }
}

/// What both ways of starting the program come down to.
struct Setup {
    settings: Settings,
    /// Where to listen for the device: one address, or a runner's choice of ports.
    listen: Vec<String>,
    config_dir: Option<PathBuf>,
    log: Option<PathBuf>,
    /// `None`: no input is taken.
    input_socket: Option<PathBuf>,
    admit: Admit,
    cursor: bool,
    render_on_commit: bool,
    switches: Switches,
    runner: bool,
}

/// What the threads tell this one.
enum Told {
    Line(String),
    /// Nobody is going to tell this program anything more.
    Silence,
    Note(Note),
    Heard(Heard),
}

fn say(line: String) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn remark(line: String) {
    eprintln!("{line}");
}

fn listen(setup: &Setup) -> Result<Host, String> {
    let paths = match &setup.config_dir {
        Some(directory) => Paths::at(directory.clone()),
        None => Paths::from_env(),
    };
    // An identity is never made here: a device without one is paired with nobody.
    if !paths.identity().exists() {
        return Err("this device has no identity yet; pair it first".into());
    }
    let identity = Identity::load_or_create(&paths.identity())
        .map_err(|error| format!("this device's identity cannot be read: {error}"))?;
    let info = StreamInfo {
        codec: wire::Codec::H265,
        width: setup.settings.width,
        height: setup.settings.height,
        fps: setup.settings.fps,
        chroma: match setup.settings.chroma {
            Chroma::C420 => wire::Chroma::Quarter,
            Chroma::C444 => wire::Chroma::Full,
        },
        colour: match setup.settings.colour {
            Colour::Srgb => wire::Colour::Srgb,
            Colour::Default => wire::Colour::Unspecified,
        },
    };
    let mut last = "there is nowhere to listen".to_string();
    for place in &setup.listen {
        let address: Option<SocketAddr> =
            place.to_socket_addrs().ok().and_then(|mut all| all.next());
        let Some(address) = address else {
            last = format!("`{place}` is not an address with a port");
            continue;
        };
        let log = Arc::new(|line: String| eprintln!("istesso-media: {line}"));
        match Host::listen_admitting(
            address,
            &identity,
            paths.peers(),
            info,
            log,
            setup.admit.clone(),
        ) {
            Ok(host) => return Ok(host),
            Err(error) => last = format!("cannot listen for paired devices: {error}"),
        }
    }
    Err(last)
}

/// Whether the elements a plugin makes under this name have this property,
/// asked of the plugin without making one.
fn plugin_has(element: &str, property: &str) -> bool {
    let Some(factory) = gstreamer::ElementFactory::find(element).and_then(|it| it.load().ok())
    else {
        return false;
    };
    gstreamer::glib::object::ObjectClass::from_type(factory.element_type())
        .is_some_and(|class| class.find_property(property).is_some())
}

fn spawn(name: &str, work: impl FnOnce() + Send + 'static) -> Result<JoinHandle<()>, String> {
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(work)
        .map_err(|error| format!("a thread cannot be started: {error}"))
}

fn run(mut setup: Setup, display: Display) -> Result<(), String> {
    gstreamer::init().map_err(|error| format!("GStreamer does not start: {error}"))?;
    // For trying this program where there is no compositor plugin and no
    // graphics card to encode on: other elements in their places. Never set
    // where the program is used.
    let set = |name: &str| std::env::var(name).ok().filter(|text| !text.is_empty());
    let stand_in = set("ISTESSO_HOST_COMPOSITOR_SIDE");
    let pacing = if stand_in.is_some() || plugin_has("waylanddisplaysrc", "render-on-commit") {
        if setup.render_on_commit {
            "on-commit"
        } else {
            "beat"
        }
    } else {
        // The plugin as it comes, or the first patch, which had no switch.
        if setup.render_on_commit {
            remark("note: this compositor plugin has no render-on-commit property; it paces frames the way it was built".into());
        }
        "as-built"
    };
    let mut render_node = "the compositor's choice".to_string();
    if stand_in.is_none() {
        // A compositor left to choose may take a card the encoder is not on.
        match &setup.settings.render_node {
            Some(told) => render_node = told.clone(),
            None => {
                let found =
                    render::of_the_encoder(Path::new(render::CARDS), Path::new(render::DEVICES));
                if let Some(node) = found.as_deref().and_then(Path::to_str) {
                    render_node = format!("{node} (chosen: the encoder's card)");
                    setup.settings.render_node = Some(node.to_string());
                }
            }
        }
        if setup.cursor && !plugin_has("waylanddisplaysrc", "cursor-messages") {
            return Err("this compositor plugin has no cursor-messages property; build it with the second patch in scripts/patches".into());
        }
        // Before the compositor is started: one that cannot render ends without saying why.
        render::check(
            setup.settings.render_node.as_deref().map(Path::new),
            Path::new(render::DEVICES),
        )?;
    }
    let door = Arc::new(MediaDoor(listen(&setup)?));
    let port = door.0.local_addr().port();

    // The switches, and which of them could be had.
    let mut switches = setup.switches.listed();
    // Whether real-time scheduling can be had is tried on a thread that is
    // made for the trying, so that this thread stays as it is.
    let mut sched = setup.switches.sched != Sched::Off;
    if sched {
        let tried = std::thread::spawn(|| wake::real_time(None))
            .join()
            .unwrap_or(Err("the trying thread ended badly".to_string()));
        if let Err(why) = tried {
            sched = false;
            switches.retain(|switch| !switch.starts_with("sched="));
            switches.push(format!("sched (refused: {why})"));
            remark(format!(
                "note: real-time scheduling cannot be had ({why}); the threads stay as they are"
            ));
        }
    }
    let tasks = Path::new("/proc/self/task");
    let threads_before = wake::threads(tasks);
    let tight_us = match setup.switches.cpu_latency {
        CpuLatency::Off => None,
        CpuLatency::Shown { us } | CpuLatency::Input { us, .. } => Some(us),
    };
    let latch = tight_us.and_then(|us| match wake::Latch::open(Path::new(wake::DEVICE), us) {
        Ok(latch) => Some(Arc::new(latch)),
        Err(error) => {
            switches.retain(|switch| !switch.starts_with("cpu-latency"));
            switches.push(format!("cpu-latency (refused: {error})"));
            remark(format!(
                "note: {} cannot be opened ({error}); the machine sleeps as it always does",
                wake::DEVICE
            ));
            None
        }
    });

    let (log, writer): (FrameLog, Option<Writer>) = match &setup.log {
        Some(path) => {
            let (log, writer) = FrameLog::create(path, clock::now_ns).map_err(|error| {
                format!(
                    "the frame log {} cannot be started: {error}",
                    path.display()
                )
            })?;
            (log, Some(writer))
        }
        None => (FrameLog::none(), None),
    };

    let (told_in, told) = channel::<Told>();
    let (notes_in, notes) = channel::<Note>();
    let stage = Stage::build(Plan {
        compositor_side: stand_in
            .clone()
            .unwrap_or_else(|| setup.settings.compositor_side()),
        // Also for trying what happens without an encoder, where there is one:
        // an encoder's side that cannot be made is refused like a real one.
        encoder_side: set("ISTESSO_HOST_ENCODER_SIDE")
            .unwrap_or_else(|| setup.settings.encoder_side()),
        waits: pacing == "on-commit",
        log: log.clone(),
        clock: clock::now_ns,
        out: Arc::new(ThroughDoor(door.clone())),
        notes: notes_in,
        // The compositor element's streaming thread and the encoder's thread say
        // so themselves, the first time a frame passes on them.
        on_path_thread: sched.then(|| {
            let log = log.clone();
            let ask: istesso_host_linux::stage::PathThread = Box::new(move |thread: &str| {
                log.line(line::sched(thread, &wake::real_time(None)));
            });
            ask
        }),
    })?;
    {
        let compositor = stage.compositor();
        if compositor.find_property("render-on-commit").is_some() {
            compositor.set_property("render-on-commit", setup.render_on_commit);
        }
        if compositor.find_property("window-messages").is_some() {
            // Told how many windows the application has, so that its end can be passed on.
            compositor.set_property("window-messages", true);
        }
        if setup.cursor && compositor.find_property("cursor-messages").is_some() {
            // From here on the cursor is not in the frames.
            compositor.set_property("cursor-messages", true);
        }
    }
    let settings = &setup.settings;
    log.line(line::header(&line::Header {
        width: settings.width,
        height: settings.height,
        fps: settings.fps,
        bitrate_kbit: settings.bitrate,
        gop: settings.gop,
        vbv_kbit: settings.rate_buffer().unwrap_or(0),
        chroma: match settings.chroma {
            Chroma::C420 => "420",
            Chroma::C444 => "444",
        },
        colour: match settings.colour {
            Colour::Srgb => "srgb",
            Colour::Default => "default",
        },
        preset: &settings.preset,
        pacing,
        cursor: if setup.cursor { "told" } else { "in-picture" },
        render_node: &render_node,
        switches: &switches,
    }));

    let stopping = Arc::new(AtomicBool::new(false));
    let mut threads = Vec::new();

    // Input: its socket is there before anything says that the program is ready.
    let taken = Arc::new(AtomicU64::new(0));
    if let Some(path) = &setup.input_socket {
        let socket = InputSocket::bind(path).map_err(|error| {
            format!(
                "the input socket {} cannot be made: {error}",
                path.display()
            )
        })?;
        let to_compositor = stage
            .to_compositor()
            .ok_or("the compositor has no way to take input")?;
        let (stopping, log, taken) = (stopping.clone(), log.clone(), taken.clone());
        let spin = Duration::from_millis(u64::from(setup.switches.spin_after_input));
        let after_input = match setup.switches.cpu_latency {
            CpuLatency::Input { ms, .. } => latch
                .clone()
                .map(|latch| (latch, u64::from(ms) * 1_000_000)),
            _ => None,
        };
        threads.push(spawn("istesso-host-input", move || {
            if sched {
                log.line(line::sched("input", &wake::real_time(None)));
            }
            let listened = socket.listen(spin, &stopping, clock::now_ns, |event, now| {
                if let Some((latch, for_ns)) = &after_input {
                    latch.input(now + for_ns);
                }
                if log.on() && matches!(event, InputEvent::Key { pressed: true, .. }) {
                    // When a key press was handed to the compositor. Never which key.
                    log.line(line::key_in(clock::now_ns()));
                }
                to_compositor.input(&event, now);
                taken.fetch_add(1, Ordering::Relaxed);
            });
            if let Err(error) = listened {
                eprintln!("error: input is no longer taken: {error}");
            }
        })?);
    }
    // The bus.
    {
        let (listener, told_in, stopping) = (stage.listener(), told_in.clone(), stopping.clone());
        threads.push(spawn("istesso-host-bus", move || {
            while !stopping.load(Ordering::Relaxed) {
                if let Some(heard) = listener.hear(Duration::from_millis(100)) {
                    if told_in.send(Told::Heard(heard)).is_err() {
                        break;
                    }
                }
            }
        })?);
    }
    // What the pipeline's threads report.
    {
        let told_in = told_in.clone();
        threads.push(spawn("istesso-host-notes", move || {
            for note in notes {
                if told_in.send(Told::Note(note)).is_err() {
                    break;
                }
            }
        })?);
    }
    // Sending the last frame again while a device watches a picture that stands still.
    if setup.switches.keep_awake > 0 {
        let (repeater, door, stopping) = (stage.repeater(), door.clone(), stopping.clone());
        let every = Duration::from_secs(1) / setup.switches.keep_awake;
        let hold = Duration::from_millis(u64::from(setup.switches.keep_awake_hold));
        threads.push(spawn("istesso-host-again", move || {
            while !stopping.load(Ordering::Relaxed) {
                let wait = if door.viewers() > 0 {
                    repeater.again(every, hold)
                } else {
                    every
                };
                std::thread::sleep(wait.max(Duration::from_millis(1)));
            }
        })?);
    }
    // Keeping the machine from sleeping deeply, only while that is wanted.
    if let Some(latch) = latch.clone() {
        let (door, stopping) = (door.clone(), stopping.clone());
        let shown = matches!(setup.switches.cpu_latency, CpuLatency::Shown { .. });
        threads.push(spawn("istesso-host-awake", move || {
            while !stopping.load(Ordering::Relaxed) {
                if shown {
                    let _ = latch.set(door.viewers() > 0);
                    std::thread::sleep(Duration::from_millis(100));
                } else {
                    latch.look(clock::now_ns());
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            let _ = latch.set(false);
        })?);
    }

    // Whether the compositor has made a frame. It makes its output for its
    // first frame, and a window whose first commit comes before there is an
    // output is never told its size and never shown. A runner says `ready`
    // only after that. Looked at once: the probe takes itself off the pad
    // with the first frame, and nothing stays on the frames' path.
    let first_frame = Arc::new(AtomicBool::new(false));
    if let Some(pad) = stage.compositor().static_pad("src") {
        let seen = first_frame.clone();
        pad.add_probe(gstreamer::PadProbeType::BUFFER, move |_, _| {
            seen.store(true, Ordering::SeqCst);
            gstreamer::PadProbeReturn::Remove
        });
    }

    let tongue = if setup.runner {
        Tongue::Runner { display, port }
    } else {
        Tongue::Measure
    };
    let (mut session, at_start) = Session::start(
        stage,
        Begin {
            door: door.clone(),
            tongue,
            say: Box::new(say),
            remark: Box::new(remark),
            log: log.clone(),
            clock: clock::now_ns,
            cursor_told: setup.cursor,
            // Read on a thread of their own, while the pipeline starts.
            cursors: setup
                .cursor
                .then(|| cursor::Book::open(cursor::theme_directories())),
        },
    )?;

    if sched {
        // The compositor plugin's own threads have started with the pipeline.
        let own = std::fs::read_to_string("/proc/self/comm").unwrap_or_default();
        let after = wake::threads(tasks);
        let main = std::process::id() as i32;
        for thread in wake::started_unnamed(&threads_before, &after, own.trim_end(), main) {
            log.line(line::sched("compositor", &wake::real_time(Some(thread))));
        }
        if setup.switches.sched == Sched::RtDriver {
            let found = wake::of_the_driver(&after);
            if found.is_empty() {
                log.line(line::sched(
                    "driver",
                    &Err("no thread of the driver's was found".to_string()),
                ));
            }
            for thread in found {
                log.line(line::sched("driver", &wake::real_time(Some(thread))));
            }
        }
    }
    let mut outcome = Ok(());
    if stand_in.is_some() {
        // What stands in for the compositor never says how an application gets into it.
        session.heard(Heard::Environment(Vec::new()));
    }
    if setup.runner {
        // Ready once the compositor has said how an application gets into
        // it, and has made its first frame: from then on it can place a window.
        let began = Instant::now();
        while (session.environment().is_none() || !first_frame.load(Ordering::SeqCst))
            && outcome.is_ok()
        {
            match told.recv_timeout(TICK) {
                Ok(Told::Heard(heard)) => {
                    if let Flow::End(end) = session.heard(heard) {
                        outcome = end.and(Err("the compositor ended before it was ready".into()));
                    }
                }
                Ok(Told::Note(note)) => session.note(note),
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) if began.elapsed() < READY_WITHIN => {}
                Err(_) => {
                    outcome = Err(if session.environment().is_none() {
                        "the compositor did not say how an application gets into it".into()
                    } else {
                        "the compositor did not make a first frame".into()
                    })
                }
            }
        }
        if outcome.is_ok() {
            let env: BTreeMap<String, String> = session
                .environment()
                .unwrap_or_default()
                .iter()
                .cloned()
                .collect();
            session.event(&Event::Ready {
                env,
                display,
                answer: contract::answer(port),
            });
        }
    } else {
        say("playing".to_string());
    }
    session.tell(at_start);

    // What the program is told, a line at a time. Not joined at the end: it
    // may wait for a line that never comes.
    {
        let told_in = told_in.clone();
        let _reader = spawn("istesso-host-told", move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                if told_in.send(Told::Line(line)).is_err() {
                    return;
                }
            }
            let _ = told_in.send(Told::Silence);
        })?;
    }
    drop(told_in);

    while outcome.is_ok() && !STOP.load(Ordering::SeqCst) {
        match told.recv_timeout(TICK) {
            Ok(Told::Line(line)) => session.line(&line),
            // A runner whose agent has gone ends; a measuring pipeline goes on as it is.
            Ok(Told::Silence) if setup.runner => break,
            Ok(Told::Silence) => {}
            Ok(Told::Note(note)) => session.note(note),
            Ok(Told::Heard(heard)) => {
                if let Flow::End(end) = session.heard(heard) {
                    outcome = end;
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => session.tick(),
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if KEY_FRAME.swap(false, Ordering::SeqCst) {
            session.key_frame("signal");
        }
    }

    stopping.store(true, Ordering::SeqCst);
    // Each frame whose time stamp had to be moved on is in the log; this is how many.
    log.line(line::restamps(session.stage().restamped()));
    session.stop();
    drop(session);
    for thread in threads {
        let _ = thread.join();
    }
    if setup.input_socket.is_some() {
        log.line(line::input(taken.load(Ordering::Relaxed)));
    }
    drop(log);
    if let Some(writer) = writer {
        writer
            .finish()
            .map_err(|error| format!("the frame log was not written to its end: {error}"))?;
    }
    outcome
}

fn measure(run_with: Measure) -> Result<(), String> {
    let display = Display {
        width: run_with.settings.width,
        height: run_with.settings.height,
        fps: Some(run_with.settings.fps),
    };
    let input_socket = run_with
        .input
        .then(|| run_with.input_socket.unwrap_or_else(input::default_socket));
    run(
        Setup {
            settings: run_with.settings,
            listen: vec![run_with.bind],
            config_dir: run_with.config_dir,
            log: run_with.log,
            input_socket,
            admit: if run_with.admit_nobody {
                Admit::Nobody
            } else {
                Admit::Paired
            },
            cursor: run_with.cursor,
            render_on_commit: run_with.render_on_commit,
            switches: run_with.switches,
            runner: false,
        },
        display,
    )
}

/// The ports a runner may listen on: `ISTESSO_HOST_PORTS=FIRST-LAST`, or
/// the ones the rehearsal used. The first that is free is taken and told in
/// `ready`.
fn runner_ports() -> Vec<String> {
    let (first, last) = std::env::var("ISTESSO_HOST_PORTS")
        .ok()
        .and_then(|ports| {
            let (first, last) = ports.split_once('-')?;
            Some((first.parse::<u16>().ok()?, last.parse::<u16>().ok()?))
        })
        .unwrap_or((47850, 47899));
    (first..=last.max(first))
        .map(|port| format!("[::]:{port}"))
        .collect()
}

fn runner() -> Result<(), String> {
    let get = |name: &str| std::env::var(name).ok().filter(|text| !text.is_empty());
    let switches = Switches::from_env(get)?;
    // The first line that says anything is `start`. An agent that goes away before it ends this.
    let (wished, peer, input_socket) = loop {
        let mut line = String::new();
        match std::io::stdin().lock().read_line(&mut line) {
            Ok(0) | Err(_) => return Ok(()),
            Ok(_) => {}
        }
        if let Some(Command::Start {
            display,
            peer,
            input_socket,
            ..
        }) = contract::read(&line)
        {
            break (display, peer, input_socket);
        }
    };
    let fps = wished.fps.filter(|fps| *fps > 0).unwrap_or(85);
    let mut settings = Settings::decided(wished.width, wished.height, fps);
    settings.render_node = get("ISTESSO_HOST_RENDER_NODE");
    let display = Display {
        width: settings.width,
        height: settings.height,
        fps: Some(fps),
    };
    let admit = match session::who(peer) {
        Who::Only { id, public_key } => Admit::Only { id, public_key },
        _ => Admit::Nobody,
    };
    let outcome = run(
        Setup {
            settings,
            listen: runner_ports(),
            config_dir: get("ISTESSO_HOST_CONFIG_DIR").map(PathBuf::from),
            log: get("ISTESSO_HOST_LOG").map(PathBuf::from),
            input_socket: Some(PathBuf::from(input_socket)),
            admit,
            // The device draws the cursor itself, where its own pointer is.
            cursor: get("ISTESSO_HOST_CURSOR").is_none_or(|cursor| cursor != "in-picture"),
            render_on_commit: true,
            switches,
            runner: true,
        },
        display,
    );
    if let Err(message) = &outcome {
        // The agent is told why, in a line it knows; this program ends after it.
        say(contract::write(&Event::Error {
            message: message.clone(),
        }));
    }
    outcome
}

fn main() -> ExitCode {
    // SAFETY: the handler only stores to atomics, which a signal handler may do.
    unsafe {
        let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGUSR1, handler);
    }
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let outcome = match arguments.first().map(String::as_str) {
        None => runner(),
        Some("measure") => options::measure(&arguments[1..]).and_then(measure),
        Some(other) => Err(format!(
            "`{other}` is not a way to start this program: without arguments it is the runner of a \
             session (ADR 0010); `measure` takes the arguments of scripts/m1-step3-host.py"
        )),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}
