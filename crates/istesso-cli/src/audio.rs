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
        address: String,
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
            address,
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
        }
        if options.stop_file.as_ref().is_some_and(|file| file.exists()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
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
    use istesso_audio::playout::{Playout, PlayoutConfig, PlayoutStats, WaitEvent};
    use istesso_audio::probe::Spread;
    use istesso_audio::resample::Sinc;
    use istesso_audio::rtrb::RingBuffer;
    use istesso_audio::session::{
        clock_offset, pump_reconnecting, subscribe, Arrivals, Rejoining, SILENT_CONNECTION,
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

    pub fn play(options: PlayOptions) -> Result<(), String> {
        if !(5.0..=2000.0).contains(&options.latency_ms) {
            return Err("the wait is between 5 and 2000 milliseconds".to_string());
        }
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
        let mut offset = loop {
            match clock_offset(addr, &keys, &peers, expected.as_deref(), 16, wait) {
                Ok(offset) => break offset,
                Err(istesso_audio::Error::Io(err)) if Instant::now() < give_up => {
                    eprintln!("cannot reach the source yet ({err}); trying again");
                    std::thread::sleep(Duration::from_secs(1));
                }
                Err(err) => return Err(err.to_string()),
            }
        };
        let subscription =
            subscribe(addr, &keys, &peers, expected.as_deref(), wait).map_err(|e| e.to_string())?;
        let info = subscription.info().clone();
        let channels = usize::from(info.channels);
        let connection = subscription.connection().map_err(|e| e.to_string())?;
        let stream_rate = f64::from(info.sample_rate);

        // The network side: packets into the queue. It runs from the moment
        // the source begins to send. Opening the device can take a second
        // or two; packets left in the connection for that long would make
        // the source's sending wait, and the source would let audio go.
        // Room for four seconds of audio between the network and the device.
        let (mut queue_in, queue_out) = queue(channels, (stream_rate * 4.0) as usize);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_pump = stop.clone();
        let keep_ledger = options.ledger.is_some();
        // One packet's time, for the scheduling of the thread that receives.
        let packet_time =
            Duration::from_secs_f64(f64::from(info.packet_frames.max(64)) / stream_rate);
        // The receiving thread makes a new connection when one is cut. It
        // gets keys and a list of paired devices of its own for that.
        let again_keys = LocalKeys::new(&identity).map_err(|e| e.to_string())?;
        let again_peers = super::super::open_peers()?;
        let again_expected = expected.clone();
        let pumping = std::thread::spawn(move || {
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
        });

        let mut output = Output::open(options.output.as_deref()).map_err(|e| e.to_string())?;
        let device = output.device().clone();
        let cfg = PlayoutConfig::new(
            channels,
            stream_rate,
            device.sample_rate,
            options.latency_ms * 1e-3,
        );
        let resampler = Sinc::new(stream_rate, device.sample_rate, channels, 64, 1.01)?;
        let stats = Arc::new(PlayoutStats::default());
        let (waits_in, mut waits_out) = RingBuffer::<WaitEvent>::new(1 << 16);
        let mut playout = Playout::new(cfg, queue_out, resampler, stats.clone(), Some(waits_in));

        // What would have been played goes to a thread that keeps it. Room
        // for ten seconds: that thread is an ordinary one and may be held up.
        let (mut record_in, mut record_out) =
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

        // The device's own word on turns that were not made in time.
        let overloads = output.count_overloads().ok();
        let seen = callbacks.clone();
        let device_rate = device.sample_rate;
        let mut turn_before: Option<(u64, u64)> = None;
        output
            .start(channels, move |frames, out, now_ns| {
                let began_ns = clock::now_ns();
                playout.render(frames, out, now_ns);
                let _ = record_in.push_partial_slice(out);
                // Nothing is to be heard yet: the device gets silence, and
                // with it the clock of the device still drives the playout.
                out.fill(0.0);
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
        eprintln!(
            "Playing silently into an output at {} Hz ({} frames at a time); the stream is {} Hz, {} channels.",
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
            if offset_taken.elapsed() >= Duration::from_secs(60) {
                offset_taken = Instant::now();
                match clock_offset(addr, &keys, &peers, expected.as_deref(), 8, wait) {
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

        // Hang up, and let the device finish.
        stop.store(true, Ordering::Relaxed);
        let _ = connection.shutdown(std::net::Shutdown::Both);
        let (ended, arrivals) = pumping
            .join()
            .map_err(|_| "the receiving thread stopped unexpectedly".to_string())?;
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
                "device_overloads": overloads.as_ref().map(|count| count.load(Ordering::Relaxed)),
            },
            // How much later than asked a thread of this process that only
            // sleeps was woken: the longest, and how often by 10 ms or more.
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

#[cfg(test)]
mod tests {
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
