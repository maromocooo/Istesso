//! The Linux source: a sink of our own in PipeWire.
//!
//! Applications play into it as they would into a sound card. Once per cycle
//! of its graph PipeWire hands over what they played; this module writes it
//! into the capture ring ([`CaptureWriter`]) and does nothing else on
//! PipeWire's thread: no waiting, no allocating, no network.
//!
//! The sink lives as long as the process does. No configuration file is
//! written and PipeWire is not restarted. The sink is never made the output
//! applications get by default: they reach it only when they are pointed at
//! it.
//!
//! While nothing plays into the sink, PipeWire does not run it, and no audio
//! reaches the ring. The sending side tells the exit so.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use pipewire as pw;
use pw::spa;

use crate::source::CaptureWriter;
use crate::wire::MAX_PACKET_FRAMES;
use crate::{Error, Result};

/// How the sink presents itself.
#[derive(Debug, Clone)]
pub struct SinkOptions {
    /// Name other programs address the sink by.
    pub node_name: String,
    /// Name people see in a list of outputs.
    pub description: String,
    /// Frames per cycle to ask the graph for. `None` asks for nothing, and
    /// the graph keeps the cycle its other clients settled on.
    pub quantum: Option<u32>,
    /// Further properties of the node.
    pub properties: Vec<(String, String)>,
}

/// What the capturing thread saw. Written from PipeWire's thread, read from
/// any.
#[derive(Debug)]
pub struct SinkStats {
    cycles: AtomicU64,
    frames: AtomicU64,
    frames_min: AtomicU32,
    frames_max: AtomicU32,
    without_audio: AtomicU64,
    unfitted: AtomicU64,
    unstamped: AtomicU64,
    age_min_ns: AtomicI64,
    age_max_ns: AtomicI64,
    step_max_ns: AtomicU64,
    resumed: AtomicU64,
    late_cycles: AtomicU64,
    watchdog_wakes: AtomicU64,
    watchdog_late: AtomicU64,
    watchdog_late_max_ns: AtomicU64,
    watchdog_late_last_ns: AtomicU64,
    /// 0: not running; 1: ordinary scheduling; 2: real-time.
    watchdog_class: AtomicU32,
    watchdog_priority: AtomicU32,
    late_total_ns: AtomicU64,
    late_max_ns: AtomicU64,
    late_last_ns: AtomicU64,
}

impl Default for SinkStats {
    fn default() -> Self {
        Self {
            cycles: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            frames_min: AtomicU32::new(u32::MAX),
            frames_max: AtomicU32::new(0),
            without_audio: AtomicU64::new(0),
            unfitted: AtomicU64::new(0),
            unstamped: AtomicU64::new(0),
            age_min_ns: AtomicI64::new(i64::MAX),
            age_max_ns: AtomicI64::new(i64::MIN),
            step_max_ns: AtomicU64::new(0),
            resumed: AtomicU64::new(0),
            late_cycles: AtomicU64::new(0),
            watchdog_wakes: AtomicU64::new(0),
            watchdog_late: AtomicU64::new(0),
            watchdog_late_max_ns: AtomicU64::new(0),
            watchdog_late_last_ns: AtomicU64::new(0),
            watchdog_class: AtomicU32::new(0),
            watchdog_priority: AtomicU32::new(0),
            late_total_ns: AtomicU64::new(0),
            late_max_ns: AtomicU64::new(0),
            late_last_ns: AtomicU64::new(0),
        }
    }
}

