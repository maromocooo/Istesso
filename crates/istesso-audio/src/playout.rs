//! Plays what is in the queue, at the pace of the sound card.
//!
//! [`Playout::render`] is called by the sound card's thread each time it
//! wants frames. It never waits and never allocates. Between the queue and
//! the sound card sit the resampler, whose ratio the controller moves to
//! keep the two clocks together, and a small store of finished frames.
//!
//! Small errors are the controller's business, large ones are handled here,
//! by leaving audio out or by pausing, and every such event is counted:
//!
//! * **Start.** Nothing is played until the newest packet would wait for the
//!   target time. What is older than that is let go, so playing starts at
//!   the target and not at whatever had piled up.
//! * **Underrun.** When the queue cannot supply what the sound card asks
//!   for, the frames that are left end in a short fade and silence follows.
//!   Playing starts again as above. If the source says that it simply has
//!   nothing to play, this is not counted as an underrun.
//! * **Skip.** When a packet would wait much longer than the target
//!   (something delivered a backlog), the audio in between is left out with
//!   a short cross-fade. Latency stays bounded; the backlog is not played
//!   off slowly through the resampler.
//! * **Break in the stream.** When the source let audio go, the two ends are
//!   joined with the same cross-fade.

use std::sync::atomic::{AtomicI64, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

use rtrb::Producer;
use serde::Serialize;

use crate::control::{ControlConfig, Controller};
use crate::fifo::{MarkKind, Reader};
use crate::resample::Resample;

/// Audio captured this much later or earlier than its frame count accounts
/// for, against the audio before it, shows that the source's timeline moved. Less is the jitter
/// of time stamps; a second or more is a clock that cannot be believed.
const SLIP_NS: i64 = 1_000_000;
const SLIP_TOO_LARGE_NS: i64 = 1_000_000_000;

/// Frames handled in one go. A sound card that asks for more gets them in
/// several steps.
const BLOCK_FRAMES: usize = 1024;
/// For how long the packet that travelled fastest is remembered: between
/// one and two of these spans. Long enough to bridge an interruption, short
/// enough for the two clocks not to have moved apart meanwhile.
const FASTEST_SPAN_NS: u64 = 4_000_000_000;
/// Callbacks that have to come at an even pace before playing begins. The
/// first time stamps of a sound card that is just starting are not to be
/// trusted.
const STEADY_CALLBACKS: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayoutConfig {
    pub channels: usize,
    /// Frames per second of the stream, by the source's clock.
    pub stream_rate: f64,
    /// Frames per second the sound card takes, by its own clock.
    pub device_rate: f64,
    pub control: ControlConfig,
    /// A packet that would wait this much longer than the target is not
    /// waited for: the playout skips ahead.
    pub skip_above_s: f64,
    /// Length of fades and cross-fades, in output frames.
    pub fade_frames: usize,
    /// Silence that lasts longer than this is an underrun, unless the source
    /// said that it has nothing to play.
    pub idle_grace_s: f64,
}

impl PlayoutConfig {
    /// The settings used unless a test says otherwise. `target_s` is the
    /// time the fastest packets wait between arriving and being played.
    pub fn new(channels: usize, stream_rate: f64, device_rate: f64, target_s: f64) -> Self {
        PlayoutConfig {
            channels,
            stream_rate,
            device_rate,
            control: ControlConfig::with_target(target_s),
            skip_above_s: 0.020,
            // About three milliseconds.
            fade_frames: (device_rate * 0.003).round() as usize,
            idle_grace_s: 1.5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Nothing has been played yet.
    Waiting,
    Playing,
    /// The queue ran dry and it is not known why.
    Starved,
    /// The source has nothing to play.
    Idle,
}

impl State {
    fn from_u8(value: u8) -> State {
        match value {
            1 => State::Playing,
            2 => State::Starved,
            3 => State::Idle,
            _ => State::Waiting,
        }
    }
}

/// One packet's wait, measured when the packet was first seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitEvent {
    /// When the source captured the packet's first frame (source's clock).
    pub capture_ns: u64,
    /// When the packet arrived (this machine's clock).
    pub arrival_ns: u64,
    /// From the arrival until the first frame is handed to the sound card.
    pub wait_ns: i64,
}

/// Counters the sound card's thread keeps and other threads read.
#[derive(Debug)]
pub struct PlayoutStats {
    state: AtomicU8,
    starts: AtomicU64,
    underruns: AtomicU64,
    underrun_ns: AtomicU64,
    skips: AtomicU64,
    skipped_frames: AtomicU64,
    source_gaps: AtomicU64,
    source_slips: AtomicU64,
    source_slip_us: AtomicU64,
    trimmed_frames: AtomicU64,
    frames_in: AtomicU64,
    frames_out: AtomicU64,
    resample_errors: AtomicU64,
    correction_ppb: AtomicI64,
    drift_ppb: AtomicI64,
    wait_us: AtomicI64,
    wait_min_us: AtomicI64,
    wait_max_us: AtomicI64,
}

impl Default for PlayoutStats {
    fn default() -> Self {
        PlayoutStats {
            state: AtomicU8::new(0),
            starts: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            underrun_ns: AtomicU64::new(0),
            skips: AtomicU64::new(0),
            skipped_frames: AtomicU64::new(0),
            source_gaps: AtomicU64::new(0),
            source_slips: AtomicU64::new(0),
            source_slip_us: AtomicU64::new(0),
            trimmed_frames: AtomicU64::new(0),
            frames_in: AtomicU64::new(0),
            frames_out: AtomicU64::new(0),
            resample_errors: AtomicU64::new(0),
            correction_ppb: AtomicI64::new(0),
            drift_ppb: AtomicI64::new(0),
            wait_us: AtomicI64::new(0),
            wait_min_us: AtomicI64::new(i64::MAX),
            wait_max_us: AtomicI64::new(i64::MIN),
        }
    }
}

/// The counters at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Snapshot {
    pub state: State,
    /// Times playing began, the first time included.
    pub starts: u64,
    /// Times the queue ran dry while the source was sending.
    pub underruns: u64,
    pub underrun_ms: f64,
    /// Times audio was left out to get back to the target.
    pub skips: u64,
    pub skipped_frames: u64,
    /// Breaks in the stream that the source announced.
    pub source_gaps: u64,
    /// Times the source's own timeline moved by a millisecond or more
    /// between two packets that follow on (a cycle of its sound system
    /// that began late), and by how much in all.
    pub source_slips: u64,
    pub source_slip_ms: f64,
    /// Frames let go when playing began, to begin at the target.
    pub trimmed_frames: u64,
    pub frames_in: u64,
    pub frames_out: u64,
    pub resample_errors: u64,
    /// The correction of the playback rate, in parts per million.
    pub correction_ppm: f64,
    /// The estimate of how much faster the sound card's clock runs than the
    /// source's, in parts per million.
    pub drift_ppm: f64,
    /// The wait of the fastest packets of the last second, in milliseconds.
    pub wait_ms: f64,
    /// Its extremes while playing. `None` before anything was measured.
    pub wait_min_ms: Option<f64>,
    pub wait_max_ms: Option<f64>,
}

impl PlayoutStats {
    pub fn snapshot(&self) -> Snapshot {
        let get = |value: &AtomicU64| value.load(Ordering::Relaxed);
        let min = self.wait_min_us.load(Ordering::Relaxed);
        let max = self.wait_max_us.load(Ordering::Relaxed);
        Snapshot {
            state: State::from_u8(self.state.load(Ordering::Relaxed)),
            starts: get(&self.starts),
            underruns: get(&self.underruns),
            underrun_ms: get(&self.underrun_ns) as f64 * 1e-6,
            skips: get(&self.skips),
            skipped_frames: get(&self.skipped_frames),
            source_gaps: get(&self.source_gaps),
            source_slips: get(&self.source_slips),
            source_slip_ms: get(&self.source_slip_us) as f64 * 1e-3,
            trimmed_frames: get(&self.trimmed_frames),
            frames_in: get(&self.frames_in),
            frames_out: get(&self.frames_out),
            resample_errors: get(&self.resample_errors),
            correction_ppm: self.correction_ppb.load(Ordering::Relaxed) as f64 * 1e-3,
            drift_ppm: self.drift_ppb.load(Ordering::Relaxed) as f64 * 1e-3,
            wait_ms: self.wait_us.load(Ordering::Relaxed) as f64 * 1e-3,
            wait_min_ms: (min != i64::MAX).then_some(min as f64 * 1e-3),
            wait_max_ms: (max != i64::MIN).then_some(max as f64 * 1e-3),
        }
    }

    /// Forgets the extremes of the wait, to measure them over a new span.
    pub fn reset_wait_extremes(&self) {
        self.wait_min_us.store(i64::MAX, Ordering::Relaxed);
        self.wait_max_us.store(i64::MIN, Ordering::Relaxed);
    }
}

fn add(counter: &AtomicU64, value: u64) {
    counter.fetch_add(value, Ordering::Relaxed);
}

/// How long the frame with this index waits between `arrival_ns` and being
/// played, when the frame `taken - pipeline` is the one played at `now_ns`:
/// the time it has waited so far, and the time the frames ahead of it take.
fn wait_of(
    index: u64,
    arrival_ns: u64,
    taken: u64,
    pipeline: f64,
    stream_rate: f64,
    now_ns: u64,
) -> f64 {
    let ahead = (index as i64 - taken as i64) as f64 + pipeline;
    let waited = (now_ns as i64 - arrival_ns as i64) as f64 * 1e-9;
    waited + ahead / stream_rate
}

/// What is remembered of a packet that arrived.
#[derive(Debug, Clone, Copy)]
struct Seen {
    index: u64,
    arrival_ns: u64,
    /// From its capture to its arrival. The two times are on different
    /// clocks, so only differences between packets mean anything.
    on_the_way_ns: i64,
}

pub struct Playout<R: Resample> {
    cfg: PlayoutConfig,
    /// Output frames per input frame when the clocks agree.
    nominal: f64,
    reader: Reader,
    resampler: R,
    controller: Controller,
    stats: Arc<PlayoutStats>,
    waits: Option<Producer<WaitEvent>>,

    state: State,
    starved_since_ns: u64,
    starved_counted: bool,
    last_now_ns: Option<u64>,
    correction: f64,
    /// The newest packet seen.
    newest: Option<Seen>,
    /// Where the newest packet stood on the source's timeline: its capture
    /// time less the time its position stands for.
    timeline_ns: Option<i64>,
    /// The shortest time a packet took from being captured to arriving, on
    /// whatever scale the difference of the two clocks gives it: of the
    /// current span of time, and of the one before. A packet that took
    /// longer than this was held up on the way by the difference.
    fastest_ns: [Option<i64>; 2],
    fastest_since_ns: u64,
    /// Frames of the last callback, and how many callbacks in a row came
    /// when the one before said the next would.
    last_frames: usize,
    steady: u32,

    /// Finished frames that have not been handed over yet.
    staging: Vec<f32>,
    staged_start: usize,
    staged: usize,
    /// The faded end of audio that was cut off, still to be mixed into the
    /// next frames handed over.
    tail: Vec<f32>,
    tail_pos: usize,
    tail_len: usize,
    /// Output frames it takes until audio that went into the resampler
    /// comes out of it. New audio is kept silent for that long before it
    /// fades in: what comes out earlier is the resampler finding its feet.
    hold_frames: usize,
    hold_left: usize,
    /// Frames of the fade-in that are still to come.
    fade_in_left: usize,
    input: Vec<f32>,
}

impl<R: Resample> Playout<R> {
    /// `waits`, when given, receives the wait of every packet, for
    /// statistics; what does not fit is dropped.
    pub fn new(
        cfg: PlayoutConfig,
        reader: Reader,
        resampler: R,
        stats: Arc<PlayoutStats>,
        waits: Option<Producer<WaitEvent>>,
    ) -> Self {
        let chunk = resampler.chunk_frames();
        let nominal = cfg.device_rate / cfg.stream_rate;
        // A few frames more than the delay, for the ripples around it.
        let hold_frames = (resampler.latency_input_frames() * nominal).ceil() as usize + 16;
        let staging_frames = BLOCK_FRAMES + cfg.fade_frames + hold_frames + 2 * chunk;
        Playout {
            nominal,
            controller: Controller::new(cfg.control),
            staging: vec![0.0; staging_frames * cfg.channels],
            tail: vec![0.0; (cfg.fade_frames + hold_frames) * cfg.channels],
            hold_frames,
            hold_left: 0,
            input: vec![0.0; resampler.input_frames_max() * cfg.channels],
            cfg,
            reader,
            resampler,
            stats,
            waits,
            state: State::Waiting,
            starved_since_ns: 0,
            starved_counted: false,
            last_now_ns: None,
            correction: 0.0,
            newest: None,
            fastest_ns: [None, None],
            timeline_ns: None,
            fastest_since_ns: 0,
            last_frames: 0,
            steady: 0,
            staged_start: 0,
            staged: 0,
            tail_pos: 0,
            tail_len: 0,
            fade_in_left: 0,
        }
    }

    /// Gives back what outlives a playout: the queue it reads, its counts
    /// and where its waits go. When the output device is replaced by one
    /// with another rate, a new playout is made of these; what is in the
    /// queue stays there, and the counts go on.
    pub fn into_parts(self) -> (Reader, Arc<PlayoutStats>, Option<Producer<WaitEvent>>) {
        (self.reader, self.stats, self.waits)
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// The correction of the playback rate in use.
    pub fn correction(&self) -> f64 {
        self.correction
    }

    /// The controller's estimate of the difference of the two clocks.
    pub fn drift(&self) -> f64 {
        self.controller.drift()
    }

    /// Writes the next `frames` frames into `out`. `now_ns` is the time, on
    /// the clock the arrivals are stamped with, at which the first of these
    /// frames is handed to the sound card.
    pub fn render(&mut self, frames: usize, out: &mut [f32], now_ns: u64) {
        let ch = self.cfg.channels;
        debug_assert_eq!(out.len(), frames * ch);
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(BLOCK_FRAMES);
            let offset_ns = (done as f64 / self.cfg.device_rate * 1e9) as u64;
            self.render_block(n, &mut out[done * ch..(done + n) * ch], now_ns + offset_ns);
            done += n;
        }
    }

    fn render_block(&mut self, frames: usize, out: &mut [f32], now_ns: u64) {
        let now_s = now_ns as f64 * 1e-9;
        let dt_s = match self.last_now_ns {
            Some(last) if now_ns > last => ((now_ns - last) as f64 * 1e-9).min(0.5),
            _ => frames as f64 / self.cfg.device_rate,
        };
        // Did this callback come when the last one said it would?
        let expected_s = self.last_frames as f64 / self.cfg.device_rate;
        let on_time = self.last_now_ns.is_some_and(|last| {
            let passed_s = (now_ns as i64 - last as i64) as f64 * 1e-9;
            (passed_s - expected_s).abs() <= (0.5 * expected_s).max(0.002)
        });
        self.steady = if on_time { self.steady + 1 } else { 0 };
        self.last_frames = frames;
        self.last_now_ns = Some(now_ns);
        let target = self.cfg.control.target_s;

        // Let go of what is too old to say which packets are the fast ones.
        let since = now_ns.saturating_sub(self.fastest_since_ns);
        if since >= 2 * FASTEST_SPAN_NS {
            self.fastest_ns = [None, None];
            self.fastest_since_ns = now_ns;
        } else if since >= FASTEST_SPAN_NS {
            self.fastest_ns = [None, self.fastest_ns[0]];
            self.fastest_since_ns = now_ns;
        }

        // What arrived since the last look, and how long it will wait. When
        // nothing is being played, the wait is the one it would have if
        // playing began now, with nothing but the resampler in the way.
        let pipeline = if self.state == State::Playing {
            self.pipeline_frames()
        } else {
            self.resampler.latency_input_frames()
        };
        let taken = self.reader.taken();
        let stream_rate = self.cfg.stream_rate;
        let mut longest: Option<f64> = None;
        let mut newest = self.newest;
        let mut fastest = self.fastest_ns[0];
        let waits = &mut self.waits;
        // The newest packet's place on the source's timeline: when its first
        // frame was captured, less the time its position stands for.
        let mut timeline: Option<(u64, i64)> = None;
        self.reader.scan_new(|mark| {
            if mark.kind == MarkKind::Audio && timeline.is_none_or(|(index, _)| mark.index >= index)
            {
                let counted_ns = (mark.position as f64 / stream_rate * 1e9) as i64;
                timeline = Some((mark.index, mark.capture_ns as i64 - counted_ns));
            }
            let seen = Seen {
                index: mark.index,
                arrival_ns: mark.arrival_ns,
                on_the_way_ns: mark.arrival_ns as i64 - mark.capture_ns as i64,
            };
            if newest.is_none_or(|newest| seen.index >= newest.index) {
                newest = Some(seen);
            }
            if fastest.is_none_or(|fastest| seen.on_the_way_ns < fastest) {
                fastest = Some(seen.on_the_way_ns);
            }
            let wait = wait_of(
                mark.index,
                mark.arrival_ns,
                taken,
                pipeline,
                stream_rate,
                now_ns,
            );
            longest = Some(longest.map_or(wait, |other| other.max(wait)));
            if let Some(waits) = waits.as_mut() {
                let _ = waits.push(WaitEvent {
                    capture_ns: mark.capture_ns,
                    arrival_ns: mark.arrival_ns,
                    wait_ns: (wait * 1e9) as i64,
                });
            }
        });
        self.newest = newest;
        self.fastest_ns[0] = fastest;
        // Audio that was captured later than its frame count accounts for,
        // compared with the audio before it: the source's timeline moved.
        if let Some((_, now)) = timeline {
            if let Some(before) = self.timeline_ns.replace(now) {
                let slip = now - before;
                if (SLIP_NS..SLIP_TOO_LARGE_NS).contains(&slip.abs()) {
                    self.controller.source_slipped(slip as f64 * 1e-9);
                    add(&self.stats.source_slips, 1);
                    add(&self.stats.source_slip_us, slip.unsigned_abs() / 1000);
                }
            }
        }

        // Frames of silence in front of the audio, when playing begins in
        // the middle of what is asked for.
        let mut silent = 0;
        if self.state == State::Playing {
            if let Some(wait) = longest {
                self.controller.observe(now_s, wait);
            }
            // Too much has piled up: skip ahead rather than play it late.
            if let Some(wait) = self.newest_wait(now_ns, pipeline) {
                if wait > target + self.cfg.skip_above_s {
                    self.skip(wait - target);
                }
            }
        } else {
            self.while_silent(now_ns);
            if self.steady >= STEADY_CALLBACKS {
                silent = self.begin(frames, now_ns, pipeline);
            }
        }

        if self.state == State::Playing {
            let ch = self.cfg.channels;
            let (quiet, audible) = out.split_at_mut(silent * ch);
            self.silence(silent, quiet);
            self.correction = self.controller.step(now_s, dt_s);
            self.resampler.set_relative_ratio(1.0 + self.correction);
            let (produced, ended) = self.fill(frames - silent, audible);
            if ended {
                self.starve(now_ns + self.frames_ns(silent + produced));
            }
        } else {
            self.silence(frames, out);
        }
        self.publish(now_s);
    }

    /// How long `frames` frames last at the sound card.
    fn frames_ns(&self, frames: usize) -> u64 {
        (frames as f64 / self.cfg.device_rate * 1e9) as u64
    }

    /// Begins to play if it is time: when a packet that travels as fast as
    /// the fastest ones has waited, or will have waited within the `frames`
    /// asked for now, as long as the target says. Returns how many of those
    /// frames stay silent before the audio begins, so that it begins
    /// exactly on time.
    ///
    /// A packet that has already waited longer has a backlog in front of
    /// it; that is let go, and playing begins at the target all the same.
    fn begin(&mut self, frames: usize, now_ns: u64, pipeline: f64) -> usize {
        // Without at least one piece of input there is nothing to begin
        // with, however long the last packet has been lying around.
        if self.reader.available() < self.resampler.input_frames_next() {
            return 0;
        }
        let Some(newest) = self.newest else {
            return 0;
        };
        // The newest packet may have been held up on the way. One that was
        // not would have arrived that much earlier, and would have waited
        // that much longer; it is that wait that is to be the target.
        let held_up_ns = match self.fastest_ns {
            [Some(a), Some(b)] => newest.on_the_way_ns - a.min(b),
            [a, b] => newest.on_the_way_ns - a.or(b).unwrap_or(newest.on_the_way_ns),
        };
        let wait = self.wait_of_packet(newest, now_ns, pipeline) + held_up_ns.max(0) as f64 * 1e-9;
        let target = self.cfg.control.target_s;
        if wait >= target {
            self.start(now_ns, wait - target);
            return 0;
        }
        let silent = ((target - wait) * self.cfg.device_rate).ceil() as usize;
        if silent >= frames {
            return 0;
        }
        self.start(now_ns + self.frames_ns(silent), 0.0);
        silent
    }

    /// Input frames that were taken from the queue but are not out yet.
    fn pipeline_frames(&self) -> f64 {
        let ratio = self.nominal * (1.0 + self.correction);
        self.resampler.latency_input_frames() + self.staged as f64 / ratio
    }

    fn newest_wait(&self, now_ns: u64, pipeline: f64) -> Option<f64> {
        self.newest
            .map(|newest| self.wait_of_packet(newest, now_ns, pipeline))
    }

    fn wait_of_packet(&self, packet: Seen, now_ns: u64, pipeline: f64) -> f64 {
        wait_of(
            packet.index,
            packet.arrival_ns,
            self.reader.taken(),
            pipeline,
            self.cfg.stream_rate,
            now_ns,
        )
    }

    /// Finds out whether a silence is the source's doing, and counts it as
    /// an underrun once it has lasted too long to be anything else.
    fn while_silent(&mut self, now_ns: u64) {
        if self.state != State::Starved {
            return;
        }
        if self.reader.idle() {
            self.state = State::Idle;
            return;
        }
        let silent_s = now_ns.saturating_sub(self.starved_since_ns) as f64 * 1e-9;
        if !self.starved_counted && silent_s > self.cfg.idle_grace_s {
            self.starved_counted = true;
            add(&self.stats.underruns, 1);
        }
    }

    /// Begins to play. `excess_s` is how much longer than the target the
    /// newest packet would wait; that much of the oldest audio is let go.
    fn start(&mut self, now_ns: u64, excess_s: f64) {
        let excess = (excess_s * self.cfg.stream_rate) as usize;
        let trimmed = self.reader.discard(excess);
        add(&self.stats.trimmed_frames, trimmed as u64);
        // A break that lies exactly where playing begins joins nothing.
        if self.reader.at_boundary() {
            self.reader.clear_boundary();
        }
        if self.state == State::Starved {
            if !self.starved_counted {
                add(&self.stats.underruns, 1);
            }
            add(
                &self.stats.underrun_ns,
                now_ns.saturating_sub(self.starved_since_ns),
            );
        }
        self.resampler.reset();
        self.staged = 0;
        self.staged_start = 0;
        self.hold_left = self.hold_frames;
        self.fade_in_left = self.cfg.fade_frames;
        self.controller.restart();
        self.state = State::Playing;
        add(&self.stats.starts, 1);
    }

    /// The queue could not supply what was asked for.
    fn starve(&mut self, at_ns: u64) {
        self.starved_since_ns = at_ns;
        self.starved_counted = false;
        self.state = if self.reader.idle() {
            State::Idle
        } else {
            State::Starved
        };
    }

    /// Leaves out `excess_s` seconds of audio and joins the ends.
    fn skip(&mut self, excess_s: f64) {
        // The frames in the store are let go by the join as well.
        let ratio = self.nominal * (1.0 + self.correction);
        let frames = excess_s * self.cfg.stream_rate - self.staged as f64 / ratio;
        if frames < 1.0 {
            return;
        }
        let dropped = self.reader.discard(frames as usize);
        if dropped == 0 {
            return;
        }
        if self.reader.at_boundary() {
            self.reader.clear_boundary();
        }
        self.join();
        self.controller.restart();
        add(&self.stats.skips, 1);
        add(&self.stats.skipped_frames, dropped as u64);
    }

    /// Prepares a cross-fade from the audio in the store to whatever is
    /// produced next. The first frames of the store become a tail that is
    /// mixed into the next frames handed over: at full level for as long as
    /// the new audio is held back, then fading out while the new audio
    /// fades in. The rest of the store is let go.
    fn join(&mut self) {
        let ch = self.cfg.channels;
        let store = self.staged_start * ch;
        // What an earlier join still had to mix goes into the store first.
        let pending = (self.tail_len - self.tail_pos).min(self.staged);
        let from = self.tail_pos * ch;
        for (sample, tail) in self.staging[store..store + pending * ch]
            .iter_mut()
            .zip(&self.tail[from..from + pending * ch])
        {
            *sample += *tail;
        }

        let keep = self.staged.min(self.hold_frames + self.cfg.fade_frames);
        let ramp = keep.min(self.cfg.fade_frames);
        let full = keep - ramp;
        for frame in 0..keep {
            let gain = if frame < full {
                1.0
            } else {
                1.0 - (frame - full + 1) as f32 / ramp as f32
            };
            for c in 0..ch {
                self.tail[frame * ch + c] = self.staging[store + frame * ch + c] * gain;
            }
        }
        self.tail_pos = 0;
        self.tail_len = keep;
        self.staged = 0;
        self.staged_start = 0;
        // The new audio starts from nothing inside the resampler.
        self.resampler.reset();
        self.resampler.set_relative_ratio(1.0 + self.correction);
        self.hold_left = self.hold_frames;
        self.fade_in_left = self.cfg.fade_frames;
    }

    /// Hands over `frames` frames, or fewer when the queue is running dry.
    /// Returns how many, and whether playing has ended with them. When it
    /// has, the audio ended in a fade and the rest of `out` is silence.
    fn fill(&mut self, frames: usize, out: &mut [f32]) -> (usize, bool) {
        let ch = self.cfg.channels;
        let fade = self.cfg.fade_frames;
        // Besides what is asked for, keep enough in the store to end softly
        // or to cross-fade, should that be needed next time.
        let reserve = fade + self.hold_frames;
        while self.staged < frames + reserve {
            if !self.produce() {
                break;
            }
        }
        // The store may be drawn on while the queue is short, but only as
        // far as leaves enough for a fade that deserves the name.
        if self.staged >= frames + fade / 2 {
            self.deliver(frames, out);
            return (frames, false);
        }

        // The queue is running dry: end now, while the store can still make
        // the end a soft one. Less than one piece of input may be left in
        // the queue; it cannot be played and must not stay in front of what
        // comes later.
        let rest = self.reader.available();
        if rest < self.resampler.input_frames_next() {
            self.reader.discard(rest);
        }
        self.fade_out_store();
        let have = self.staged.min(frames);
        self.deliver(have, &mut out[..have * ch]);
        self.store_becomes_tail();
        self.silence(frames - have, &mut out[have * ch..]);
        (have, true)
    }

    /// Moves what is left in the store to the tail that is mixed into the
    /// next frames handed over, whatever they are.
    fn store_becomes_tail(&mut self) {
        let ch = self.cfg.channels;
        let left = self.tail_len - self.tail_pos;
        self.tail
            .copy_within(self.tail_pos * ch..self.tail_len * ch, 0);
        let keep = self.staged.min(self.cfg.fade_frames + self.hold_frames);
        let len = left.max(keep);
        self.tail[left * ch..len * ch].fill(0.0);
        let from = self.staged_start * ch;
        for (tail, sample) in self.tail[..keep * ch]
            .iter_mut()
            .zip(&self.staging[from..from + keep * ch])
        {
            *tail += *sample;
        }
        self.tail_pos = 0;
        self.tail_len = len;
        self.staged = 0;
        self.staged_start = 0;
    }

    /// Silence, with what is left of a fading tail on top.
    fn silence(&mut self, frames: usize, out: &mut [f32]) {
        out.fill(0.0);
        self.mix_tail(frames, out);
    }

    fn mix_tail(&mut self, frames: usize, out: &mut [f32]) {
        let ch = self.cfg.channels;
        let fading = frames.min(self.tail_len - self.tail_pos);
        let from = self.tail_pos * ch;
        for (sample, tail) in out[..fading * ch]
            .iter_mut()
            .zip(&self.tail[from..from + fading * ch])
        {
            *sample += *tail;
        }
        self.tail_pos += fading;
    }

    /// Runs the resampler once. Returns `false` when the queue does not hold
    /// what it needs.
    fn produce(&mut self) -> bool {
        let ch = self.cfg.channels;
        let chunk = self.resampler.chunk_frames();
        let mut need = self.resampler.input_frames_next();
        if self.reader.available() < need {
            return false;
        }
        // Does the stream break within what is read next?
        let mut joined = false;
        if self.reader.at_boundary() {
            self.reader.clear_boundary();
            joined = true;
        } else {
            let run = self.reader.contiguous(need);
            if run < need {
                // Let the short rest of the old audio go; the join covers it.
                self.reader.discard(run);
                if self.reader.at_boundary() {
                    self.reader.clear_boundary();
                }
                joined = true;
            }
        }
        if joined {
            self.source_gap();
            // The join starts the resampler afresh, which changes what it
            // reads next.
            need = self.resampler.input_frames_next();
            if self.reader.available() < need {
                return false;
            }
        }

        // Make room at the end of the store.
        let capacity = self.staging.len() / ch.max(1);
        if ch > 0 && self.staged_start + self.staged + chunk > capacity {
            let from = self.staged_start * ch;
            self.staging.copy_within(from..from + self.staged * ch, 0);
            self.staged_start = 0;
        }

        self.reader.read(need, &mut self.input[..need * ch]);
        let at = (self.staged_start + self.staged) * ch;
        let piece = &mut self.staging[at..at + chunk * ch];
        if !self.resampler.process(&self.input[..need * ch], piece) {
            add(&self.stats.resample_errors, 1);
        }
        if self.hold_left > 0 || self.fade_in_left > 0 {
            let total = self.cfg.fade_frames as f32;
            for frame in piece.chunks_exact_mut(ch.max(1)) {
                let gain = if self.hold_left > 0 {
                    self.hold_left -= 1;
                    0.0
                } else if self.fade_in_left > 0 {
                    self.fade_in_left -= 1;
                    1.0 - (self.fade_in_left + 1) as f32 / total
                } else {
                    break;
                };
                for sample in frame {
                    *sample *= gain;
                }
            }
            if ch == 0 {
                // No samples to walk over; keep the count of frames right.
                let held = self.hold_left.min(chunk);
                self.hold_left -= held;
                self.fade_in_left -= self.fade_in_left.min(chunk - held);
            }
        }
        self.staged += chunk;
        add(&self.stats.frames_in, need as u64);
        true
    }

    fn source_gap(&mut self) {
        self.join();
        add(&self.stats.source_gaps, 1);
    }

    /// Lets the frames in the store end in silence.
    fn fade_out_store(&mut self) {
        let ch = self.cfg.channels;
        let fade = self.staged.min(self.cfg.fade_frames);
        let first = self.staged_start + self.staged - fade;
        for frame in 0..fade {
            let gain = 1.0 - (frame + 1) as f32 / fade as f32;
            for sample in &mut self.staging[(first + frame) * ch..(first + frame + 1) * ch] {
                *sample *= gain;
            }
        }
    }

    /// Copies `frames` frames out of the store, silence where the store has
    /// none, and mixes in what is left of a fading tail.
    fn deliver(&mut self, frames: usize, out: &mut [f32]) {
        let ch = self.cfg.channels;
        let stored = frames.min(self.staged);
        let from = self.staged_start * ch;
        out[..stored * ch].copy_from_slice(&self.staging[from..from + stored * ch]);
        out[stored * ch..].fill(0.0);
        self.staged -= stored;
        self.staged_start = if self.staged == 0 {
            0
        } else {
            self.staged_start + stored
        };

        self.mix_tail(frames, out);
        add(&self.stats.frames_out, frames as u64);
    }

    fn publish(&mut self, now_s: f64) {
        let stats = &self.stats;
        stats.state.store(self.state as u8, Ordering::Relaxed);
        stats
            .correction_ppb
            .store((self.correction * 1e9) as i64, Ordering::Relaxed);
        stats
            .drift_ppb
            .store((self.controller.drift() * 1e9) as i64, Ordering::Relaxed);
        if self.state != State::Playing {
            return;
        }
        if let Some(wait) = self.controller.wait(now_s) {
            let wait_us = (wait * 1e6) as i64;
            stats.wait_us.store(wait_us, Ordering::Relaxed);
            stats.wait_min_us.fetch_min(wait_us, Ordering::Relaxed);
            stats.wait_max_us.fetch_max(wait_us, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fifo::{queue, Writer};
    use crate::resample::{Counting, Sinc};

    const RATE: f64 = 48_000.0;
    const PACKET: usize = 256;
    const CALLBACK: usize = 512;
    const TARGET: f64 = 0.030;

    /// A source that produces packets and a sound card that asks for
    /// frames, both on one perfect clock, with the playout in between.
    struct Rig<R: Resample> {
        writer: Writer,
        playout: Playout<R>,
        stats: Arc<PlayoutStats>,
        channels: usize,
        /// Frames the source has produced, and frames it has let go.
        position: u64,
        /// Time of the next callback, in frames of the sound card.
        frame: u64,
        /// Everything that was played.
        played: Vec<f32>,
    }

    /// The largest step from one sample to the next, and the largest
    /// sample, of what a rig has played.
    fn steepest_and_highest(played: &[f32]) -> (f32, f32) {
        let steepest = played
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0f32, f32::max);
        let highest = played
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0f32, f32::max);
        (steepest, highest)
    }

    #[test]
    fn no_seam_is_a_hard_cut_and_nothing_is_louder_than_the_source() {
        // A constant level is the hardest case: any cut shows as a step of
        // the whole level (0.5 here) from one sample to the next.
        let mut rig = sinc_rig();
        rig.run(1.0);
        // The source says that its audio does not follow on.
        rig.packet(true);
        rig.run(0.5);
        // Nothing arrives for a while: the queue runs dry, then audio is back.
        for _ in 0..((0.3 * RATE) as usize / CALLBACK) {
            rig.callback();
        }
        rig.run(0.5);
        // Far too much arrives at once: the playout has to leave some out.
        for _ in 0..40 {
            rig.packet(false);
        }
        rig.run(0.5);
        // The same again while the last seam is hardly over.
        rig.packet(true);
        rig.callback();
        rig.packet(true);
        rig.run(0.5);
        let seen = rig.snapshot();
        assert!(seen.starts >= 2 && seen.underruns >= 1, "{seen:?}");
        assert!(seen.skips >= 1, "{seen:?}");
        let (steepest, highest) = steepest_and_highest(&rig.played);
        eprintln!("steepest step {steepest}, highest sample {highest}");
        assert!(steepest <= 0.05, "a step of {steepest} between two samples");
        assert!(
            highest <= 0.5 * 1.05,
            "a sample of {highest} from a source of 0.5"
        );
    }

    #[test]
    fn a_value_that_is_not_a_number_leaves_no_trace_in_the_resampler() {
        // Received values are cleaned before they reach the queue. Should
        // one get there all the same, the resampler must not keep it: its
        // filter looks back a fixed number of frames and feeds nothing
        // back, so what a bad value spoils ends with that window.
        let mut rig = sinc_rig();
        rig.run(0.5);
        let bad = [f32::INFINITY, f32::NAN, f32::NEG_INFINITY, f32::NAN];
        let samples: Vec<f32> = bad.iter().cycle().take(PACKET).copied().collect();
        let arrival = rig.now_ns();
        rig.writer
            .push_audio(PACKET, &samples, false, rig.position, arrival, arrival);
        rig.position += PACKET as u64;
        rig.run(0.5);
        // Half a second on, every sample is a number again and at the
        // level of the source.
        let tail = &rig.played[rig.played.len() - (0.2 * RATE) as usize..];
        assert!(tail.iter().all(|sample| sample.is_finite()));
        assert!(
            tail.iter().all(|sample| (sample - 0.5).abs() < 0.01),
            "{:?}",
            &tail[..4]
        );
    }

    #[test]
    fn a_playout_that_is_taken_apart_keeps_what_is_in_the_queue() {
        let (mut writer, reader) = crate::fifo::queue(2, 48_000);
        let samples = vec![0.25f32; 256 * 2];
        for packet in 0..10u64 {
            assert!(writer.push_audio(256, &samples, packet == 0, packet * 256, 0, 0));
        }
        let stats = Arc::new(PlayoutStats::default());
        let first = Playout::new(
            PlayoutConfig::new(2, 48_000.0, 48_000.0, 0.040),
            reader,
            Sinc::new(48_000.0, 48_000.0, 2, 64, 1.01).unwrap(),
            stats.clone(),
            None,
        );
        let (reader, stats_again, waits) = first.into_parts();
        assert_eq!(reader.available(), 10 * 256);
        assert!(Arc::ptr_eq(&stats, &stats_again));
        assert!(waits.is_none());
        // The same queue under a playout for a device with another rate.
        let second = Playout::new(
            PlayoutConfig::new(2, 48_000.0, 44_100.0, 0.040),
            reader,
            Sinc::new(48_000.0, 44_100.0, 2, 64, 1.01).unwrap(),
            stats_again,
            None,
        );
        assert_eq!(second.state(), State::Waiting);
    }

    fn counting_rig() -> Rig<Counting> {
        let cfg = PlayoutConfig::new(0, RATE, RATE, TARGET);
        let (writer, reader) = queue(0, 1 << 20);
        let stats = Arc::new(PlayoutStats::default());
        let playout = Playout::new(
            cfg,
            reader,
            Counting::new(RATE, RATE, 64),
            stats.clone(),
            None,
        );
        Rig {
            writer,
            playout,
            stats,
            channels: 0,
            position: 0,
            frame: 0,
            played: Vec::new(),
        }
    }

    fn sinc_rig() -> Rig<Sinc> {
        sinc_rig_with(PlayoutConfig::new(1, RATE, RATE, TARGET))
    }

    fn sinc_rig_with(cfg: PlayoutConfig) -> Rig<Sinc> {
        let (writer, reader) = queue(1, 1 << 16);
        let stats = Arc::new(PlayoutStats::default());
        let resampler = Sinc::new(RATE, RATE, 1, 64, 1.01).unwrap();
        let playout = Playout::new(cfg, reader, resampler, stats.clone(), None);
        Rig {
            writer,
            playout,
            stats,
            channels: 1,
            position: 0,
            frame: 0,
            played: Vec::new(),
        }
    }

    impl<R: Resample> Rig<R> {
        fn now_ns(&self) -> u64 {
            (self.frame as f64 / RATE * 1e9) as u64
        }

        /// The source produces one packet, which arrives at once. With
        /// audio, it is a constant 0.5: any join or fade shows as a change.
        fn packet(&mut self, discontinuity: bool) {
            let samples = vec![0.5f32; PACKET * self.channels];
            let arrival = self.now_ns();
            self.writer.push_audio(
                PACKET,
                &samples,
                discontinuity,
                self.position,
                arrival,
                arrival,
            );
            self.position += PACKET as u64;
        }

        /// The sound card asks once.
        fn callback(&mut self) {
            let mut out = vec![0.0f32; CALLBACK * self.channels];
            let now = self.now_ns();
            self.playout.render(CALLBACK, &mut out, now);
            self.played.extend_from_slice(&out);
            self.frame += CALLBACK as u64;
        }

        /// `seconds` of both running smoothly: two packets per callback.
        fn run(&mut self, seconds: f64) {
            for _ in 0..(seconds * RATE / CALLBACK as f64) as usize {
                self.packet(false);
                self.packet(false);
                self.callback();
            }
        }

        fn snapshot(&self) -> Snapshot {
            self.stats.snapshot()
        }
    }

    #[test]
    fn playing_begins_once_the_target_is_reached_and_not_before() {
        let mut rig = counting_rig();
        // The sound card has been asking at an even pace for a while.
        for _ in 0..4 {
            rig.callback();
        }
        assert_eq!(rig.playout.state(), State::Waiting);
        // Four packets: the newest has 16 ms of audio in front of it, and
        // the 11 ms asked for now do not bring its wait to 30 ms.
        for _ in 0..4 {
            rig.packet(false);
        }
        rig.callback();
        assert_eq!(rig.playout.state(), State::Waiting);
        // Two more: the newest has 27 ms in front of it, so its wait
        // reaches 30 ms a little into this callback. Playing begins
        // exactly there, and nothing is let go.
        rig.packet(false);
        rig.packet(false);
        rig.callback();
        assert_eq!(rig.playout.state(), State::Playing);
        let snapshot = rig.snapshot();
        assert_eq!(snapshot.starts, 1);
        assert_eq!(snapshot.underruns, 0);
        assert_eq!(snapshot.trimmed_frames, 0);
        rig.run(1.0);
        assert!((rig.snapshot().wait_ms - 30.0).abs() < 0.1);
    }

    #[test]
    fn the_first_time_stamps_of_a_sound_card_are_not_trusted() {
        // A card that is just starting asks once, and then says nothing
        // for 150 ms while it gets going. Audio arrives all the while.
        let mut rig = counting_rig();
        for _ in 0..8 {
            rig.packet(false);
        }
        rig.callback();
        assert_eq!(rig.playout.state(), State::Waiting);
        rig.frame += 7200;
        for _ in 0..28 {
            rig.packet(false);
        }
        // From here on it asks evenly. Playing begins once it has done so a
        // few times, in one go: nothing is skipped afterwards.
        rig.run(2.0);
        let snapshot = rig.snapshot();
        assert_eq!(snapshot.state, State::Playing);
        assert_eq!(snapshot.starts, 1, "{snapshot:?}");
        assert_eq!(snapshot.skips, 0, "{snapshot:?}");
        assert_eq!(snapshot.underruns, 0, "{snapshot:?}");
        assert!((snapshot.wait_ms - 30.0).abs() < 0.5, "{snapshot:?}");
    }

    #[test]
    fn a_packet_that_was_held_up_does_not_set_the_wait() {
        let mut rig = counting_rig();
        rig.run(2.0);
        // The source hesitates for 60 ms: the queue runs dry. Then it
        // catches up, and the last packet of what it sends at once is
        // itself still 4 ms behind its time.
        for _ in 0..6 {
            rig.callback();
        }
        assert_eq!(rig.playout.state(), State::Starved);
        let late_ns = 4_000_000;
        for _ in 0..12 {
            let samples: [f32; 0] = [];
            let arrival = rig.now_ns();
            // Captured when it should have been, arriving late.
            let on_time = arrival - late_ns;
            rig.writer
                .push_audio(PACKET, &samples, false, rig.position, on_time, arrival);
            rig.position += PACKET as u64;
        }
        rig.run(1.0);
        let snapshot = rig.snapshot();
        assert_eq!(snapshot.underruns, 1, "{snapshot:?}");
        // Packets that are on time again wait as long as the target says,
        // not 4 ms longer; so the correction has nothing to make up for.
        assert!((snapshot.wait_ms - 30.0).abs() < 0.5, "{snapshot:?}");
        assert!(snapshot.correction_ppm.abs() < 60.0, "{snapshot:?}");
    }

    #[test]
    fn a_steady_stream_plays_without_events_at_the_target() {
        let mut rig = counting_rig();
        rig.run(60.0);
        let snapshot = rig.snapshot();
        assert_eq!(snapshot.state, State::Playing);
        assert_eq!(snapshot.starts, 1);
        assert_eq!(snapshot.underruns, 0);
        assert_eq!(snapshot.skips, 0);
        assert_eq!(snapshot.source_gaps, 0);
        assert!((snapshot.wait_ms - 30.0).abs() < 1.0, "{snapshot:?}");
        // Same clock on both sides: nothing to correct.
        assert!(snapshot.correction_ppm.abs() < 5.0, "{snapshot:?}");
    }

    #[test]
    fn a_backlog_at_the_start_is_not_played() {
        let mut rig = counting_rig();
        // Half a second arrives at once before the first callback.
        for _ in 0..94 {
            rig.packet(false);
        }
        rig.run(5.0);
        let snapshot = rig.snapshot();
        assert_eq!(snapshot.starts, 1);
        assert!(snapshot.trimmed_frames > 20_000, "{snapshot:?}");
        assert!((snapshot.wait_ms - 30.0).abs() < 1.0, "{snapshot:?}");
    }

    #[test]
    fn a_gap_in_the_arrivals_is_one_underrun_and_ends_at_the_target() {
        let mut rig = counting_rig();
        rig.run(5.0);
        // Nothing arrives for 200 ms, then everything at once.
        for _ in 0..19 {
            rig.callback();
        }
        assert_eq!(rig.playout.state(), State::Starved);
        for _ in 0..38 {
            rig.packet(false);
        }
        rig.run(5.0);
        let snapshot = rig.snapshot();
        assert_eq!(snapshot.state, State::Playing);
        assert_eq!(snapshot.underruns, 1);
        assert_eq!(snapshot.starts, 2);
        // The silence lasted from when the queue ran dry until the backlog
        // came: 200 ms less the 30 ms that were in the queue.
        assert!((snapshot.underrun_ms - 170.0).abs() < 15.0, "{snapshot:?}");
        // The backlog was let go, not played late.
        assert!(snapshot.trimmed_frames > 6000, "{snapshot:?}");
        assert!((snapshot.wait_ms - 30.0).abs() < 1.0, "{snapshot:?}");
    }

    #[test]
    fn silence_announced_by_the_source_is_not_an_underrun() {
        let mut rig = counting_rig();
        rig.run(2.0);
        let idle_at = rig.now_ns();
        rig.writer.push_idle(idle_at);
        for _ in 0..300 {
            rig.callback();
        }
        assert_eq!(rig.playout.state(), State::Idle);
        rig.run(2.0);
        let snapshot = rig.snapshot();
        assert_eq!(snapshot.state, State::Playing);
        assert_eq!(snapshot.underruns, 0);
        assert_eq!(snapshot.starts, 2);
    }

    #[test]
    fn silence_that_is_not_announced_counts_even_if_it_never_ends() {
        let mut rig = counting_rig();
        rig.run(2.0);
        for _ in 0..300 {
            rig.callback();
        }
        let snapshot = rig.snapshot();
        assert_eq!(snapshot.state, State::Starved);
        assert_eq!(snapshot.underruns, 1);
    }

    #[test]
    fn a_burst_while_playing_is_skipped_not_bled_off() {
        let mut rig = counting_rig();
        rig.run(5.0);
        // The source hands over a whole second at once.
        for _ in 0..188 {
            rig.packet(false);
        }
        rig.run(0.1);
        let snapshot = rig.snapshot();
        assert_eq!(snapshot.skips, 1, "{snapshot:?}");
        assert!(snapshot.skipped_frames > 45_000, "{snapshot:?}");
        assert_eq!(snapshot.underruns, 0);
        rig.run(2.0);
        let snapshot = rig.snapshot();
        assert_eq!(snapshot.skips, 1);
        assert!((snapshot.wait_ms - 30.0).abs() < 1.5, "{snapshot:?}");
    }

    #[test]
    fn a_little_too_much_is_left_to_the_controller() {
        let mut rig = counting_rig();
        rig.run(5.0);
        // Two extra packets: about 11 ms more than the target.
        rig.packet(false);
        rig.packet(false);
        rig.run(5.0);
        let snapshot = rig.snapshot();
        assert_eq!(snapshot.skips, 0);
        // The wait is on its way back, at no more than the allowed pace.
        assert!(snapshot.correction_ppm < -100.0, "{snapshot:?}");
        assert!(snapshot.correction_ppm >= -1000.0, "{snapshot:?}");
    }

    #[test]
    fn a_break_announced_by_the_source_is_joined_and_counted() {
        let mut rig = counting_rig();
        rig.run(2.0);
        rig.position += 48_000;
        rig.packet(true);
        rig.packet(false);
        rig.callback();
        rig.run(2.0);
        let snapshot = rig.snapshot();
        assert_eq!(snapshot.source_gaps, 1);
        assert_eq!(snapshot.underruns, 0);
        assert_eq!(snapshot.skips, 0);
    }

    /// Largest step between neighbouring samples of what was played.
    fn largest_step(played: &[f32]) -> f32 {
        played
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn audio_begins_and_ends_softly() {
        let mut rig = sinc_rig();
        rig.run(1.0);
        // The constant 0.5 comes through as 0.5.
        let steady = &rig.played[24_000..];
        assert!(steady.iter().all(|sample| (sample - 0.5).abs() < 0.005));
        // It was reached in steps far smaller than itself (a fade over 144
        // frames, not a jump).
        assert!(largest_step(&rig.played) < 0.02);

        // The source stops; what is left fades out.
        let before = rig.played.len();
        for _ in 0..20 {
            rig.callback();
        }
        assert!(largest_step(&rig.played[before..]) < 0.02);
        assert!(rig.played[rig.played.len() - CALLBACK..]
            .iter()
            .all(|sample| *sample == 0.0));
        assert_eq!(rig.playout.state(), State::Starved);
    }

    #[test]
    fn a_queue_that_empties_bit_by_bit_still_ends_in_a_fade() {
        let mut rig = sinc_rig();
        rig.run(1.0);
        let before = rig.played.len();
        // Only half of what the sound card takes arrives. The store is
        // drawn on before the queue is dry, and playing stops and starts
        // several times; no end and no beginning may be a jump.
        for _ in 0..200 {
            rig.packet(false);
            rig.callback();
        }
        let snapshot = rig.snapshot();
        assert!(snapshot.underruns >= 3, "{snapshot:?}");
        assert!(largest_step(&rig.played[before..]) < 0.02);
    }

    #[test]
    fn a_skip_is_crossfaded() {
        let mut rig = sinc_rig();
        rig.run(1.0);
        let before = rig.played.len();
        for _ in 0..188 {
            rig.packet(false);
        }
        rig.run(1.0);
        assert_eq!(rig.snapshot().skips, 1);
        // Old and new are the same constant, so a cross-fade that adds up
        // to one leaves no trace; a cut or a fade through silence would.
        let after = &rig.played[before..];
        assert!(
            after.iter().all(|sample| (sample - 0.5).abs() < 0.01),
            "largest deviation {}",
            after
                .iter()
                .map(|sample| (sample - 0.5).abs())
                .fold(0.0, f32::max)
        );
    }

    #[test]
    fn a_callback_larger_than_the_block_is_served_whole() {
        // Skipping is put out of the way: the audio for the large callback
        // has to be in the queue beforehand.
        let mut cfg = PlayoutConfig::new(1, RATE, RATE, TARGET);
        cfg.skip_above_s = 10.0;
        let mut rig = sinc_rig_with(cfg);
        rig.run(1.0);
        // 4096 frames in one call, with the audio for it in the queue.
        for _ in 0..18 {
            rig.packet(false);
        }
        let mut out = vec![0.0f32; 4096];
        let now = rig.now_ns();
        rig.playout.render(4096, &mut out, now);
        assert!(out.iter().all(|sample| (sample - 0.5).abs() < 0.005));
    }

    #[test]
    fn waits_are_reported_for_statistics() {
        let cfg = PlayoutConfig::new(0, RATE, RATE, TARGET);
        let (mut writer, reader) = queue(0, 1 << 20);
        let (events_in, mut events_out) = rtrb::RingBuffer::new(64);
        let mut playout = Playout::new(
            cfg,
            reader,
            Counting::new(RATE, RATE, 64),
            Arc::new(PlayoutStats::default()),
            Some(events_in),
        );
        writer.push_audio(PACKET, &[], false, 0, 111, 1_000_000);
        playout.render(CALLBACK, &mut [], 3_000_000);
        let event = events_out.pop().unwrap();
        assert_eq!(event.capture_ns, 111);
        assert_eq!(event.arrival_ns, 1_000_000);
        // First in the queue and 2 ms old: were it played now, it would
        // have waited 2 ms, and the half frame the resampler holds back.
        assert!((event.wait_ns - 2_000_000).abs() < 20_000, "{event:?}");
        assert!(events_out.pop().is_err());
    }
}
