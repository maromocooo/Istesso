//! `istesso audio`: the sound of one machine, played on another.
//!
//! What is here so far is what the first tests need. A source is what
//! applications play into a sink made for it (Linux, when built with the
//! `pipewire` feature), a test tone, or raw audio read from a pipe. An exit
//! plays into the real output device but hands it silence: what it would
//! have played is kept for checking instead.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::{Subcommand, ValueEnum};

use istesso_agent::Paths;
use istesso_audio::probe::Tone;
use istesso_audio::session::{self, Source, StreamInfo};
use istesso_audio::source::{capture_ring, feed_reader, feed_tone};
use istesso_audio::{ledger, probe, sim, wire};

/// Frames per second and channels of the streams this version sends.
const STREAM_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;

#[derive(Subcommand)]
pub enum AudioAction {
    /// Offer sound to paired devices that may have it
    ///
    /// The sound is what applications play into an output made for it
    /// (`--source pipewire`), a test tone, or raw audio read from standard
    /// input (32-bit floats, little-endian, two channels, 48 kHz). Only
    /// paired devices that were given the `audio` right are sent anything:
    /// `istesso peers allow <device> audio`.
    Serve {
        /// TCP port to listen on
        #[arg(long, default_value_t = wire::DEFAULT_PORT)]
        port: u16,
        /// Address to listen on (default: every network of this machine)
        #[arg(long, default_value_t = IpAddr::V4(Ipv4Addr::UNSPECIFIED))]
        bind: IpAddr,
        /// Where the sound comes from
        #[arg(long, value_enum, default_value_t = SourceArg::Tone)]
        source: SourceArg,
        /// Pitch of the test tone in Hz
        #[arg(long, default_value_t = 997.0)]
        tone_hz: f64,
        /// Level of the test tone in dB relative to full scale
        #[arg(long, default_value_t = -12.0, allow_negative_numbers = true)]
        tone_db: f64,
        /// Frames per packet
        #[arg(long, default_value_t = 256)]
        packet_frames: usize,
        /// Stop after this many seconds (default: run until interrupted)
        #[arg(long)]
        seconds: Option<f64>,
        /// Write a line for every packet sent to this file, for
        /// `istesso audio compare`
        #[arg(long)]
        ledger: Option<PathBuf>,
        /// With `--source pipewire`: the name programs address the output by
        #[arg(long, default_value = "istesso-audio")]
        node_name: String,
        /// With `--source pipewire`: the name people see in a list of outputs
        #[arg(long, default_value = "Istesso")]
        node_description: String,
        /// With `--source pipewire`: frames per cycle to ask the sound
        /// system for (default: ask for nothing)
        #[arg(long)]
        quantum: Option<u32>,
        /// With `--source pipewire`: a further property of the output, as
        /// `key=value`; may be given several times
        #[arg(long)]
        node_property: Vec<String>,
        /// Stop, as at the end of `--seconds`, once this file exists
        #[arg(long)]
        stop_file: Option<PathBuf>,
    },
    /// A test tool: put a quiet tone into the sound system of this machine,
    /// connected to nothing (Linux, PipeWire)
    ///
    /// The tone reaches an output only when it is linked to one by hand,
    /// e.g. `pw-link istesso-audio-test-tone:capture_FL <output>:playback_FL`.
    TestTone {
        /// The name programs address the tone's node by
        #[arg(long, default_value = "istesso-audio-test-tone")]
        node_name: String,
        /// Pitch of the tone in Hz
        #[arg(long, default_value_t = 997.0)]
        tone_hz: f64,
        /// Level of the tone in dB relative to full scale
        #[arg(long, default_value_t = -60.0, allow_negative_numbers = true)]
        tone_db: f64,
        /// Frames per cycle to ask the sound system for
        #[arg(long, default_value_t = 256)]
        quantum: u32,
        /// Stop after this many seconds (default: run until interrupted)
        #[arg(long)]
        seconds: Option<f64>,
        /// A further property of the node, as `key=value`; may be given
        /// several times
        #[arg(long)]
        node_property: Vec<String>,
        /// Stop once this file exists
        #[arg(long)]
        stop_file: Option<PathBuf>,
    },
    /// Play the sound of a paired host on this machine
    ///
    /// For now the output device is handed silence, so nothing is heard:
    /// the audio that would have been played is measured, and written to a
    /// file with --record.
    Play {
        /// Address of the source, e.g. `studio.local` or `192.0.2.20:47802`
        #[arg(required_unless_present = "reference")]
        address: Option<String>,
        /// Play from a source inside this program instead of from the
        /// network: a reference for what this machine and its output
        /// device skip and break on their own. No address is given
        #[arg(long, conflicts_with = "address")]
        reference: bool,
        /// Let the sound be heard: hand it to the output device. Without
        /// this the device gets silence and the sound is only measured
        #[arg(long, conflicts_with = "reference")]
        audible: bool,
        /// Id or name of the paired device expected there (default: any
        /// paired device)
        #[arg(long)]
        peer: Option<String>,
        /// Name of the output device (default: the system's default output)
        #[arg(long)]
        output: Option<String>,
        /// How long the sound waits between arriving and being handed to
        /// the output device, in milliseconds
        #[arg(long, default_value_t = 40.0)]
        latency_ms: f64,
        /// Stop after this many seconds (default: run until interrupted)
        #[arg(long)]
        seconds: Option<f64>,
        /// Write what would have been played to this file (32-bit floats,
        /// little-endian, at the rate of the output device)
        #[arg(long)]
        record: Option<PathBuf>,
        /// Pitch of the test tone to look for in what would have been
        /// played (0 for none)
        #[arg(long, default_value_t = 997.0)]
        tone_hz: f64,
        /// Write a line for every packet received to this file, for
        /// `istesso audio compare`
        #[arg(long)]
        ledger: Option<PathBuf>,
        /// Write the measurements as JSON to this file (default: standard
        /// output)
        #[arg(long)]
        report: Option<PathBuf>,
        /// Keep trying to reach the source for this many seconds before
        /// giving up
        #[arg(long, default_value_t = 0.0)]
        connect_for: f64,
        /// Stop, and write the measurements, once this file exists
        #[arg(long)]
        stop_file: Option<PathBuf>,
        /// Keep this machine from going to sleep while playing
        #[arg(long)]
        keep_awake: bool,
    },
    /// List the devices sound can come out of on this machine
    Outputs {
        #[arg(long)]
        json: bool,
    },
    /// Look at a recording of the test tone: level, pitch, gaps, clicks
    Check {
        /// Raw audio: 32-bit floats, little-endian
        file: PathBuf,
        /// Frames per second of the recording
        #[arg(long, default_value_t = 48_000.0)]
        rate: f64,
        /// Channels in the file; the first one is looked at
        #[arg(long, default_value_t = 2)]
        channels: usize,
        /// Pitch the tone was made with, in Hz
        #[arg(long, default_value_t = 997.0)]
        tone_hz: f64,
    },
    /// Compare what a source sent with what an exit received, packet by
    /// packet (the files written with --ledger)
    Compare { sent: PathBuf, received: PathBuf },
    /// Run the playout against simulated clocks and print what happened
    ///
    /// No sound device and no network are involved: a source, a network and
    /// a sound card are simulated, each with a clock of its own.
    Simulate {
        /// Simulated time in hours
        #[arg(long, default_value_t = 8.0)]
        hours: f64,
        /// How much faster than true time the source's clock runs, in parts
        /// per million
        #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
        source_ppm: f64,
        /// How much faster than true time the sound card's clock runs
        #[arg(long, default_value_t = 100.0, allow_negative_numbers = true)]
        device_ppm: f64,
        /// Frames per second of the sound card
        #[arg(long, default_value_t = 48_000.0)]
        device_rate: f64,
        /// Frames the sound card asks for at a time
        #[arg(long, default_value_t = 512)]
        callback_frames: usize,
        /// Frames per packet
        #[arg(long, default_value_t = 256)]
        packet_frames: usize,
        /// The wait the playout holds, in milliseconds
        #[arg(long, default_value_t = 40.0)]
        latency_ms: f64,
        /// Up to how much later than the fastest a packet arrives, in
        /// milliseconds
        #[arg(long, default_value_t = 0.5)]
        jitter_ms: f64,
        /// A span without delivery, as `start:length` in seconds; may be
        /// given several times
        #[arg(long)]
        stall: Vec<String>,
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum SourceArg {
    /// A steady sine
    Tone,
    /// Raw audio from standard input
    Stdin,
    /// What applications play into an output made for it (Linux, PipeWire)
    Pipewire,
}

pub fn run(action: AudioAction) -> Result<(), String> {
    match action {
        AudioAction::Serve {
            port,
            bind,
            source,
            tone_hz,
            tone_db,
            packet_frames,
            seconds,
            ledger,
            node_name,
            node_description,
            quantum,
            node_property,
            stop_file,
        } => serve(ServeOptions {
            addr: SocketAddr::new(bind, port),
            source,
            tone_hz,
            tone_db,
            packet_frames,
            seconds,
            ledger,
            node_name,
            node_description,
            quantum,
            node_properties: node_property,
            stop_file,
        }),
        AudioAction::Play {
            address,
            reference,
            audible,
            peer,
            output,
            latency_ms,
            seconds,
            record,
            tone_hz,
            ledger,
            report,
            connect_for,
            stop_file,
            keep_awake,
        } => play(PlayOptions {
            address: address.unwrap_or_default(),
            reference,
            audible,
            peer,
            output,
            latency_ms,
            seconds,
            record,
            tone_hz,
            ledger,
            report,
            connect_for,
            stop_file,
            keep_awake,
        }),
        AudioAction::TestTone {
            node_name,
            tone_hz,
            tone_db,
            quantum,
            seconds,
            node_property,
            stop_file,
        } => test_tone(ToneOptions {
            node_name,
            tone_hz,
            tone_db,
            quantum,
            seconds,
            node_properties: node_property,
            stop_file,
        }),
        AudioAction::Outputs { json } => outputs(json),
        AudioAction::Check {
            file,
            rate,
            channels,
            tone_hz,
        } => check(&file, rate, channels, tone_hz),
        AudioAction::Compare { sent, received } => compare(&sent, &received),
        AudioAction::Simulate {
            hours,
            source_ppm,
            device_ppm,
            device_rate,
            callback_frames,
            packet_frames,
            latency_ms,
            jitter_ms,
            stall,
            seed,
        } => {
            let mut scenario = sim::Scenario::quiet(hours * 3600.0);
            scenario.source_ppm = source_ppm;
            scenario.device_ppm = device_ppm;
            scenario.device_rate = device_rate;
            scenario.callback_frames = callback_frames;
            scenario.callback_lead_s = callback_frames as f64 / device_rate + 0.001;
            scenario.packet_frames = packet_frames;
            scenario.target_s = latency_ms * 1e-3;
            scenario.jitter_s = jitter_ms * 1e-3;
            scenario.stalls = stall
                .iter()
                .map(|text| parse_stall(text.as_str()))
                .collect::<Result<_, _>>()?;
            scenario.seed = seed;
            let outcome = sim::run(&scenario);
            super::print_json(&serde_json::json!({ "scenario": scenario, "outcome": outcome }))
        }
    }
}

fn parse_stall(text: &str) -> Result<(f64, f64), String> {
    let wrong = || format!("'{text}' is not a stall; write it as start:length, in seconds");
    let (start, length) = text.split_once(':').ok_or_else(wrong)?;
    match (start.trim().parse::<f64>(), length.trim().parse::<f64>()) {
        (Ok(start), Ok(length)) if start >= 0.0 && length > 0.0 => Ok((start, length)),
        _ => Err(wrong()),
    }
}

// ---------------------------------------------------------------------------
// serve
// ---------------------------------------------------------------------------

struct ServeOptions {
    addr: SocketAddr,
    source: SourceArg,
    tone_hz: f64,
    tone_db: f64,
    packet_frames: usize,
    seconds: Option<f64>,
    ledger: Option<PathBuf>,
    node_name: String,
    node_description: String,
    quantum: Option<u32>,
    node_properties: Vec<String>,
    stop_file: Option<PathBuf>,
}

/// A thread that feeds audio into the capture ring.
type Feeder = std::thread::JoinHandle<std::io::Result<()>>;
/// What a source that captures has to say about itself at the end.
type CaptureReport = Box<dyn Fn() -> serde_json::Value>;

fn parse_property(text: &str) -> Result<(String, String), String> {
    match text.split_once('=') {
        Some((key, value)) if !key.trim().is_empty() => {
            Ok((key.trim().to_string(), value.trim().to_string()))
        }
        _ => Err(format!("'{text}' is not a property; write it as key=value")),
    }
}

#[cfg(all(target_os = "linux", feature = "pipewire"))]
fn pipewire_feeder(
    options: &ServeOptions,
    capture: istesso_audio::source::CaptureWriter,
    stop: Arc<AtomicBool>,
) -> Result<(Feeder, CaptureReport), String> {
    use istesso_audio::linux::{run_sink, SinkOptions, SinkStats};

    if options
        .quantum
        .is_some_and(|frames| !(16..=8192).contains(&frames))
    {
        return Err("a cycle holds between 16 and 8192 frames".to_string());
    }
    let sink = SinkOptions {
        node_name: options.node_name.clone(),
        description: options.node_description.clone(),
        quantum: options.quantum,
        properties: options
            .node_properties
            .iter()
            .map(|text| parse_property(text))
            .collect::<Result<_, _>>()?,
    };
    let stats = Arc::new(SinkStats::default());
    let seen = stats.clone();
    let feeder = std::thread::spawn(move || {
        run_sink(
            &sink,
            capture,
            stop,
            seen,
            Arc::new(|line| eprintln!("{line}")),
        )
        .map_err(|err| std::io::Error::other(err.to_string()))
    });
    let report = move || serde_json::to_value(stats.snapshot()).unwrap_or_default();
    Ok((feeder, Box::new(report)))
}

#[cfg(not(all(target_os = "linux", feature = "pipewire")))]
fn pipewire_feeder(
    options: &ServeOptions,
    _capture: istesso_audio::source::CaptureWriter,
    _stop: Arc<AtomicBool>,
) -> Result<(Feeder, CaptureReport), String> {
    let _ = (
        &options.node_name,
        &options.node_description,
        options.quantum,
    );
    for text in &options.node_properties {
        parse_property(text)?;
    }
    Err(
        "this build cannot capture from PipeWire: build it on Linux with `--features pipewire`"
            .to_string(),
    )
}

struct ToneOptions {
    node_name: String,
    tone_hz: f64,
    tone_db: f64,
    quantum: u32,
    seconds: Option<f64>,
    node_properties: Vec<String>,
    stop_file: Option<PathBuf>,
}

#[cfg(all(target_os = "linux", feature = "pipewire"))]
fn test_tone(options: ToneOptions) -> Result<(), String> {
    use istesso_audio::linux::{run_test_tone, SinkOptions};

    if !(16..=8192).contains(&options.quantum) {
        return Err("a cycle holds between 16 and 8192 frames".to_string());
    }
    let node = SinkOptions {
        node_name: options.node_name.clone(),
        description: "Istesso audio test tone".to_string(),
        quantum: Some(options.quantum),
        properties: options
            .node_properties
            .iter()
            .map(|text| parse_property(text))
            .collect::<Result<_, _>>()?,
    };
    let amplitude = 10f64.powf(options.tone_db.min(0.0) / 20.0) as f32;
    let tone = Tone::new(options.tone_hz, amplitude, f64::from(STREAM_RATE));
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let player = std::thread::spawn(move || {
        run_test_tone(
            &node,
            STREAM_RATE,
            u32::from(CHANNELS),
            tone,
            stopping,
            Arc::new(|line| eprintln!("{line}")),
        )
    });
    let begin = Instant::now();
    while !player.is_finished() {
        let over = options
            .seconds
            .is_some_and(|limit| begin.elapsed().as_secs_f64() >= limit);
        if over || options.stop_file.as_ref().is_some_and(|file| file.exists()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    stop.store(true, Ordering::Relaxed);
    match player.join() {
        Ok(result) => result.map_err(|err| err.to_string()),
        Err(_) => Err("the thread that plays the tone stopped unexpectedly".to_string()),
    }
}

#[cfg(not(all(target_os = "linux", feature = "pipewire")))]
fn test_tone(options: ToneOptions) -> Result<(), String> {
    let _ = (
        options.node_name,
        options.tone_hz,
        options.tone_db,
        options.quantum,
        options.seconds,
        options.stop_file,
    );
    for text in &options.node_properties {
        parse_property(text)?;
    }
    Err(
        "this build has no PipeWire test tone: build it on Linux with `--features pipewire`"
            .to_string(),
    )
}

fn serve(options: ServeOptions) -> Result<(), String> {
    if !(16..=wire::MAX_PACKET_FRAMES).contains(&options.packet_frames) {
        return Err(format!(
            "a packet holds between 16 and {} frames",
            wire::MAX_PACKET_FRAMES
        ));
    }
    let paths = Paths::from_env();
    let identity = super::load_identity()?;
    let allowed = super::open_peers()?
        .peers()
        .iter()
        .filter(|peer| peer.permissions.contains(&istesso_pair::Permission::Audio))
        .count();

    // A second of room; when sending falls more than 100 ms behind, the
    // oldest audio is let go until 20 ms are left.
    let (mut capture, packetizer) = capture_ring(
        usize::from(CHANNELS),
        f64::from(STREAM_RATE),
        1.0,
        0.1,
        0.02,
    );
    let sent = options
        .ledger
        .as_ref()
        .map(|_| Arc::new(Mutex::new(Vec::with_capacity(session::NIGHT_OF_PACKETS))));
    let source = Source::bind(
        options.addr,
        &identity,
        paths.peers(),
        StreamInfo::new(STREAM_RATE, CHANNELS, options.packet_frames as u32),
        packetizer,
        sent.clone(),
        Arc::new(|line| eprintln!("{line}")),
    )
    .map_err(|err| format!("cannot offer sound on {}: {err}", options.addr))?;
    let addr = source.local_addr().map_err(|e| e.to_string())?;
    let handle = source.stopper();
    println!(
        "Offering {} on port {}. Paired devices that may have it: {allowed}.",
        match options.source {
            SourceArg::Tone => "a test tone",
            SourceArg::Stdin => "the audio on standard input",
            SourceArg::Pipewire => "what applications play into the output made for it",
        },
        addr.port()
    );
    if allowed == 0 {
        println!("Allow a paired device with `istesso peers allow <device> audio`.");
    }
    std::thread::spawn(move || source.run());

    let stop = Arc::new(AtomicBool::new(false));
    let feeding = stop.clone();
    let packet_frames = options.packet_frames;
    let seconds = options.seconds;
    let mut capture_report: Option<CaptureReport> = None;
    let feeder: Feeder = match options.source {
        SourceArg::Tone => {
            let amplitude = 10f64.powf(options.tone_db.min(0.0) / 20.0) as f32;
            let mut tone = Tone::new(options.tone_hz, amplitude, f64::from(STREAM_RATE));
            std::thread::spawn(move || {
                feed_tone(&mut capture, &mut tone, packet_frames, seconds, &feeding);
                Ok(())
            })
        }
        SourceArg::Stdin => std::thread::spawn(move || {
            let input = std::io::stdin().lock();
            feed_reader(&mut capture, input, packet_frames, &feeding)
        }),
        SourceArg::Pipewire => {
            let (feeder, report) = pipewire_feeder(&options, capture, feeding)?;
            capture_report = Some(report);
            feeder
        }
    };
    // The tone ends by itself after `seconds`; a pipe ends when its writer
    // does. With a limit and a pipe that goes on, stop at the limit.
    let begin = Instant::now();
    let mut late_cycles_said = 0u64;
    let mut watchdog_said = 0u64;
    let mut record = record::Record::open("source", &[]);
    let mut record_at = Instant::now();
    let source_totals = |capture: &serde_json::Value| {
        [
            (
                "clicks",
                capture["late_cycles"].as_u64().unwrap_or(0) as f64,
            ),
            ("late_ms", capture["late_total_ms"].as_f64().unwrap_or(0.0)),
            ("cycles", capture["cycles"].as_u64().unwrap_or(0) as f64),
            ("pauses", capture["resumed"].as_u64().unwrap_or(0) as f64),
            (
                "watchdog_late",
                capture["watchdog_late"].as_u64().unwrap_or(0) as f64,
            ),
        ]
    };
    while !feeder.is_finished() {
        if seconds.is_some_and(|limit| begin.elapsed().as_secs_f64() >= limit + 0.5) {
            break;
        }
        // A capture cycle that began late is said at once, with the time,
        // so that it can be laid beside what the exit saw.
        if let Some(late) = capture_report.as_ref().map(|report| report()) {
            let cycles = late["late_cycles"].as_u64().unwrap_or(0);
            if cycles > late_cycles_said {
                late_cycles_said = cycles;
                eprintln!(
                    "{:>8.1} s since the start: a capture cycle began {:.1} ms late ({} so far)",
                    begin.elapsed().as_secs_f64(),
                    late["late_last_ms"].as_f64().unwrap_or(0.0),
                    cycles
                );
            }
            // So is a late wake-up of the watchdog beside the capture: the
            // two at the same moment mean that the whole machine stood
            // still; the capture alone, that PipeWire's side was late.
            let wakes = late["watchdog_late"].as_u64().unwrap_or(0);
            if wakes > watchdog_said {
                watchdog_said = wakes;
                eprintln!(
                    "{:>8.1} s since the start: the watchdog beside the capture woke {:.1} ms late ({} so far)",
                    begin.elapsed().as_secs_f64(),
                    late["watchdog_late_last_ms"].as_f64().unwrap_or(0.0),
                    wakes
                );
            }
        }
        if record_at.elapsed() >= Duration::from_secs(1) {
            record_at = Instant::now();
            if let Some(capture) = capture_report.as_ref().map(|report| report()) {
                record.tick(&source_totals(&capture));
            }
        }
        if options.stop_file.as_ref().is_some_and(|file| file.exists()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if let Some(capture) = capture_report.as_ref().map(|report| report()) {
        record.end(&source_totals(&capture));
    }
    stop.store(true, Ordering::Relaxed);
    if feeder.is_finished() {
        match feeder.join() {
            Ok(Ok(())) => {}
            Ok(Err(err)) => eprintln!("the source of the audio failed: {err}"),
            Err(_) => eprintln!("the thread that feeds the audio stopped unexpectedly"),
        }
    }
    // Let what is still in the ring go out, then tell the listener.
    std::thread::sleep(Duration::from_millis(150));
    handle.stop();
    std::thread::sleep(Duration::from_millis(100));

    if let (Some(path), Some(sent)) = (&options.ledger, &sent) {
        let entries = sent.lock().unwrap_or_else(|e| e.into_inner());
        let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        ledger::write(&entries, std::io::BufWriter::new(file))
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    let mut report = serde_json::to_value(handle.stats()).map_err(|e| e.to_string())?;
    if let Some(fields) = report.as_object_mut() {
        let sending = serde_json::to_value(handle.sending()).map_err(|e| e.to_string())?;
        fields.insert("sending".to_string(), sending);
        if let Some(capture) = capture_report {
            fields.insert("capture".to_string(), capture());
        }
    }
    super::print_json(&report)
}

// ---------------------------------------------------------------------------
// play
// ---------------------------------------------------------------------------

struct PlayOptions {
    address: String,
    reference: bool,
    audible: bool,
    peer: Option<String>,
    output: Option<String>,
    latency_ms: f64,
    seconds: Option<f64>,
    record: Option<PathBuf>,
    tone_hz: f64,
    ledger: Option<PathBuf>,
    report: Option<PathBuf>,
    connect_for: f64,
    stop_file: Option<PathBuf>,
    keep_awake: bool,
}

fn resolve(address: &str) -> Result<SocketAddr, String> {
    let with_port = super::with_default_port(address.trim(), wire::DEFAULT_PORT);
    with_port
        .to_socket_addrs()
        .map_err(|err| format!("cannot find '{address}': {err}"))?
        .next()
        .ok_or_else(|| format!("'{address}' has no address"))
}

#[cfg(not(target_os = "macos"))]
fn play(options: PlayOptions) -> Result<(), String> {
    let _ = (
        resolve(&options.address)?,
        options.peer,
        options.output,
        options.latency_ms,
        options.seconds,
        options.record,
        options.tone_hz,
        options.ledger,
        options.report,
        options.connect_for,
        options.stop_file,
        options.keep_awake,
        options.reference,
        options.audible,
    );
    Err("playing is only built for macOS so far".to_string())
}

#[cfg(not(target_os = "macos"))]
fn outputs(_json: bool) -> Result<(), String> {
    Err("listing outputs is only built for macOS so far".to_string())
}

#[cfg(target_os = "macos")]
fn outputs(json: bool) -> Result<(), String> {
    let devices = istesso_audio::mac::outputs().map_err(|e| e.to_string())?;
    if json {
        return super::print_json(&devices);
    }
    for device in devices {
        println!(
            "{} {:<40} {:>6.0} Hz  buffer {:>4} frames",
            if device.is_default { "*" } else { " " },
            device.name,
            device.sample_rate,
            device.buffer_frames
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn play(options: PlayOptions) -> Result<(), String> {
    mac::play(options)
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;

    use std::io::Write;
    use std::sync::atomic::{AtomicI64, AtomicU64};

    use istesso_audio::clock;
    use istesso_audio::fifo::queue;
    use istesso_audio::mac::Output;
    use istesso_audio::outlet::{Outlet, OutletCounts};
    use istesso_audio::playout::{Playout, PlayoutConfig, PlayoutStats, WaitEvent};
    use istesso_audio::probe::Spread;
    use istesso_audio::resample::Sinc;
    use istesso_audio::rtrb::RingBuffer;
    use istesso_audio::session::{
        clock_offset, pump_local, pump_reconnecting, subscribe, Arrivals, ClockOffset, Ended,
        Rejoining, SILENT_CONNECTION,
    };
    use istesso_link::LocalKeys;

    /// How the device asked for audio.
    #[derive(Default)]
    pub(super) struct Callbacks {
        count: AtomicU64,
        frames_min: AtomicU64,
        frames_max: AtomicU64,
        /// How long before its frames are due a callback ran, microseconds.
        lead_min_us: AtomicI64,
        lead_max_us: AtomicI64,
        lead_sum_us: AtomicI64,
        /// Callbacks that began after their frames were due.
        late: AtomicU64,
        /// How long this program took in a callback at most, microseconds,
        /// and how often more than half the time of the frames asked for.
        took_max_us: AtomicU64,
        took_long: AtomicU64,
        /// Turns the device did not come for: the frames of a callback were
        /// due later than those of the one before left room for. How many
        /// turns in all, on how many occasions, and the longest such step.
        missed_turns: AtomicU64,
        missed_occasions: AtomicU64,
        step_max_us: AtomicU64,
    }

    /// Notes what one callback says about the device's turns. `due_ns` is
    /// when its frames are handed over, `before` the same for the callback
    /// before it with the time its frames lasted.
    fn note_turn(seen: &Callbacks, due_ns: u64, before: Option<(u64, u64)>) {
        let Some((due_before, lasted_ns)) = before else {
            return;
        };
        let step = due_ns.saturating_sub(due_before);
        seen.step_max_us.fetch_max(step / 1000, Ordering::Relaxed);
        // Half a turn of slack for the jitter of the time stamps.
        if lasted_ns > 0 && step > lasted_ns + lasted_ns / 2 {
            let missed = (step + lasted_ns / 2) / lasted_ns - 1;
            seen.missed_turns.fetch_add(missed, Ordering::Relaxed);
            seen.missed_occasions.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[cfg(test)]
    pub(super) fn callbacks_for_test() -> Callbacks {
        Callbacks::default()
    }

    #[cfg(test)]
    pub(super) fn note_turn_for_test(seen: &Callbacks, due_ns: u64, before: Option<(u64, u64)>) {
        note_turn(seen, due_ns, before)
    }

    #[cfg(test)]
    pub(super) fn missed_for_test(seen: &Callbacks) -> (u64, u64) {
        (
            seen.missed_turns.load(Ordering::Relaxed),
            seen.missed_occasions.load(Ordering::Relaxed),
        )
    }

    /// The longest the player waits for the outlet's fade to have been
    /// handed to the device before it stops the device all the same.
    const CLOSING_CEILING: Duration = Duration::from_millis(300);

    /// The two ends of an outlet's closing: the request, and the word that
    /// the faded block is with the device.
    struct Closing {
        ask: Arc<AtomicBool>,
        done: Arc<AtomicBool>,
    }

    /// Asks the outlet to fade to silence and waits until the device has
    /// taken the fade, however long its turns are. A device that does not
    /// come within [`CLOSING_CEILING`] is stopped all the same, and that is
    /// counted.
    fn close_outlet(closing: &Closing, counts: &OutletCounts) {
        closing.ask.store(true, Ordering::Relaxed);
        let asked = Instant::now();
        while !closing.done.load(Ordering::Relaxed) {
            if asked.elapsed() >= CLOSING_CEILING {
                counts.closed_without_fade.fetch_add(1, Ordering::Relaxed);
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// What the device's callback plays from and where it leaves what it
    /// would have played.
    struct Voice {
        playout: Playout<Sinc>,
        record_in: istesso_audio::rtrb::Producer<f32>,
        /// Seconds of sound that have been let out since the player began
        /// to be heard: the first of them are turned down.
        risen_s: f64,
    }

    impl Voice {
        /// Turns down what is heard first: see [`audible_gain`]. Silence
        /// does not count, so that the rise begins with the sound.
        fn rise(&mut self, out: &mut [f32], channels: usize, device_rate: f64) {
            if self.risen_s >= AUDIBLE_RISE_S || !out.iter().any(|sample| *sample != 0.0) {
                return;
            }
            for frame in out.chunks_exact_mut(channels.max(1)) {
                let gain = audible_gain(self.risen_s);
                for sample in frame {
                    *sample *= gain;
                }
                self.risen_s += 1.0 / device_rate;
            }
        }
    }

    /// Starts `output` with the callback that plays from `voice`.
    fn start_output(
        output: &mut Output,
        channels: usize,
        audible: bool,
        voice: Arc<Mutex<Option<Voice>>>,
        seen: Arc<Callbacks>,
        counts: Arc<OutletCounts>,
    ) -> Result<Closing, String> {
        let device_rate = output.device().sample_rate;
        // Set before the device is stopped: what is played then fades out.
        let closing = Arc::new(AtomicBool::new(false));
        let mut outlet = Outlet::new(channels, device_rate, closing.clone(), counts);
        let done = outlet.closed_flag();
        let mut turn_before: Option<(u64, u64)> = None;
        output
            .start(channels, move |frames, out, now_ns| {
                let began_ns = clock::now_ns();
                // The playout is here unless it is just being handed to
                // another device; then this turn is silence.
                match voice.try_lock().ok().as_deref_mut() {
                    Some(Some(voice)) => {
                        voice.playout.render(frames, out, now_ns);
                        outlet.clean(out);
                        let _ = voice.record_in.push_partial_slice(out);
                        if audible {
                            voice.rise(out, channels, device_rate);
                        }
                    }
                    _ => out.fill(0.0),
                }
                // Unless the person asked to hear it, the device gets
                // silence, and with it the clock of the device still drives
                // the playout.
                if !audible {
                    out.fill(0.0);
                }
                outlet.limit_and_close(out);
                // Was this program late, or did the device not come? How
                // long the work here took, and whether the frames of this
                // callback are due when those of the last one end.
                let lasts_ns = (frames as f64 / device_rate * 1e9) as u64;
                let took_ns = clock::now_ns().saturating_sub(began_ns);
                seen.took_max_us
                    .fetch_max(took_ns / 1000, Ordering::Relaxed);
                if took_ns > lasts_ns / 2 {
                    seen.took_long.fetch_add(1, Ordering::Relaxed);
                }
                note_turn(&seen, now_ns, turn_before);
                turn_before = Some((now_ns, lasts_ns));
                let lead_us = (now_ns as i64 - began_ns as i64) / 1000;
                if lead_us <= 0 {
                    seen.late.fetch_add(1, Ordering::Relaxed);
                }
                seen.count.fetch_add(1, Ordering::Relaxed);
                seen.frames_min.fetch_min(frames as u64, Ordering::Relaxed);
                seen.frames_max.fetch_max(frames as u64, Ordering::Relaxed);
                seen.lead_min_us.fetch_min(lead_us, Ordering::Relaxed);
                seen.lead_max_us.fetch_max(lead_us, Ordering::Relaxed);
                seen.lead_sum_us.fetch_add(lead_us, Ordering::Relaxed);
            })
            .map_err(|e| e.to_string())?;
        Ok(Closing { ask: closing, done })
    }

    pub fn play(options: PlayOptions) -> Result<(), String> {
        if !(5.0..=2000.0).contains(&options.latency_ms) {
            return Err("the wait is between 5 and 2000 milliseconds".to_string());
        }
        // The network side, or none: the reference plays from a source
        // inside this program and needs neither keys nor a paired device.
        let net = if options.reference {
            None
        } else {
            let addr = resolve(&options.address)?;
            let identity = super::super::load_identity()?;
            let keys = LocalKeys::new(&identity).map_err(|e| e.to_string())?;
            let peers = super::super::open_peers()?;
            let expected = match &options.peer {
                Some(selector) => Some(
                    peers
                        .find(selector)
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| format!("no paired device matches '{selector}'"))?
                        .id
                        .clone(),
                ),
                None => None,
            };
            Some((addr, identity, keys, peers, expected))
        };
        let wait = Duration::from_secs(3);
        // Playing keeps the machine awake (ADR 0005). For now the system's
        // own tool holds it awake for as long as this process lives.
        let _awake = options.keep_awake.then(|| {
            std::process::Command::new("/usr/bin/caffeinate")
                .args(["-i", "-w", &std::process::id().to_string()])
                .spawn()
                .map_err(|err| eprintln!("cannot keep this machine awake: {err}"))
                .ok()
        });

        // How the two clocks relate, for stating the latency. This is the
        // first time the source is reached. The system may still be asking
        // the person whether this program may use the local network, and it
        // only asks while the program keeps trying.
        let give_up = Instant::now() + Duration::from_secs_f64(options.connect_for.max(0.0));
        let mut offset = match &net {
            None => ClockOffset {
                source_to_local_ns: 0,
                round_trip_ns: 0,
                rounds: 0,
            },
            Some((addr, _, keys, peers, expected)) => loop {
                match clock_offset(*addr, keys, peers, expected.as_deref(), 16, wait) {
                    Ok(offset) => break offset,
                    Err(istesso_audio::Error::Io(err)) if Instant::now() < give_up => {
                        eprintln!("cannot reach the source yet ({err}); trying again");
                        std::thread::sleep(Duration::from_secs(1));
                    }
                    Err(err) => return Err(err.to_string()),
                }
            },
        };
        let (info, connection, subscription) = match &net {
            Some((addr, _, keys, peers, expected)) => {
                let subscription = subscribe(*addr, keys, peers, expected.as_deref(), wait)
                    .map_err(|e| e.to_string())?;
                let info = subscription.info().clone();
                let connection = subscription.connection().map_err(|e| e.to_string())?;
                (info, Some(connection), Some(subscription))
            }
            // What the host sends: 48 kHz, two channels, 256 frames a packet.
            None => (StreamInfo::new(48_000, 2, 256), None, None),
        };
        let channels = usize::from(info.channels);
        let stream_rate = f64::from(info.sample_rate);

        // The network side: packets into the queue. It runs from the moment
        // the source begins to send. Opening the device can take a second
        // or two; packets left in the connection for that long would make
        // the source's sending wait, and the source would let audio go.
        // Room for four seconds of audio between the network and the device.
        let (mut queue_in, queue_out) = queue(channels, (stream_rate * 4.0) as usize);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_pump = stop.clone();
        let cuts = Arc::new(AtomicU64::new(0));
        let cuts_seen = cuts.clone();
        let keep_ledger = options.ledger.is_some();
        // One packet's time, for the scheduling of the thread that receives.
        let packet_time =
            Duration::from_secs_f64(f64::from(info.packet_frames.max(64)) / stream_rate);
        // The receiving thread makes a new connection when one is cut. It
        // gets keys and a list of paired devices of its own for that.
        let pumping = if let (Some((addr, identity, _, _, expected)), Some(subscription)) =
            (&net, subscription)
        {
            let addr = *addr;
            let again_keys = LocalKeys::new(identity).map_err(|e| e.to_string())?;
            let again_peers = super::super::open_peers()?;
            let again_expected = expected.clone();
            std::thread::spawn(move || {
                // The receiving thread is part of the audio path: scheduled as
                // the sound card's thread is, so that it is not held up.
                let real_time = || {
                    if !istesso_audio::mac::make_this_thread_real_time(
                        packet_time,
                        Duration::from_micros(1500),
                        Duration::from_millis(3),
                    ) {
                        eprintln!("the receiving thread could not be given real-time scheduling");
                    }
                };
                real_time();
                let mut arrivals = Arrivals::new(keep_ledger);
                subscription.set_timeout(SILENT_CONNECTION);
                let connect = || {
                    let wait = Duration::from_secs(2);
                    let next = subscribe(
                        addr,
                        &again_keys,
                        &again_peers,
                        again_expected.as_deref(),
                        wait,
                    )?;
                    next.set_timeout(SILENT_CONNECTION);
                    Ok(next)
                };
                let began = Instant::now();
                let result = pump_reconnecting(
                    subscription,
                    connect,
                    &mut queue_in,
                    &mut arrivals,
                    &stop_pump,
                    Duration::from_millis(250),
                    |event| {
                        let at = began.elapsed().as_secs_f64();
                        match event {
                            Rejoining::Cut => {
                                cuts_seen.fetch_add(1, Ordering::Relaxed);
                                eprintln!("{at:>7.1} s since the start: the connection was cut; connecting again")
                            }
                            Rejoining::Rejoined => {
                                eprintln!("{at:>7.1} s since the start: connected again");
                                // Making a connection takes longer than this
                                // thread said its work would; say it anew.
                                real_time();
                            }
                        }
                    },
                );
                (result, arrivals)
            })
        } else {
            // The reference: the same test tone, from this program's own
            // clock, on a thread scheduled as the receiving thread is.
            let local = info.clone();
            let mut tone = Tone::new(options.tone_hz.max(0.0), 0.001, stream_rate);
            std::thread::spawn(move || {
                if !istesso_audio::mac::make_this_thread_real_time(
                    packet_time,
                    Duration::from_micros(1500),
                    Duration::from_millis(3),
                ) {
                    eprintln!("the reference's source could not be given real-time scheduling");
                }
                let mut arrivals = Arrivals::new(keep_ledger);
                let ended = pump_local(
                    &local,
                    |out| tone.fill(out, usize::from(local.channels)),
                    &mut queue_in,
                    &mut arrivals,
                    &stop_pump,
                );
                (Ok::<Ended, istesso_audio::Error>(ended), arrivals)
            })
        };

        let mut output = Output::open(options.output.as_deref()).map_err(|e| e.to_string())?;
        let mut device = output.device().clone();
        let cfg = PlayoutConfig::new(
            channels,
            stream_rate,
            device.sample_rate,
            options.latency_ms * 1e-3,
        );
        let resampler = Sinc::new(stream_rate, device.sample_rate, channels, 64, 1.01)?;
        let stats = Arc::new(PlayoutStats::default());
        let (waits_in, mut waits_out) = RingBuffer::<WaitEvent>::new(1 << 16);
        let playout = Playout::new(cfg, queue_out, resampler, stats.clone(), Some(waits_in));

        // What would have been played goes to a thread that keeps it. Room
        // for ten seconds: that thread is an ordinary one and may be held up.
        let (record_in, mut record_out) =
            RingBuffer::<f32>::new((device.sample_rate as usize) * channels * 10);
        let callbacks = Arc::new(Callbacks {
            frames_min: AtomicU64::new(u64::MAX),
            lead_min_us: AtomicI64::new(i64::MAX),
            lead_max_us: AtomicI64::new(i64::MIN),
            ..Callbacks::default()
        });
        // A thread that only sleeps 5 ms at a time and notes how much later
        // than asked it was woken. When packets arrive late, this says
        // whether the whole process was being held up at that moment.
        let held_up_max_us = Arc::new(AtomicU64::new(0));
        let held_up_times = Arc::new(AtomicU64::new(0));
        {
            let (max_us, times, stop) =
                (held_up_max_us.clone(), held_up_times.clone(), stop.clone());
            let started = Instant::now();
            std::thread::spawn(move || {
                let step = Duration::from_millis(5);
                while !stop.load(Ordering::Relaxed) {
                    let due = Instant::now() + step;
                    std::thread::sleep(step);
                    let late = Instant::now().saturating_duration_since(due);
                    max_us.fetch_max(late.as_micros() as u64, Ordering::Relaxed);
                    if late >= Duration::from_millis(10) {
                        times.fetch_add(1, Ordering::Relaxed);
                        eprintln!(
                            "{:>7.1} s since the start: this process was held up for {:.1} ms",
                            started.elapsed().as_secs_f64(),
                            late.as_secs_f64() * 1e3
                        );
                    }
                }
            });
        }

        // The device's own word on turns that were not made in time, and
        // the system's on its devices being rearranged.
        let overload_counts = std::cell::RefCell::new(
            output
                .count_overloads()
                .ok()
                .into_iter()
                .collect::<Vec<_>>(),
        );
        let overloads_now = || {
            overload_counts
                .borrow()
                .iter()
                .map(|count| count.load(Ordering::Relaxed))
                .sum::<u64>()
        };
        let audible = options.audible;
        let rearranged = istesso_audio::mac::watch_devices().ok();
        let mut rearranged_seen = (0u64, 0u64);
        // The playout is shared with the device's callback, so that it can
        // be handed to another device when the default output changes.
        let voice = Arc::new(Mutex::new(Some(Voice {
            playout,
            record_in,
            risen_s: 0.0,
        })));
        let outlet_counts = Arc::new(OutletCounts::default());
        let mut closing = start_output(
            &mut output,
            channels,
            audible,
            voice.clone(),
            callbacks.clone(),
            outlet_counts.clone(),
        )?;
        eprintln!(
            "Playing {} an output at {} Hz ({} frames at a time); the stream is {} Hz, {} channels.",
            if audible { "into" } else { "silently into" },
            device.sample_rate, device.buffer_frames, info.sample_rate, info.channels
        );

        // This thread keeps what would have been played and the waits, and
        // says once a second how things stand.
        let mut record_file = match &options.record {
            Some(path) => Some(std::io::BufWriter::new(
                std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?,
            )),
            None => None,
        };
        // The first channel is looked at for the tone as it goes by, a
        // minute at a time, so that a run may last for hours.
        let mut watch = (options.tone_hz > 0.0)
            .then(|| probe::ToneWatch::new(device.sample_rate, options.tone_hz, 60.0));
        let mut played_frames = 0u64;
        let mut frame = vec![0.0f32; channels];
        // The two machines' clocks drift apart by milliseconds in minutes,
        // so their difference is taken again once a minute while playing.
        let first_offset = offset;
        let mut offset_taken = Instant::now();
        let mut offset_checks = 0u32;
        let mut offset_failures = 0u32;
        let mut offset_round_trip_max_ns = offset.round_trip_ns;
        let mut waits_ms: Vec<f64> = Vec::new();
        let mut latency_ms: Vec<f64> = Vec::new();
        let mut on_the_way_ms: Vec<f64> = Vec::new();
        let begin = Instant::now();
        let mut said = Instant::now();
        // The record this path keeps of itself: a line an hour and a day.
        let mut record = record::Record::open(
            if options.reference {
                "reference"
            } else {
                "exit"
            },
            &[("audible", audible), ("reference", options.reference)],
        );
        let mut gap_began_ms = 0.0f64;
        let mut underruns_before = 0u64;
        let mut underrun_ms_before = 0.0f64;
        let totals = |now: &istesso_audio::playout::Snapshot| {
            [
                ("clicks", now.source_slips as f64),
                ("skips", now.skips as f64),
                ("breaks", now.underruns as f64),
                ("break_ms", now.underrun_ms),
                ("reconnections", cuts.load(Ordering::Relaxed) as f64),
                (
                    "devices_rearranged",
                    rearranged
                        .as_ref()
                        .map_or(0, |c| c.devices.load(Ordering::Relaxed))
                        as f64,
                ),
                (
                    "default_output_changed",
                    rearranged
                        .as_ref()
                        .map_or(0, |c| c.default_output.load(Ordering::Relaxed))
                        as f64,
                ),
                ("device_overloads", overloads_now() as f64),
                (
                    "missed_device_turns",
                    callbacks.missed_turns.load(Ordering::Relaxed) as f64,
                ),
                // Values that were not numbers (as received, and at the
                // outlet), and samples held at full scale.
                (
                    "not_numbers",
                    (wire::not_numbers_received()
                        + outlet_counts.not_numbers.load(Ordering::Relaxed))
                        as f64,
                ),
                (
                    "limited",
                    outlet_counts.limited.load(Ordering::Relaxed) as f64,
                ),
                (
                    "closed_without_fade",
                    outlet_counts.closed_without_fade.load(Ordering::Relaxed) as f64,
                ),
            ]
        };
        loop {
            while record_out.slots() >= channels {
                if record_out.pop_entire_slice(&mut frame).is_err() {
                    break;
                }
                played_frames += 1;
                if let Some(piece) = watch.as_mut().and_then(|watch| watch.push(frame[0])) {
                    eprintln!(
                        "{:>5.0} s  tone  level {} dBFS  pitch {:+.1} to {:+.1} ppm  clicks {}",
                        begin.elapsed().as_secs_f64(),
                        piece
                            .level_dbfs
                            .map_or("none".to_string(), |level| format!("{level:.2}")),
                        piece.pitch_ppm_min,
                        piece.pitch_ppm_max,
                        piece.clicks
                    );
                }
                if let Some(file) = &mut record_file {
                    for sample in &frame {
                        file.write_all(&sample.to_le_bytes())
                            .map_err(|e| e.to_string())?;
                    }
                }
            }
            while let Ok(event) = waits_out.pop() {
                let wait = event.wait_ns as f64 * 1e-6;
                // From the capture on the source to the hand-over here.
                let captured = event.capture_ns as i64 + offset.source_to_local_ns;
                let on_the_way = (event.arrival_ns as i64 - captured) as f64 * 1e-6;
                if waits_ms.len() < 20_000_000 {
                    waits_ms.push(wait);
                    latency_ms.push(wait + on_the_way);
                    on_the_way_ms.push(on_the_way);
                }
            }
            if let Some((addr, _, keys, peers, expected)) = net
                .as_ref()
                .filter(|_| offset_taken.elapsed() >= Duration::from_secs(60))
            {
                offset_taken = Instant::now();
                match clock_offset(*addr, keys, peers, expected.as_deref(), 8, wait) {
                    Ok(taken) => {
                        offset = taken;
                        offset_checks += 1;
                        offset_round_trip_max_ns =
                            offset_round_trip_max_ns.max(taken.round_trip_ns);
                    }
                    Err(_) => offset_failures += 1,
                }
            }
            if said.elapsed() >= Duration::from_secs(1) {
                said = Instant::now();
                let now = stats.snapshot();
                eprintln!(
                    "{:>5.0} s  {:?}  wait {:>6.2} ms  correction {:>+7.1} ppm  underruns {}  skips {}",
                    begin.elapsed().as_secs_f64(),
                    now.state,
                    now.wait_ms,
                    now.correction_ppm,
                    now.underruns,
                    now.skips
                );
                // The longest single break, and the delay as it is now.
                if now.underruns != underruns_before {
                    // A break began since the last look: it is counted
                    // from what the breaks added up to then.
                    underruns_before = now.underruns;
                    gap_began_ms = underrun_ms_before;
                }
                underrun_ms_before = now.underrun_ms;
                record.note_gap(now.underrun_ms - gap_began_ms);
                if let Some(delay) = latency_ms.last() {
                    record.note_delay(*delay);
                }
                record.tick(&totals(&now));
                if let Some(changes) = &rearranged {
                    let now = (
                        changes.default_output.load(Ordering::Relaxed),
                        changes.devices.load(Ordering::Relaxed),
                    );
                    if now.0 != rearranged_seen.0 {
                        let at = begin.elapsed().as_secs_f64();
                        eprintln!(
                            "{at:>7.1} s since the start: the default output changed ({} so far)",
                            now.0
                        );
                        // A player that was not told which device to use
                        // goes with the default one.
                        let next = match &options.output {
                            None => Output::open(None).map_err(|e| e.to_string()),
                            Some(_) => Err("it was told which output to use".to_string()),
                        };
                        match next {
                            Ok(next) if next.device().id == device.id => {}
                            Ok(mut next) => {
                                let to = next.device().clone();
                                let resampler =
                                    Sinc::new(stream_rate, to.sample_rate, channels, 64, 1.01)?;
                                // The old device is left with a fade, not a cut.
                                close_outlet(&closing, &outlet_counts);
                                output.stop();
                                let old = voice
                                    .lock()
                                    .map_err(|_| "the playout was lost".to_string())?
                                    .take()
                                    .ok_or("the playout was lost")?;
                                let (reader, counts, waits) = old.playout.into_parts();
                                let cfg = PlayoutConfig::new(
                                    channels,
                                    stream_rate,
                                    to.sample_rate,
                                    options.latency_ms * 1e-3,
                                );
                                *voice.lock().map_err(|_| "the playout was lost".to_string())? =
                                    Some(Voice {
                                        playout: Playout::new(cfg, reader, resampler, counts, waits),
                                        record_in: old.record_in,
                                        risen_s: old.risen_s,
                                    });
                                closing = start_output(
                                    &mut next,
                                    channels,
                                    audible,
                                    voice.clone(),
                                    callbacks.clone(),
                                    outlet_counts.clone(),
                                )?;
                                if let Ok(count) = next.count_overloads() {
                                    overload_counts.borrow_mut().push(count);
                                }
                                if to.sample_rate != device.sample_rate {
                                    // What would have been played is looked
                                    // at anew at the new rate.
                                    watch = (options.tone_hz > 0.0).then(|| {
                                        probe::ToneWatch::new(to.sample_rate, options.tone_hz, 60.0)
                                    });
                                }
                                output = next;
                                device = to;
                                eprintln!(
                                    "{:>7.1} s since the start: playing into the new default output at {} Hz ({} frames at a time)",
                                    begin.elapsed().as_secs_f64(),
                                    device.sample_rate,
                                    device.buffer_frames
                                );
                            }
                            Err(why) => eprintln!(
                                "{at:>7.1} s since the start: staying on the output opened before: {why}"
                            ),
                        }
                    }
                    if now.1 != rearranged_seen.1 {
                        eprintln!(
                            "{:>7.1} s since the start: the audio devices were rearranged ({} so far)",
                            begin.elapsed().as_secs_f64(),
                            now.1
                        );
                    }
                    rearranged_seen = now;
                }
                if options.stop_file.as_ref().is_some_and(|file| file.exists()) {
                    break;
                }
            }
            if pumping.is_finished() {
                break;
            }
            if options
                .seconds
                .is_some_and(|limit| begin.elapsed().as_secs_f64() >= limit)
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let ran_s = begin.elapsed().as_secs_f64();
        record.end(&totals(&stats.snapshot()));

        // Hang up, and let the device finish.
        stop.store(true, Ordering::Relaxed);
        if let Some(connection) = &connection {
            let _ = connection.shutdown(std::net::Shutdown::Both);
        }
        let (ended, arrivals) = pumping
            .join()
            .map_err(|_| "the receiving thread stopped unexpectedly".to_string())?;
        close_outlet(&closing, &outlet_counts);
        output.stop();
        if let Some(file) = &mut record_file {
            file.flush().map_err(|e| e.to_string())?;
        }
        if let Some(path) = &options.ledger {
            let file =
                std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
            ledger::write(arrivals.ledger(), std::io::BufWriter::new(file))
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }

        let tone = watch.map(probe::ToneWatch::finish);
        // The first ten minutes against the last ten: lateness that piled
        // up over the run would show as a difference. Taken before the
        // lists are sorted for their spread.
        let ends_of = |values: &[f64]| {
            let ten_minutes =
                (600.0 * stream_rate / f64::from(info.packet_frames.max(64))) as usize;
            let part = ten_minutes.min(values.len() / 2);
            let median = |part: &[f64]| {
                let mut part = part.to_vec();
                Spread::of(&mut part).map(|spread| spread.median)
            };
            serde_json::json!({
                "packets_each": part,
                "first_median": median(&values[..part]),
                "last_median": median(&values[values.len() - part..]),
            })
        };
        let latency_ends = ends_of(&latency_ms);
        let wait_ends = ends_of(&waits_ms);
        let count = callbacks.count.load(Ordering::Relaxed).max(1);
        let device_latency_ms = f64::from(device.latency_frames + device.safety_offset_frames)
            / device.sample_rate
            * 1e3;
        let report = serde_json::json!({
            "seconds": ran_s,
            "ended": match &ended {
                Ok(how) => format!("{how:?}"),
                Err(err) => err.to_string(),
            },
            "stream": {
                "sample_rate": info.sample_rate,
                "channels": info.channels,
                "packet_frames": info.packet_frames,
            },
            "output": {
                "sample_rate": device.sample_rate,
                "buffer_frames": device.buffer_frames,
                "latency_frames": device.latency_frames,
                "safety_offset_frames": device.safety_offset_frames,
                "stated_latency_ms": device_latency_ms,
            },
            "target_wait_ms": options.latency_ms,
            // The difference of the two clocks: as first taken, as last
            // taken, and how far it moved in between.
            "clock": first_offset,
            "clock_at_the_end": offset,
            "clock_checks": {
                "taken_again": offset_checks,
                "failed": offset_failures,
                "moved_ms": (offset.source_to_local_ns - first_offset.source_to_local_ns) as f64 * 1e-6,
                "round_trip_max_ms": offset_round_trip_max_ns as f64 * 1e-6,
            },
            "capture_to_output_first_and_last_ten_minutes_ms": latency_ends,
            "wait_first_and_last_ten_minutes_ms": wait_ends,
            "arrivals": arrivals.stats(),
            "playout": stats.snapshot(),
            "callbacks": {
                "count": count,
                "frames_min": callbacks.frames_min.load(Ordering::Relaxed),
                "frames_max": callbacks.frames_max.load(Ordering::Relaxed),
                "lead_min_ms": callbacks.lead_min_us.load(Ordering::Relaxed) as f64 * 1e-3,
                "lead_mean_ms": callbacks.lead_sum_us.load(Ordering::Relaxed) as f64 * 1e-3
                    / count as f64,
                "lead_max_ms": callbacks.lead_max_us.load(Ordering::Relaxed) as f64 * 1e-3,
                // Callbacks that began after their frames were due.
                "began_late": callbacks.late.load(Ordering::Relaxed),
                // This program's own work in a callback: the longest, and
                // how often it took more than half the time of the frames.
                "took_max_ms": callbacks.took_max_us.load(Ordering::Relaxed) as f64 * 1e-3,
                "took_more_than_half_a_turn": callbacks.took_long.load(Ordering::Relaxed),
                // Turns the device did not come for, by the times its
                // frames were due: how many, on how many occasions, and the
                // longest step from one callback's frames to the next's.
                "missed_turns": callbacks.missed_turns.load(Ordering::Relaxed),
                "missed_occasions": callbacks.missed_occasions.load(Ordering::Relaxed),
                "step_max_ms": callbacks.step_max_us.load(Ordering::Relaxed) as f64 * 1e-3,
                // What the device itself said: turns not made in time, by
                // any program playing into it. `null` if it does not say.
                "device_overloads": overloads_now(),
            },
            // How much later than asked a thread of this process that only
            // sleeps was woken: the longest, and how often by 10 ms or more.
            // Whether the source was inside this program (the reference).
            "reference": options.reference,
            // How often the system said that its audio devices changed.
            "devices": {
                "default_output_changed": rearranged.as_ref().map(|c| c.default_output.load(Ordering::Relaxed)),
                "rearranged": rearranged.as_ref().map(|c| c.devices.load(Ordering::Relaxed)),
            },
            // What the last step before the device had to do.
            "outlet": {
                "not_numbers_received": wire::not_numbers_received(),
                "not_numbers_at_the_outlet": outlet_counts.not_numbers.load(Ordering::Relaxed),
                "limited": outlet_counts.limited.load(Ordering::Relaxed),
                "closed_without_fade": outlet_counts.closed_without_fade.load(Ordering::Relaxed),
            },
            "held_up": {
                "longest_ms": held_up_max_us.load(Ordering::Relaxed) as f64 * 1e-3,
                "times_10_ms_or_more": held_up_times.load(Ordering::Relaxed),
            },
            // From the capture of a packet's first frame on the source to
            // the packet's arrival here. Its highest value less its lowest
            // is how late the latest packet was.
            "capture_to_arrival_ms": Spread::of(&mut on_the_way_ms),
            // From a packet's arrival to its first frame being handed over.
            "wait_ms": Spread::of(&mut waits_ms),
            // From the capture of that frame on the source to the same
            // moment; the output's own latency comes on top.
            "capture_to_output_ms": Spread::of(&mut latency_ms),
            "would_have_played": {
                "frames": played_frames,
                "tone": tone,
            },
        });
        match &options.report {
            Some(path) => {
                let text = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
                std::fs::write(path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))
            }
            None => super::super::print_json(&report),
        }
    }
}

// ---------------------------------------------------------------------------
// check / compare
// ---------------------------------------------------------------------------

fn check(file: &std::path::Path, rate: f64, channels: usize, tone_hz: f64) -> Result<(), String> {
    if channels == 0 {
        return Err("a recording has at least one channel".to_string());
    }
    let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let first_channel: Vec<f32> = bytes
        .chunks_exact(4 * channels)
        .map(|frame| f32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]))
        .collect();
    let report = probe::analyze_tone(&first_channel, rate, tone_hz)
        .ok_or("the recording is too short to say anything about it")?;
    super::print_json(&report)
}

fn compare(sent: &std::path::Path, received: &std::path::Path) -> Result<(), String> {
    let read = |path: &std::path::Path| {
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        ledger::read(std::io::BufReader::new(file)).map_err(|e| format!("{}: {e}", path.display()))
    };
    let comparison = ledger::compare(&read(sent)?, &read(received)?);
    super::print_json(&comparison)?;
    if comparison.intact() {
        Ok(())
    } else {
        Err("what was received is not what was sent".to_string())
    }
}

/// How long the sound takes to come up when the player is made audible,
/// and from how far down.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const AUDIBLE_RISE_S: f64 = 20.0;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const AUDIBLE_RISE_FROM_DB: f64 = -40.0;

/// The gain of what is heard, `seconds` of sound after the player began to
/// be heard: it starts 40 dB down and comes up evenly, in decibels, over
/// 20 seconds, and is 1 from then on. It is never more than 1.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn audible_gain(seconds: f64) -> f32 {
    if seconds >= AUDIBLE_RISE_S {
        return 1.0;
    }
    let still_down_db = AUDIBLE_RISE_FROM_DB * (1.0 - seconds.max(0.0) / AUDIBLE_RISE_S);
    (10f64.powf(still_down_db / 20.0) as f32).min(1.0)
}

/// The record the audio path keeps of itself: one line for every hour and
/// one for every day it ran, in a file of this machine. A line holds
/// counts and times only: no name, no address, nothing about what played.
mod record {
    use std::io::Write;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Where the lines go: `ISTESSO_AUDIO_RECORD_DIR`, or
    /// `~/.local/state/istesso/audio`. The value `off` keeps no record.
    fn directory() -> Option<PathBuf> {
        match std::env::var("ISTESSO_AUDIO_RECORD_DIR") {
            Ok(dir) if dir == "off" => None,
            Ok(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
            _ => {
                let state = std::env::var("XDG_STATE_HOME")
                    .ok()
                    .filter(|dir| !dir.is_empty())
                    .map(PathBuf::from)
                    .or_else(|| {
                        std::env::var("HOME")
                            .ok()
                            .map(|home| PathBuf::from(home).join(".local/state"))
                    })?;
                Some(state.join("istesso/audio"))
            }
        }
    }

    /// This machine's offset from UTC in seconds, from the system's `date`
    /// (`+0900`). Zero when that cannot be had.
    fn local_offset_s() -> i64 {
        std::process::Command::new("date")
            .arg("+%z")
            .output()
            .ok()
            .and_then(|out| parse_offset(String::from_utf8_lossy(&out.stdout).trim()))
            .unwrap_or(0)
    }

    pub(super) fn parse_offset(text: &str) -> Option<i64> {
        let (sign, digits) = match text.as_bytes().first()? {
            b'+' => (1, &text[1..]),
            b'-' => (-1, &text[1..]),
            _ => return None,
        };
        if digits.len() != 4 {
            return None;
        }
        let hours: i64 = digits[..2].parse().ok()?;
        let minutes: i64 = digits[2..].parse().ok()?;
        Some(sign * (hours * 3600 + minutes * 60))
    }

    /// `2026-11-17T15:00:00+09:00` for a moment and an offset from UTC.
    pub(super) fn stamp(epoch_s: u64, offset_s: i64) -> String {
        let local = epoch_s as i64 + offset_s;
        let (days, second_of_day) = (local.div_euclid(86_400), local.rem_euclid(86_400));
        // Days since 1970-01-01 to a date of the calendar in use.
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let day_of_era = z.rem_euclid(146_097);
        let year_of_era =
            (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month_index = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * month_index + 2) / 5 + 1;
        let month = if month_index < 10 {
            month_index + 3
        } else {
            month_index - 9
        };
        let year = year_of_era + era * 400 + i64::from(month <= 2);
        format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}{}{:02}:{:02}",
            second_of_day / 3600,
            second_of_day % 3600 / 60,
            second_of_day % 60,
            if offset_s < 0 { '-' } else { '+' },
            offset_s.abs() / 3600,
            offset_s.abs() % 3600 / 60
        )
    }

    /// One hour or one day that is being counted.
    struct Period {
        kind: &'static str,
        length_s: i64,
        /// Which hour or day, counted in local time.
        key: i64,
        from_s: u64,
        base: Vec<f64>,
        longest_gap_ms: f64,
        delays_ms: Vec<f64>,
    }

    pub(super) struct Record {
        file: Option<PathBuf>,
        role: &'static str,
        flags: Vec<(&'static str, bool)>,
        offset_s: i64,
        periods: Vec<Period>,
    }

    fn now_s() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs())
    }

    impl Record {
        /// The record of the program in the role `role` ("exit", "source",
        /// "reference"); `flags` are written into every line as they are.
        pub(super) fn open(role: &'static str, flags: &[(&'static str, bool)]) -> Self {
            let file = directory().map(|dir| dir.join(format!("{role}.jsonl")));
            Record::at(file, role, flags, local_offset_s(), now_s())
        }

        pub(super) fn at(
            file: Option<PathBuf>,
            role: &'static str,
            flags: &[(&'static str, bool)],
            offset_s: i64,
            now_s: u64,
        ) -> Self {
            let period = |kind, length_s: i64| Period {
                kind,
                length_s,
                key: (now_s as i64 + offset_s).div_euclid(length_s),
                from_s: now_s,
                base: Vec::new(),
                longest_gap_ms: 0.0,
                delays_ms: Vec::new(),
            };
            Record {
                file,
                role,
                flags: flags.to_vec(),
                offset_s,
                periods: vec![period("hour", 3_600), period("day", 86_400)],
            }
        }

        /// A break of this length is going on or just ended.
        #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
        pub(super) fn note_gap(&mut self, ms: f64) {
            for period in &mut self.periods {
                period.longest_gap_ms = period.longest_gap_ms.max(ms);
            }
        }

        /// The delay from the capture to the hand-over, as it is now.
        #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
        pub(super) fn note_delay(&mut self, ms: f64) {
            for period in &mut self.periods {
                if period.delays_ms.len() < 100_000 {
                    period.delays_ms.push(ms);
                }
            }
        }

        /// Call about once a second with the counts since the program
        /// started. Writes a line when an hour or a day has ended.
        pub(super) fn tick(&mut self, totals: &[(&'static str, f64)]) {
            self.tick_at(now_s(), totals, false);
        }

        /// The program ends: the hour and the day it is in are written as
        /// far as they got.
        pub(super) fn end(&mut self, totals: &[(&'static str, f64)]) {
            self.tick_at(now_s(), totals, true);
        }

        pub(super) fn tick_at(&mut self, now_s: u64, totals: &[(&'static str, f64)], ending: bool) {
            let mut lines = Vec::new();
            for period in &mut self.periods {
                if period.base.is_empty() {
                    period.base = vec![0.0; totals.len()];
                }
                let key = (now_s as i64 + self.offset_s).div_euclid(period.length_s);
                if key == period.key && !ending {
                    continue;
                }
                let mut line = serde_json::Map::new();
                line.insert("kind".into(), period.kind.into());
                line.insert("role".into(), self.role.into());
                line.insert("from".into(), stamp(period.from_s, self.offset_s).into());
                line.insert("to".into(), stamp(now_s, self.offset_s).into());
                line.insert("seconds".into(), now_s.saturating_sub(period.from_s).into());
                for ((name, total), base) in totals.iter().zip(&period.base) {
                    let counted = ((total - base) * 10.0).round() / 10.0;
                    line.insert((*name).into(), counted.into());
                }
                line.insert(
                    "longest_gap_ms".into(),
                    ((period.longest_gap_ms * 10.0).round() / 10.0).into(),
                );
                period.delays_ms.sort_by(f64::total_cmp);
                let delay = period.delays_ms.get(period.delays_ms.len() / 2);
                line.insert(
                    "delay_ms".into(),
                    delay.map_or(serde_json::Value::Null, |ms| {
                        ((ms * 10.0).round() / 10.0).into()
                    }),
                );
                for (name, value) in &self.flags {
                    line.insert((*name).into(), (*value).into());
                }
                lines.push(serde_json::Value::Object(line).to_string());
                period.key = key;
                period.from_s = now_s;
                period.base = totals.iter().map(|(_, total)| *total).collect();
                period.longest_gap_ms = 0.0;
                period.delays_ms.clear();
            }
            let Some(file) = &self.file else { return };
            if lines.is_empty() {
                return;
            }
            // A record that cannot be written must not stop the sound.
            let written = file
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| {
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(file)
                })
                .and_then(|mut out| lines.iter().try_for_each(|line| writeln!(out, "{line}")));
            if let Err(err) = written {
                eprintln!("the audio record could not be written: {err}");
            }
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn what_is_heard_first_is_quiet_and_comes_up_without_a_step() {
        assert!(
            (audible_gain(0.0) - 0.01).abs() < 1e-6,
            "40 dB down at the start"
        );
        assert!(
            (audible_gain(10.0) - 0.1).abs() < 1e-6,
            "20 dB down half way"
        );
        assert_eq!(audible_gain(20.0), 1.0);
        assert_eq!(audible_gain(3_600.0), 1.0);
        // From one frame to the next the gain rises by less than a
        // thousandth of itself, never falls, and never passes 1.
        let mut before = audible_gain(0.0);
        for frame in 1..=(21 * 48_000) {
            let gain = audible_gain(f64::from(frame) / 48_000.0);
            assert!(gain >= before && gain <= 1.0);
            assert!(gain - before <= before * 1e-3, "a step at frame {frame}");
            before = gain;
        }
    }

    #[test]
    fn the_record_stamps_times_in_local_time() {
        assert_eq!(record::parse_offset("+0900"), Some(32_400));
        assert_eq!(record::parse_offset("-0330"), Some(-12_600));
        assert_eq!(record::parse_offset("JST"), None);
        assert_eq!(record::stamp(0, 0), "1970-01-01T00:00:00+00:00");
        // 2026-11-17 06:00:00 UTC is 15:00 in a place nine hours ahead.
        assert_eq!(
            record::stamp(1_794_895_200, 32_400),
            "2026-11-17T15:00:00+09:00"
        );
        assert_eq!(record::stamp(951_782_400, 0), "2000-02-29T00:00:00+00:00");
    }

    #[test]
    fn the_record_writes_a_line_when_an_hour_ends_and_counts_from_there() {
        let dir = std::env::temp_dir().join(format!("istesso-record-test-{}", std::process::id()));
        let file = dir.join("exit.jsonl");
        let _ = std::fs::remove_file(&file);
        let start = 1_794_895_200 + 1_800; // half past the hour
        let mut record = record::Record::at(
            Some(file.clone()),
            "exit",
            &[("audible", false)],
            32_400,
            start,
        );
        record.tick_at(start + 1, &[("clicks", 0.0), ("skips", 0.0)], false);
        record.note_gap(40.0);
        record.note_delay(61.0);
        record.note_delay(63.0);
        record.note_delay(62.0);
        record.tick_at(start + 1_799, &[("clicks", 2.0), ("skips", 1.0)], false);
        assert!(!file.exists(), "nothing is written before the hour ends");
        record.tick_at(start + 1_800, &[("clicks", 2.0), ("skips", 1.0)], false);
        record.tick_at(start + 2_000, &[("clicks", 5.0), ("skips", 1.0)], true);
        let text = std::fs::read_to_string(&file).unwrap();
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_dir(&dir);
        assert_eq!(lines.len(), 3, "{text}");
        assert_eq!(lines[0]["kind"], "hour");
        assert_eq!(lines[0]["from"], "2026-11-17T15:30:00+09:00");
        assert_eq!(lines[0]["to"], "2026-11-17T16:00:00+09:00");
        assert_eq!(lines[0]["seconds"], 1_800);
        assert_eq!(lines[0]["clicks"], 2.0);
        assert_eq!(lines[0]["longest_gap_ms"], 40.0);
        assert_eq!(lines[0]["delay_ms"], 62.0);
        assert_eq!(lines[0]["audible"], false);
        // The end writes the hour and the day as far as they got.
        assert_eq!(lines[1]["kind"], "hour");
        assert_eq!(lines[1]["clicks"], 3.0);
        assert_eq!(lines[1]["longest_gap_ms"], 0.0);
        assert_eq!(lines[2]["kind"], "day");
        assert_eq!(lines[2]["clicks"], 5.0);
        assert_eq!(lines[2]["seconds"], 2_000);
    }
    use super::*;

    #[test]
    fn stalls_are_written_as_start_and_length() {
        assert_eq!(parse_stall("600:1"), Ok((600.0, 1.0)));
        assert_eq!(parse_stall(" 12.5 : 0.25 "), Ok((12.5, 0.25)));
        assert!(parse_stall("600").is_err());
        assert!(parse_stall("600:0").is_err());
        assert!(parse_stall("-1:1").is_err());
        assert!(parse_stall("a:b").is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_turn_the_device_did_not_come_for_is_told_from_jitter() {
        let seen = mac::callbacks_for_test();
        let turn = 10_666_667u64;
        // On time, a little late, a little early: no turn missed.
        mac::note_turn_for_test(&seen, 100_000_000, None);
        mac::note_turn_for_test(&seen, 100_000_000 + turn, Some((100_000_000, turn)));
        mac::note_turn_for_test(&seen, 3_000_000 + 2 * turn, Some((turn, turn)));
        assert_eq!(mac::missed_for_test(&seen), (0, 0));
        // The frames are due two turns later than the last ones end: the
        // device did not come twice, on one occasion.
        mac::note_turn_for_test(&seen, 4 * turn, Some((turn, turn)));
        assert_eq!(mac::missed_for_test(&seen), (2, 1));
        mac::note_turn_for_test(&seen, 6 * turn, Some((4 * turn, turn)));
        assert_eq!(mac::missed_for_test(&seen), (3, 2));
    }

    #[test]
    fn properties_are_written_as_key_and_value() {
        assert_eq!(
            parse_property("device.routes=1"),
            Ok(("device.routes".to_string(), "1".to_string()))
        );
        assert_eq!(
            parse_property(" node.group = a=b "),
            Ok(("node.group".to_string(), "a=b".to_string()))
        );
        assert!(parse_property("device.routes").is_err());
        assert!(parse_property("=1").is_err());
    }

    #[test]
    fn addresses_get_the_audio_port_unless_they_name_one() {
        assert_eq!(
            resolve("127.0.0.1").unwrap(),
            "127.0.0.1:47802".parse().unwrap()
        );
        assert_eq!(
            resolve("127.0.0.1:5000").unwrap(),
            "127.0.0.1:5000".parse().unwrap()
        );
    }
}