/// [`SinkStats`] at one moment.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct SinkSnapshot {
    /// Cycles of the graph in which audio was handed over.
    pub cycles: u64,
    pub frames: u64,
    /// Fewest and most frames handed over in one cycle.
    pub frames_min: u32,
    pub frames_max: u32,
    /// Cycles that came without audio.
    pub cycles_without_audio: u64,
    /// Cycles whose audio did not fit into the ring.
    pub cycles_not_fitted: u64,
    /// Cycles the graph gave no time for; they carry the time this thread
    /// ran instead.
    pub cycles_without_time: u64,
    /// How long after a cycle began this thread was given its audio.
    pub cycle_age_min_ms: Option<f64>,
    pub cycle_age_max_ms: Option<f64>,
    /// The longest time from the beginning of one cycle to the beginning of
    /// the next, while audio flowed. A graph that stalls shows here.
    pub cycle_step_max_ms: f64,
    /// Times audio came again after a second or more without any.
    pub resumed: u64,
    /// Cycles that began a millisecond or more later than the cycle before
    /// them left room for, while audio flowed: how many, by how much in
    /// all, the latest of them, and the last one.
    pub late_cycles: u64,
    pub late_total_ms: f64,
    pub late_max_ms: f64,
    pub late_last_ms: f64,
    /// The watchdog beside the capture: a thread that only wakes every two
    /// milliseconds. How often it woke, how often a millisecond or more
    /// late, its latest and its last lateness, and whether it runs with
    /// real-time scheduling as the capture does (`None`: it is not
    /// running), at which priority. Late at the same moment as the capture:
    /// the whole machine stood still. The capture alone: PipeWire's side.
    pub watchdog_wakes: u64,
    pub watchdog_late: u64,
    pub watchdog_late_max_ms: f64,
    pub watchdog_late_last_ms: f64,
    pub watchdog_real_time: Option<bool>,
    pub watchdog_priority: u32,
}

impl SinkStats {
    pub fn snapshot(&self) -> SinkSnapshot {
        let cycles = self.cycles.load(Ordering::Relaxed);
        let age =
            |value: i64| (value != i64::MAX && value != i64::MIN).then_some(value as f64 * 1e-6);
        SinkSnapshot {
            cycles,
            frames: self.frames.load(Ordering::Relaxed),
            frames_min: if cycles == 0 {
                0
            } else {
                self.frames_min.load(Ordering::Relaxed)
            },
            frames_max: self.frames_max.load(Ordering::Relaxed),
            cycles_without_audio: self.without_audio.load(Ordering::Relaxed),
            cycles_not_fitted: self.unfitted.load(Ordering::Relaxed),
            cycles_without_time: self.unstamped.load(Ordering::Relaxed),
            cycle_age_min_ms: age(self.age_min_ns.load(Ordering::Relaxed)),
            cycle_age_max_ms: age(self.age_max_ns.load(Ordering::Relaxed)),
            cycle_step_max_ms: self.step_max_ns.load(Ordering::Relaxed) as f64 * 1e-6,
            resumed: self.resumed.load(Ordering::Relaxed),
            late_cycles: self.late_cycles.load(Ordering::Relaxed),
            late_total_ms: self.late_total_ns.load(Ordering::Relaxed) as f64 * 1e-6,
            late_max_ms: self.late_max_ns.load(Ordering::Relaxed) as f64 * 1e-6,
            late_last_ms: self.late_last_ns.load(Ordering::Relaxed) as f64 * 1e-6,
            watchdog_wakes: self.watchdog_wakes.load(Ordering::Relaxed),
            watchdog_late: self.watchdog_late.load(Ordering::Relaxed),
            watchdog_late_max_ms: self.watchdog_late_max_ns.load(Ordering::Relaxed) as f64 * 1e-6,
            watchdog_late_last_ms: self.watchdog_late_last_ns.load(Ordering::Relaxed) as f64 * 1e-6,
            watchdog_real_time: match self.watchdog_class.load(Ordering::Relaxed) {
                0 => None,
                class => Some(class == 2),
            },
            watchdog_priority: self.watchdog_priority.load(Ordering::Relaxed),
        }
    }
}

/// The configuration PipeWire ships for clients whose processing thread
/// should run with real-time priority. In PipeWire 1.0 the configuration a
/// client gets by default does not ask for it. Later versions ask by
/// default and may not ship this file; then the default is used.
const REALTIME_CLIENT_CONFIG: &str = "client-rt.conf";

/// An age of a cycle that can be believed: not in the future, and less than
/// a second.
const BELIEVABLE_AGE_NS: std::ops::Range<i64> = 0..1_000_000_000;

/// The part that runs on PipeWire's thread.
struct Capture {
    writer: CaptureWriter,
    channels: usize,
    /// Room for the largest piece the ring takes; filled anew every cycle.
    samples: Vec<f32>,
    /// When the cycle before this one began, and how long its audio lasts.
    last_began_ns: Option<u64>,
    last_lasts_ns: u64,
    stats: Arc<SinkStats>,
}

/// A cycle that begins this much later than the one before left room for
/// counts as late.
const LATE_NS: u64 = 1_000_000;

/// With no audio for this long, the next cycle counts as audio coming again,
/// not as a cycle that was late.
const PAUSE_NS: u64 = 1_000_000_000;

impl Capture {
    fn new(writer: CaptureWriter, stats: Arc<SinkStats>) -> Self {
        let channels = writer.channels();
        Self {
            writer,
            channels,
            samples: Vec::with_capacity(MAX_PACKET_FRAMES * channels),
            last_began_ns: None,
            last_lasts_ns: 0,
            stats,
        }
    }

    /// Takes the audio of one cycle: 32-bit floats, little-endian, channels
    /// interleaved. `now_ns` is this crate's clock now, `cycle_age_ns` how
    /// long ago the cycle began, if the graph says.
    ///
    /// The audio is stamped with the beginning of its cycle. That is when a
    /// sound card in this sink's place would have begun to play it, and it
    /// does not move when this thread is late.
    fn take(&mut self, bytes: &[u8], now_ns: u64, cycle_age_ns: Option<i64>) {
        let frame_bytes = 4 * self.channels;
        let frames = bytes.len() / frame_bytes;
        if frames == 0 {
            self.stats.without_audio.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let stats = &self.stats;
        stats.cycles.fetch_add(1, Ordering::Relaxed);
        stats.frames.fetch_add(frames as u64, Ordering::Relaxed);
        stats.frames_min.fetch_min(frames as u32, Ordering::Relaxed);
        stats.frames_max.fetch_max(frames as u32, Ordering::Relaxed);
        let mut capture_ns = match cycle_age_ns.filter(|age| BELIEVABLE_AGE_NS.contains(age)) {
            Some(age) => {
                stats.age_min_ns.fetch_min(age, Ordering::Relaxed);
                stats.age_max_ns.fetch_max(age, Ordering::Relaxed);
                now_ns.saturating_sub(age as u64)
            }
            None => {
                stats.unstamped.fetch_add(1, Ordering::Relaxed);
                now_ns
            }
        };
        if let Some(before) = self.last_began_ns {
            let step = capture_ns.saturating_sub(before);
            if step >= PAUSE_NS {
                stats.resumed.fetch_add(1, Ordering::Relaxed);
            } else {
                stats.step_max_ns.fetch_max(step, Ordering::Relaxed);
                let late = step.saturating_sub(self.last_lasts_ns);
                if late >= LATE_NS {
                    stats.late_cycles.fetch_add(1, Ordering::Relaxed);
                    stats.late_total_ns.fetch_add(late, Ordering::Relaxed);
                    stats.late_max_ns.fetch_max(late, Ordering::Relaxed);
                    stats.late_last_ns.store(late, Ordering::Relaxed);
                }
            }
        }
        self.last_began_ns = Some(capture_ns);
        self.last_lasts_ns = (frames as f64 / self.writer.rate() * 1e9) as u64;

        let rate = self.writer.rate();
        let mut fitted = true;
        for piece in bytes[..frames * frame_bytes].chunks(MAX_PACKET_FRAMES * frame_bytes) {
            // Within the capacity set aside at the start: no allocation.
            self.samples.clear();
            for raw in piece.chunks_exact(4) {
                let value = f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
                self.samples
                    .push(if value.is_finite() { value } else { 0.0 });
            }
            fitted &= self.writer.push(&self.samples, capture_ns);
            capture_ns += ((piece.len() / frame_bytes) as f64 / rate * 1e9) as u64;
        }
        if !fitted {
            stats.unfitted.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The monotonic clock PipeWire stamps its cycles with.
// The fields of a `timespec` are narrower on some platforms.
#[allow(clippy::unnecessary_cast)]
fn monotonic_ns() -> i64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `now` is a valid place for the one value the call writes.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    now.tv_sec as i64 * 1_000_000_000 + now.tv_nsec as i64
}

/// How long ago the cycle that is being processed began.
fn cycle_age_ns(stream: &pw::stream::Stream) -> Option<i64> {
    let began = stream.time().ok()?.now();
    (began > 0).then(|| monotonic_ns() - began)
}

/// The format the sink takes: what the stream on the wire is made of.
fn format(rate: u32, channels: u32) -> Result<Vec<u8>> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(rate);
    info.set_channels(channels);
    if channels == 2 {
        let mut position = [0; spa::param::audio::MAX_CHANNELS];
        position[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
        position[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
        info.set_position(position);
    }
    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )
    .map(|(cursor, _)| cursor.into_inner())
    .map_err(|err| Error::Device(format!("PipeWire: cannot describe the format: {err:?}")))
}

/// Offers a sink to the applications of this machine and writes what they
/// play into `writer`, until `stop` is set. Runs on the calling thread.
///
/// The name under which a PipeWire context keeps the helper that gives
/// threads real-time scheduling.
const THREAD_UTILS: &[u8] = b"Spa:Pointer:Interface:ThreadUtils\0";

/// How often the watchdog wakes, and from when on a wake-up counts as late.
const WATCHDOG_STEP: Duration = Duration::from_millis(2);
const WATCHDOG_LATE: Duration = Duration::from_millis(1);

/// Asks the helper PipeWire uses for its own processing threads to
/// schedule the calling thread as it schedules those.
///
/// # Safety
/// `utils` is null or the thread helper of a context that is alive.
unsafe fn schedule_as_the_capture(utils: *mut spa::sys::spa_thread_utils) -> bool {
    if utils.is_null() {
        return false;
    }
    // SAFETY: the helper is an interface of the context: a table of
    // functions and the object they are called with.
    unsafe {
        let iface = &(*utils).iface;
        let methods = iface.cb.funcs.cast::<spa::sys::spa_thread_utils_methods>();
        if methods.is_null() {
            return false;
        }
        match (*methods).acquire_rt {
            Some(acquire) => {
                let this_thread = libc::pthread_self() as usize as *mut spa::sys::spa_thread;
                // -1: the priority the configuration gives processing threads.
                acquire(iface.cb.data, this_thread, -1) >= 0
            }
            None => false,
        }
    }
}

/// The watchdog beside the capture: a thread scheduled as the capture's
/// is, that does nothing but wake every [`WATCHDOG_STEP`] and note how late
/// it was woken. `utils` is the context's thread helper, as a number so
/// that it can cross to the new thread.
fn start_watchdog(stats: Arc<SinkStats>, stop: Arc<AtomicBool>, utils: usize) {
    let spawned = std::thread::Builder::new()
        .name("istesso-watchdog".to_string())
        .spawn(move || {
            // SAFETY: the pointer comes from the context of `run_sink`,
            // which outlives this thread's use of it: the request is made
            // once, here, before the sink's loop can end.
            let _ = unsafe { schedule_as_the_capture(utils as *mut spa::sys::spa_thread_utils) };
            // What the thread really has, whatever was answered.
            let mut policy = 0;
            // SAFETY: plain values for the call to fill.
            let mut param: libc::sched_param = unsafe { std::mem::zeroed() };
            // SAFETY: both pointers are valid for the call.
            unsafe { libc::pthread_getschedparam(libc::pthread_self(), &mut policy, &mut param) };
            let policy = policy & !libc::SCHED_RESET_ON_FORK;
            let real_time = policy == libc::SCHED_FIFO || policy == libc::SCHED_RR;
            stats
                .watchdog_class
                .store(if real_time { 2 } else { 1 }, Ordering::Relaxed);
            stats
                .watchdog_priority
                .store(param.sched_priority.max(0) as u32, Ordering::Relaxed);
            while !stop.load(Ordering::Relaxed) {
                let due = std::time::Instant::now() + WATCHDOG_STEP;
                std::thread::sleep(WATCHDOG_STEP);
                let late = std::time::Instant::now().saturating_duration_since(due);
                stats.watchdog_wakes.fetch_add(1, Ordering::Relaxed);
                let late_ns = late.as_nanos() as u64;
                stats
                    .watchdog_late_max_ns
                    .fetch_max(late_ns, Ordering::Relaxed);
                if late >= WATCHDOG_LATE {
                    stats
                        .watchdog_late_last_ns
                        .store(late_ns, Ordering::Relaxed);
                    stats.watchdog_late.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
    if spawned.is_err() {
        // Without the watchdog the capture works as before.
    }
}

/// The sink takes audio in the form `writer` was made for (its rate and
/// channels); PipeWire converts what applications play.
pub fn run_sink(
    options: &SinkOptions,
    writer: CaptureWriter,
    stop: Arc<AtomicBool>,
    stats: Arc<SinkStats>,
    log: Arc<dyn Fn(String) + Send + Sync>,
) -> Result<()> {
    let device = |err: pw::Error| Error::Device(format!("PipeWire: {err}"));
    let rate = writer.rate() as u32;
    let channels = writer.channels() as u32;

    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(device)?;
    let realtime = pw::properties::properties! {
        *pw::keys::CONFIG_NAME => REALTIME_CLIENT_CONFIG,
    };
    let context = pw::context::ContextRc::new(&mainloop, Some(realtime))
        .or_else(|_| pw::context::ContextRc::new(&mainloop, None))
        .map_err(device)?;
    let core = context.connect_rc(None).map_err(device)?;

    // SAFETY: the context is alive; the name is a C string.
    let utils = unsafe {
        pw::sys::pw_context_get_object(context.as_raw_ptr(), THREAD_UTILS.as_ptr().cast())
    } as usize;
    start_watchdog(stats.clone(), stop.clone(), utils);

    let mut props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CLASS => "Audio/Sink",
        *pw::keys::NODE_NAME => options.node_name.as_str(),
        *pw::keys::NODE_DESCRIPTION => options.description.as_str(),
        *pw::keys::NODE_VIRTUAL => "true",
        // Last in line when the session manager looks for a default output.
        *pw::keys::PRIORITY_SESSION => "0",
    };
    if let Some(quantum) = options.quantum {
        props.insert(*pw::keys::NODE_LATENCY, format!("{quantum}/{rate}"));
    }
    for (key, value) in &options.properties {
        props.insert(key.as_str(), value.as_str());
    }

    let stream = pw::stream::StreamBox::new(&core, &options.node_name, props).map_err(device)?;
    let failure = Rc::new(RefCell::new(None::<String>));
    let failed = failure.clone();
    let failing_loop = mainloop.downgrade();
    let said = log.clone();
    let _listener = stream
        .add_local_listener_with_user_data(Capture::new(writer, stats))
        .state_changed(move |_, _, _, new| match new {
            pw::stream::StreamState::Error(reason) => {
                *failed.borrow_mut() = Some(reason);
                if let Some(mainloop) = failing_loop.upgrade() {
                    mainloop.quit();
                }
            }
            pw::stream::StreamState::Streaming => said("something plays into the sink".into()),
            pw::stream::StreamState::Paused => said("nothing plays into the sink".into()),
            _ => {}
        })
        .process(|stream, capture| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let now_ns = crate::clock::now_ns();
            let age = cycle_age_ns(stream);
            let Some(data) = buffer.datas_mut().first_mut() else {
                return;
            };
            let offset = data.chunk().offset() as usize;
            let size = data.chunk().size() as usize;
            let Some(bytes) = data.data() else {
                return;
            };
            let begin = offset.min(bytes.len());
            let end = begin.saturating_add(size).min(bytes.len());
            capture.take(&bytes[begin..end], now_ns, age);
        })
        .register()
        .map_err(device)?;

    let format = format(rate, channels)?;
    let pod = spa::pod::Pod::from_bytes(&format)
        .ok_or_else(|| Error::Device("PipeWire: cannot describe the format".into()))?;
    // Not connected to anything by the session manager: applications come
    // to the sink, the sink does not go looking for a source.
    stream
        .connect(
            spa::utils::Direction::Input,
            None,
            pw::stream::StreamFlags::MAP_BUFFERS | pw::stream::StreamFlags::RT_PROCESS,
            &mut [pod],
        )
        .map_err(device)?;
    log(format!(
        "offering the output \"{}\" ({}) to the applications of this machine",
        options.description, options.node_name
    ));

    // The loop sleeps in PipeWire; a timer looks for the request to stop.
    let stopping_loop = mainloop.downgrade();
    let timer = mainloop.loop_().add_timer(move |_| {
        if stop.load(Ordering::Relaxed) {
            if let Some(mainloop) = stopping_loop.upgrade() {
                mainloop.quit();
            }
        }
    });
    let every = Duration::from_millis(50);
    timer
        .update_timer(Some(every), Some(every))
        .into_result()
        .map_err(|err| Error::Device(format!("PipeWire: {err:?}")))?;

    mainloop.run();
    let _ = stream.disconnect();
    let reason = failure.borrow_mut().take();
    match reason {
        Some(reason) => Err(Error::Device(format!("PipeWire: {reason}"))),
        None => Ok(()),
    }
}

/// Writes the next frames of `tone` into `bytes` as 32-bit floats,
/// little-endian, every channel the same: `wanted` frames, or as many as
/// fit when nothing particular is wanted. Returns the frames written.
fn fill_tone(
    tone: &mut crate::probe::Tone,
    frame: &mut [f32],
    bytes: &mut [u8],
    wanted: usize,
) -> usize {
    let frame_bytes = 4 * frame.len();
    let room = bytes.len() / frame_bytes;
    let frames = if wanted == 0 { room } else { wanted.min(room) };
    for raw in bytes[..frames * frame_bytes].chunks_exact_mut(frame_bytes) {
        tone.fill(frame, frame.len());
        for (sample, out) in frame.iter().zip(raw.chunks_exact_mut(4)) {
            out.copy_from_slice(&sample.to_le_bytes());
        }
    }
    frames
}

/// A test tool: puts a node into PipeWire that produces a steady tone and
/// is connected to nothing. Whoever runs the test links it to the sink
/// under test by hand (`pw-link`); the session manager is not asked to
/// connect it anywhere, so the tone cannot reach a sound card by itself.
/// Runs on the calling thread until `stop` is set.
///
/// The node presents itself as a source that is last in line, not as an
/// application's stream: a stream would be connected by the session
/// manager's rules.
pub fn run_test_tone(
    options: &SinkOptions,
    rate: u32,
    channels: u32,
    tone: crate::probe::Tone,
    stop: Arc<AtomicBool>,
    log: Arc<dyn Fn(String) + Send + Sync>,
) -> Result<()> {
    let device = |err: pw::Error| Error::Device(format!("PipeWire: {err}"));
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(device)?;
    let realtime = pw::properties::properties! {
        *pw::keys::CONFIG_NAME => REALTIME_CLIENT_CONFIG,
    };
    let context = pw::context::ContextRc::new(&mainloop, Some(realtime))
        .or_else(|_| pw::context::ContextRc::new(&mainloop, None))
        .map_err(device)?;
    let core = context.connect_rc(None).map_err(device)?;

    let mut props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CLASS => "Audio/Source",
        *pw::keys::NODE_NAME => options.node_name.as_str(),
        *pw::keys::NODE_DESCRIPTION => options.description.as_str(),
        *pw::keys::NODE_VIRTUAL => "true",
        *pw::keys::PRIORITY_SESSION => "0",
    };
    if let Some(quantum) = options.quantum {
        props.insert(*pw::keys::NODE_LATENCY, format!("{quantum}/{rate}"));
    }
    for (key, value) in &options.properties {
        props.insert(key.as_str(), value.as_str());
    }

    let stream = pw::stream::StreamBox::new(&core, &options.node_name, props).map_err(device)?;
    let failure = Rc::new(RefCell::new(None::<String>));
    let failed = failure.clone();
    let failing_loop = mainloop.downgrade();
    let frame = vec![0.0f32; channels as usize];
    let _listener = stream
        .add_local_listener_with_user_data((tone, frame))
        .state_changed(move |_, _, _, new| {
            if let pw::stream::StreamState::Error(reason) = new {
                *failed.borrow_mut() = Some(reason);
                if let Some(mainloop) = failing_loop.upgrade() {
                    mainloop.quit();
                }
            }
        })
        .process(|stream, (tone, frame)| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let wanted = buffer.requested() as usize;
            let stride = 4 * frame.len();
            let Some(data) = buffer.datas_mut().first_mut() else {
                return;
            };
            let frames = match data.data() {
                Some(bytes) => fill_tone(tone, frame, bytes, wanted),
                None => 0,
            };
            let chunk = data.chunk_mut();
            *chunk.offset_mut() = 0;
            *chunk.stride_mut() = stride as i32;
            *chunk.size_mut() = (stride * frames) as u32;
        })
        .register()
        .map_err(device)?;

    let format = format(rate, channels)?;
    let pod = spa::pod::Pod::from_bytes(&format)
        .ok_or_else(|| Error::Device("PipeWire: cannot describe the format".into()))?;
    stream
        .connect(
            spa::utils::Direction::Output,
            None,
            pw::stream::StreamFlags::MAP_BUFFERS | pw::stream::StreamFlags::RT_PROCESS,
            &mut [pod],
        )
        .map_err(device)?;
    log(format!(
        "a test tone waits in the node {}; it is connected to nothing",
        options.node_name
    ));

    let stopping_loop = mainloop.downgrade();
    let timer = mainloop.loop_().add_timer(move |_| {
        if stop.load(Ordering::Relaxed) {
            if let Some(mainloop) = stopping_loop.upgrade() {
                mainloop.quit();
            }
        }
    });
    let every = Duration::from_millis(50);
    timer
        .update_timer(Some(every), Some(every))
        .into_result()
        .map_err(|err| Error::Device(format!("PipeWire: {err:?}")))?;

    mainloop.run();
    let _ = stream.disconnect();
    let reason = failure.borrow_mut().take();
    match reason {
        Some(reason) => Err(Error::Device(format!("PipeWire: {reason}"))),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::capture_ring;
    use crate::wire::Kind;

    fn bytes(samples: &[f32]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    fn capture() -> (Capture, crate::source::Packetizer, Arc<SinkStats>) {
        let (writer, packets) = capture_ring(2, 48_000.0, 1.0, 0.5, 0.02);
        let stats = Arc::new(SinkStats::default());
        (Capture::new(writer, stats.clone()), packets, stats)
    }

    #[test]
    fn the_test_tone_is_written_as_many_frames_as_are_wanted() {
        let mut tone = crate::probe::Tone::new(997.0, 0.001, 48_000.0);
        let mut frame = [0.0f32; 2];
        let mut bytes = vec![0xffu8; 8 * 300];
        assert_eq!(fill_tone(&mut tone, &mut frame, &mut bytes, 256), 256);
        // What was written is the tone, the same on both channels, and what
        // was not wanted is left alone.
        let mut expected = vec![0.0f32; 512];
        crate::probe::Tone::new(997.0, 0.001, 48_000.0).fill(&mut expected, 2);
        let written: Vec<f32> = bytes[..8 * 256]
            .chunks_exact(4)
            .map(|raw| f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
            .collect();
        assert_eq!(written, expected);
        assert!(bytes[8 * 256..].iter().all(|byte| *byte == 0xff));
        // Nothing particular wanted: as much as fits. More than fits: no
        // more than fits.
        assert_eq!(fill_tone(&mut tone, &mut frame, &mut bytes, 0), 300);
        assert_eq!(fill_tone(&mut tone, &mut frame, &mut bytes, 1000), 300);
    }

    #[test]
    fn a_cycle_becomes_one_packet_stamped_with_the_beginning_of_the_cycle() {
        let (mut capture, mut packets, stats) = capture();
        let audio: Vec<f32> = (0..512).map(|i| i as f32 / 1024.0).collect();
        // The thread ran 0.3 ms after the cycle began.
        capture.take(&bytes(&audio), 5_000_000, Some(300_000));

        let (header, samples) = packets.next_packet().unwrap();
        assert_eq!(header.kind, Kind::Audio);
        assert_eq!(header.position, 0);
        assert_eq!(header.capture_ns, 4_700_000);
        assert_eq!(samples, &audio[..]);
        assert!(packets.next_packet().is_none());

        let seen = stats.snapshot();
        assert_eq!((seen.cycles, seen.frames), (1, 256));
        assert_eq!((seen.frames_min, seen.frames_max), (256, 256));
        assert!((seen.cycle_age_min_ms.unwrap() - 0.3).abs() < 1e-9);
        assert_eq!(seen.cycles_without_time, 0);
    }

    #[test]
    fn a_cycle_the_graph_gives_no_time_for_carries_the_time_it_was_seen() {
        let (mut capture, mut packets, stats) = capture();
        let audio = bytes(&[0.25; 128]);
        capture.take(&audio, 1_000_000, None);
        // A time in the future, and one that is far too old, are not
        // believed either.
        capture.take(&audio, 2_000_000, Some(-5));
        capture.take(&audio, 3_000_000, Some(2_000_000_000));

        for expected in [1_000_000, 2_000_000, 3_000_000] {
            assert_eq!(packets.next_packet().unwrap().0.capture_ns, expected);
        }
        let seen = stats.snapshot();
        assert_eq!(seen.cycles_without_time, 3);
        assert_eq!(seen.cycle_age_min_ms, None);
    }

    #[test]
    fn a_cycle_that_begins_late_shows_and_a_pause_does_not() {
        let (mut capture, _packets, stats) = capture();
        let audio = bytes(&[0.25; 512]);
        let ms = 1_000_000u64;
        // Cycles 5.3 ms apart, then one that begins 40 ms after the last.
        capture.take(&audio, 100 * ms, Some(0));
        capture.take(&audio, 105 * ms + 333_333, Some(0));
        capture.take(&audio, 145 * ms + 333_333, Some(0));
        let seen = stats.snapshot();
        assert!((seen.cycle_step_max_ms - 40.0).abs() < 1e-6, "{seen:?}");
        assert_eq!(seen.resumed, 0);
        // One cycle began late: 40 ms after the last, which lasted 5.3 ms.
        assert_eq!(seen.late_cycles, 1);
        assert!((seen.late_max_ms - 34.667).abs() < 0.01, "{seen:?}");
        assert!((seen.late_total_ms - seen.late_last_ms).abs() < 1e-9);

        // Nothing played for three seconds: that is a pause, not a stall.
        capture.take(&audio, 3_145 * ms, Some(0));
        let seen = stats.snapshot();
        assert!((seen.cycle_step_max_ms - 40.0).abs() < 1e-6, "{seen:?}");
        assert_eq!(seen.resumed, 1);
    }

    #[test]
    fn values_that_are_not_numbers_become_silence() {
        let (mut capture, mut packets, _) = capture();
        capture.take(
            &bytes(&[0.5, f32::NAN, f32::INFINITY, -0.5]),
            1_000,
            Some(0),
        );
        let (_, samples) = packets.next_packet().unwrap();
        assert_eq!(samples, &[0.5, 0.0, 0.0, -0.5]);
    }

    #[test]
    fn half_a_frame_at_the_end_is_left_out_and_nothing_counts_as_no_audio() {
        let (mut capture, mut packets, stats) = capture();
        // Two frames and one more sample.
        capture.take(&bytes(&[0.1, 0.2, 0.3, 0.4, 0.5]), 1_000, Some(0));
        let (_, samples) = packets.next_packet().unwrap();
        assert_eq!(samples, &[0.1, 0.2, 0.3, 0.4]);

        capture.take(&[], 2_000, Some(0));
        capture.take(&bytes(&[0.1]), 3_000, Some(0));
        assert!(packets.next_packet().is_none());
        let seen = stats.snapshot();
        assert_eq!((seen.cycles, seen.cycles_without_audio), (1, 2));
    }

    #[test]
    fn a_cycle_longer_than_a_packet_is_handed_over_in_pieces_that_follow_on() {
        let (writer, mut packets) = capture_ring(2, 48_000.0, 1.0, 0.9, 0.02);
        let stats = Arc::new(SinkStats::default());
        let mut capture = Capture::new(writer, stats.clone());
        let frames = MAX_PACKET_FRAMES + 480;
        capture.take(&bytes(&vec![0.125; frames * 2]), 1_000_000_000, Some(0));

        let (first, samples) = packets.next_packet().unwrap();
        assert_eq!((first.position, samples.len()), (0, MAX_PACKET_FRAMES * 2));
        assert!(!first.discontinuity);
        let (second, samples) = packets.next_packet().unwrap();
        assert_eq!(second.position, MAX_PACKET_FRAMES as u64);
        assert_eq!(samples.len(), 480 * 2);
        assert!(!second.discontinuity);
        // The second piece is stamped later by the length of the first.
        let piece_ns = (MAX_PACKET_FRAMES as f64 / 48_000.0 * 1e9) as u64;
        assert_eq!(second.capture_ns, first.capture_ns + piece_ns);
        assert_eq!(stats.snapshot().cycles, 1);
    }

    #[test]
    fn audio_that_does_not_fit_is_counted_and_shows_as_a_jump() {
        // Room for a tenth of a second only.
        let (writer, mut packets) = capture_ring(2, 48_000.0, 0.1, 0.5, 0.02);
        let stats = Arc::new(SinkStats::default());
        let mut capture = Capture::new(writer, stats.clone());
        let cycle = bytes(&[0.5; 2048 * 2]);
        capture.take(&cycle, 1_000, Some(0));
        capture.take(&cycle, 2_000, Some(0));
        capture.take(&cycle, 3_000, Some(0));
        assert_eq!(stats.snapshot().cycles_not_fitted, 1);

        // What fitted comes out; once there is room again, the next cycle
        // is marked as not following on.
        assert_eq!(packets.next_packet().unwrap().0.position, 0);
        assert_eq!(packets.next_packet().unwrap().0.position, 2048);
        assert!(packets.next_packet().is_none());
        capture.take(&cycle, 4_000, Some(0));
        let (header, _) = packets.next_packet().unwrap();
        assert_eq!(header.position, 3 * 2048);
        assert!(header.discontinuity);
    }
}
