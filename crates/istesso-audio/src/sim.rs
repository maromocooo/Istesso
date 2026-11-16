//! A source, a network and a sound card with clocks of their own.
//!
//! The drift between two real clocks takes hours to show. Here the clocks
//! are numbers: a source that produces packets at the rate of one clock, a
//! network that delays them, and a sound card that asks for frames at the
//! rate of another. The playout in between is the real one. With a
//! resampler that only counts frames, eight hours pass in about a second.
//!
//! Everything is decided by the [`Scenario`], including the random numbers,
//! so a run can be repeated exactly.

use std::sync::Arc;

use serde::Serialize;

use crate::fifo::queue;
use crate::playout::{Playout, PlayoutConfig, PlayoutStats, Snapshot};
use crate::probe::Tone;
use crate::resample::{Counting, Resample, Sinc};

/// The conditions of one run. Times are in seconds of true time, clock
/// errors in parts per million.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Scenario {
    pub seconds: f64,
    /// Frames per second of the stream by the source's own clock, and how
    /// much faster than true time that clock runs.
    pub stream_rate: f64,
    pub source_ppm: f64,
    pub packet_frames: usize,
    /// Frames per second of the sound card by its own clock, and how much
    /// faster than true time that clock runs.
    pub device_rate: f64,
    pub device_ppm: f64,
    /// The clock error of the sound card changes to the given value at the
    /// given time (a card that warms up).
    pub device_ppm_changes: Vec<(f64, f64)>,
    pub callback_frames: usize,
    /// The wait the playout holds.
    pub target_s: f64,
    /// What the network adds to every packet, and up to how much more it
    /// adds to each at random.
    pub delay_s: f64,
    pub jitter_s: f64,
    /// One packet in this many is held up by a further random time of up
    /// to `spike_s`. Zero for none.
    pub spike_one_in: u32,
    pub spike_s: f64,
    /// Spans (start, length) in which nothing is delivered; what was due
    /// then arrives together when the span ends.
    pub stalls: Vec<(f64, f64)>,
    /// How long before its frames are due the sound card asks for them, and
    /// by up to how much that varies either way.
    pub callback_lead_s: f64,
    pub callback_jitter_s: f64,
    /// By up to how much the time stamp of a callback is off, either way.
    pub timestamp_noise_s: f64,
    /// From when on the pitch and the wait are judged.
    pub settle_s: f64,
    pub seed: u64,
}

impl Scenario {
    /// A quiet network and exact clocks at 48 kHz, a sound card that asks
    /// for 512 frames about one such buffer ahead of time, and a wait of
    /// 40 ms: the starting point the tests change one thing at a time from.
    ///
    /// Of the 40 ms, about 27 are spoken for: frames have to be in the queue
    /// when the sound card asks (11 ms ahead), for the whole buffer it asks
    /// for (another 11 ms), plus the few milliseconds the playout keeps in
    /// hand for fades. The rest, 13 ms, is how late a packet may be.
    pub fn quiet(seconds: f64) -> Self {
        Scenario {
            seconds,
            stream_rate: 48_000.0,
            source_ppm: 0.0,
            packet_frames: 256,
            device_rate: 48_000.0,
            device_ppm: 0.0,
            device_ppm_changes: Vec::new(),
            callback_frames: 512,
            target_s: 0.040,
            delay_s: 0.0005,
            jitter_s: 0.0005,
            spike_one_in: 0,
            spike_s: 0.0,
            stalls: Vec::new(),
            callback_lead_s: 0.011,
            callback_jitter_s: 0.0,
            timestamp_noise_s: 0.0,
            settle_s: 300.0,
            seed: 1,
        }
    }

    /// The correction that keeps the wait still for the given clock error
    /// of the sound card: how much faster it takes frames than the source
    /// makes them.
    fn true_drift(&self, device_ppm: f64) -> f64 {
        (1.0 + device_ppm * 1e-6) / (1.0 + self.source_ppm * 1e-6) - 1.0
    }
}

/// What a run showed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Outcome {
    /// The playout's counters at the end.
    pub stats: Snapshot,
    /// The correction that would hold the wait exactly, at the end.
    pub true_drift_ppm: f64,
    /// From `settle_s` on: the largest difference between the correction in
    /// use and the true drift. This is by how much the pitch that is heard
    /// differs from the pitch the source made.
    pub pitch_error_ppm: f64,
    /// From `settle_s` on: the fastest change of the correction.
    pub pitch_slew_ppm_per_s: f64,
    /// From `settle_s` on: the extremes of the wait of the fastest packets.
    pub wait_min_ms: Option<f64>,
    pub wait_max_ms: Option<f64>,
    /// Underruns, skips and announced breaks from `settle_s` on.
    pub underruns_after_settling: u64,
    pub skips_after_settling: u64,
    /// The correction once a minute, in parts per million.
    pub correction_ppm_by_minute: Vec<f64>,
    /// What was played (only when audio was run through).
    #[serde(skip)]
    pub played: Vec<f32>,
}

/// Small, fast and repeatable random numbers (xorshift64*).
struct Random(u64);

impl Random {
    fn new(seed: u64) -> Self {
        Random(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    /// Between 0 and 1.
    fn unit(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let value = self.0.wrapping_mul(0x2545_f491_4f6c_dd1d);
        (value >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Between -1 and 1.
    fn either_way(&mut self) -> f64 {
        2.0 * self.unit() - 1.0
    }
}

/// Runs the scenario with a resampler that only counts frames.
pub fn run(scenario: &Scenario) -> Outcome {
    let resampler = Counting::new(scenario.stream_rate, scenario.device_rate, 64);
    simulate(scenario, resampler, 0, None)
}

/// Runs the scenario with real audio: the source plays a sine of `tone_hz`
/// on one channel, and what the sound card is given is kept.
pub fn run_with_tone(scenario: &Scenario, tone_hz: f64) -> Result<Outcome, String> {
    let resampler = Sinc::new(scenario.stream_rate, scenario.device_rate, 1, 64, 1.01)?;
    let tone = Tone::new(tone_hz, 0.25, scenario.stream_rate);
    Ok(simulate(scenario, resampler, 1, Some(tone)))
}

fn simulate<R: Resample>(
    scenario: &Scenario,
    resampler: R,
    channels: usize,
    mut tone: Option<Tone>,
) -> Outcome {
    let cfg = PlayoutConfig::new(
        channels,
        scenario.stream_rate,
        scenario.device_rate,
        scenario.target_s,
    );
    // Room for ten seconds: a stall delivers its backlog at once.
    let (mut writer, reader) = queue(channels, (scenario.stream_rate * 10.0) as usize);
    let stats = Arc::new(PlayoutStats::default());
    let mut playout = Playout::new(cfg, reader, resampler, stats.clone(), None);

    let mut random = Random::new(scenario.seed);
    let source_rate = scenario.stream_rate * (1.0 + scenario.source_ppm * 1e-6);
    let packet = scenario.packet_frames;
    let mut samples = vec![0.0f32; packet * channels];
    let mut out = vec![0.0f32; scenario.callback_frames * channels];
    let mut played = Vec::new();

    // The arrival of packet `n`, given the arrival of the one before it.
    let arrival = |n: u64, previous: f64, random: &mut Random| -> f64 {
        // A packet exists once its last frame has been captured.
        let captured = (n + 1) as f64 * packet as f64 / source_rate;
        let mut at = captured + scenario.delay_s + scenario.jitter_s * random.unit();
        if scenario.spike_one_in > 0 && random.unit() * f64::from(scenario.spike_one_in) < 1.0 {
            at += scenario.spike_s * random.unit();
        }
        for (start, length) in &scenario.stalls {
            if at >= *start && at < start + length {
                at = start + length;
            }
        }
        // Packets arrive in the order they were sent.
        at.max(previous)
    };

    let mut next_packet = 0u64;
    let mut next_arrival = arrival(0, 0.0, &mut random);
    let mut device_ppm = scenario.device_ppm;
    let mut changes = scenario.device_ppm_changes.iter().peekable();

    // Time at which the frames of the next callback are due. Starts late
    // enough for the first callback not to lie before time zero.
    let mut due = scenario.callback_lead_s + scenario.callback_jitter_s + 0.001;
    let mut previous_correction: Option<f64> = None;
    let mut pitch_error: f64 = 0.0;
    let mut pitch_slew: f64 = 0.0;
    let mut settled = false;
    let mut at_settling = stats.snapshot();
    let mut by_minute = Vec::new();

    while due < scenario.seconds {
        while let Some((at, ppm)) = changes.peek() {
            if *at > due {
                break;
            }
            device_ppm = *ppm;
            changes.next();
        }
        let device_step =
            scenario.callback_frames as f64 / (scenario.device_rate * (1.0 + device_ppm * 1e-6));

        // What has arrived by the time the sound card asks.
        let asked =
            due - scenario.callback_lead_s + scenario.callback_jitter_s * random.either_way();
        while next_arrival <= asked {
            if let Some(tone) = &mut tone {
                tone.fill(&mut samples, channels);
            }
            let arrival_ns = (next_arrival * 1e9) as u64;
            // The first frame of the packet was captured when the source's
            // count of frames reached it.
            let position = next_packet * packet as u64;
            let capture_ns = (position as f64 / source_rate * 1e9) as u64;
            writer.push_audio(packet, &samples, false, position, capture_ns, arrival_ns);
            next_packet += 1;
            next_arrival = arrival(next_packet, next_arrival, &mut random);
        }

        let stamp = due + scenario.timestamp_noise_s * random.either_way();
        playout.render(
            scenario.callback_frames,
            &mut out,
            (stamp.max(0.0) * 1e9) as u64,
        );
        if tone.is_some() {
            played.extend_from_slice(&out);
        }

        let correction = playout.correction();
        if !settled && due >= scenario.settle_s {
            settled = true;
            stats.reset_wait_extremes();
            at_settling = stats.snapshot();
            previous_correction = None;
        }
        if settled {
            let error = (correction - scenario.true_drift(device_ppm)).abs();
            pitch_error = pitch_error.max(error);
            if let Some(previous) = previous_correction {
                pitch_slew = pitch_slew.max((correction - previous).abs() / device_step);
            }
        }
        previous_correction = Some(correction);
        if due >= (by_minute.len() + 1) as f64 * 60.0 {
            by_minute.push(correction * 1e6);
        }
        due += device_step;
    }

    let snapshot = stats.snapshot();
    Outcome {
        true_drift_ppm: scenario.true_drift(device_ppm) * 1e6,
        pitch_error_ppm: pitch_error * 1e6,
        pitch_slew_ppm_per_s: pitch_slew * 1e6,
        wait_min_ms: snapshot.wait_min_ms.filter(|_| settled),
        wait_max_ms: snapshot.wait_max_ms.filter(|_| settled),
        underruns_after_settling: snapshot.underruns - at_settling.underruns,
        skips_after_settling: snapshot.skips - at_settling.skips,
        correction_ppm_by_minute: by_minute,
        stats: snapshot,
        played,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playout::State;
    use crate::probe::analyze_tone;

    const HOUR: f64 = 3600.0;

    /// The bound on what the correction does to the pitch, once the first
    /// five minutes have passed: the pitch that is heard stays within this
    /// many parts per million of the pitch the source made, whatever the
    /// network and the sound card do within what is tested here. 50 ppm is
    /// 0.005%, less than a tenth of a cent; a cent is a hundredth of a
    /// semitone, and differences of a few cents are the smallest that
    /// trained ears tell apart.
    const PITCH_ERROR_PPM: f64 = 50.0;
    /// With a network that delivers evenly and exact time stamps it stays
    /// within this.
    const QUIET_PITCH_ERROR_PPM: f64 = 5.0;
    /// It moves no faster than this many parts per million a second; the
    /// controller does not let it (`ControlConfig::max_slew_per_s`).
    const PITCH_SLEW_PPM_PER_S: f64 = 100.5;

    /// Everything a run without events has to show.
    fn assert_uneventful(outcome: &Outcome) {
        let stats = &outcome.stats;
        assert_eq!(stats.state, State::Playing, "{outcome:?}");
        assert_eq!(stats.starts, 1, "{outcome:?}");
        assert_eq!(stats.underruns, 0, "{outcome:?}");
        assert_eq!(stats.skips, 0, "{outcome:?}");
        assert_eq!(stats.source_gaps, 0, "{outcome:?}");
        assert_eq!(stats.resample_errors, 0, "{outcome:?}");
    }

    fn assert_pitch_bound(outcome: &Outcome) {
        assert!(
            outcome.pitch_error_ppm <= PITCH_ERROR_PPM,
            "pitch error {} ppm: {outcome:?}",
            outcome.pitch_error_ppm
        );
        assert!(
            outcome.pitch_slew_ppm_per_s <= PITCH_SLEW_PPM_PER_S,
            "pitch slew {} ppm/s: {outcome:?}",
            outcome.pitch_slew_ppm_per_s
        );
    }

    fn assert_quiet_pitch(outcome: &Outcome) {
        assert_pitch_bound(outcome);
        assert!(
            outcome.pitch_error_ppm <= QUIET_PITCH_ERROR_PPM,
            "pitch error {} ppm: {outcome:?}",
            outcome.pitch_error_ppm
        );
    }

    /// The wait stayed within `tolerance_ms` of the target.
    fn assert_wait(outcome: &Outcome, target_ms: f64, tolerance_ms: f64) {
        let (min, max) = (outcome.wait_min_ms.unwrap(), outcome.wait_max_ms.unwrap());
        assert!(
            min >= target_ms - tolerance_ms && max <= target_ms + tolerance_ms,
            "wait between {min} and {max} ms: {outcome:?}"
        );
    }

    fn report(name: &str, outcome: &Outcome) {
        println!(
            "{name}: drift {:+.1} ppm, correction {:+.1} ppm, pitch error <= {:.2} ppm, \
             slew <= {:.3} ppm/s, wait {:.2}..{:.2} ms, starts {}, underruns {}, skips {}",
            outcome.true_drift_ppm,
            outcome.stats.correction_ppm,
            outcome.pitch_error_ppm,
            outcome.pitch_slew_ppm_per_s,
            outcome.wait_min_ms.unwrap_or(f64::NAN),
            outcome.wait_max_ms.unwrap_or(f64::NAN),
            outcome.stats.starts,
            outcome.stats.underruns,
            outcome.stats.skips,
        );
    }

    #[test]
    fn eight_hours_with_a_sound_card_that_is_100_ppm_fast() {
        let mut scenario = Scenario::quiet(8.0 * HOUR);
        scenario.device_ppm = 100.0;
        let outcome = run(&scenario);
        report("8 h, card +100 ppm", &outcome);
        assert_uneventful(&outcome);
        assert_quiet_pitch(&outcome);
        assert_wait(&outcome, 40.0, 0.5);
        // The part that remembers has found the difference of the clocks.
        assert!((outcome.stats.drift_ppm - 100.0).abs() < 2.0, "{outcome:?}");
        // Without correction the queue would have run dry within minutes:
        // 100 ppm of eight hours are 2.9 seconds. With it, the card was
        // given 1.0001 frames for every frame that came in.
        let ratio = outcome.stats.frames_out as f64 / outcome.stats.frames_in as f64;
        assert!((ratio / 1.0001 - 1.0).abs() < 2e-6, "{ratio}");
    }

    #[test]
    fn eight_hours_with_a_sound_card_that_is_100_ppm_slow() {
        let mut scenario = Scenario::quiet(8.0 * HOUR);
        scenario.device_ppm = -100.0;
        let outcome = run(&scenario);
        report("8 h, card -100 ppm", &outcome);
        assert_uneventful(&outcome);
        assert_quiet_pitch(&outcome);
        assert_wait(&outcome, 40.0, 0.5);
        assert!((outcome.stats.drift_ppm + 100.0).abs() < 2.0, "{outcome:?}");
    }

    #[test]
    fn eight_hours_with_both_clocks_off_in_opposite_directions() {
        let mut scenario = Scenario::quiet(8.0 * HOUR);
        scenario.source_ppm = -100.0;
        scenario.device_ppm = 100.0;
        let outcome = run(&scenario);
        report("8 h, source -100 ppm, card +100 ppm", &outcome);
        assert_uneventful(&outcome);
        assert_quiet_pitch(&outcome);
        assert_wait(&outcome, 40.0, 0.5);
    }

    #[test]
    fn eight_hours_on_a_44100_hz_sound_card() {
        for device_ppm in [100.0, -100.0] {
            let mut scenario = Scenario::quiet(8.0 * HOUR);
            scenario.device_rate = 44_100.0;
            scenario.device_ppm = device_ppm;
            // 512 frames at 44.1 kHz.
            scenario.callback_lead_s = 0.012;
            let outcome = run(&scenario);
            report(&format!("8 h, 44.1 kHz card {device_ppm:+} ppm"), &outcome);
            assert_uneventful(&outcome);
            assert_quiet_pitch(&outcome);
            assert_wait(&outcome, 40.0, 0.5);
            assert!(
                (outcome.stats.drift_ppm - device_ppm).abs() < 2.0,
                "{outcome:?}"
            );
            // 48000 frames in for every 44100 out.
            let ratio = outcome.stats.frames_in as f64 / outcome.stats.frames_out as f64;
            let expected = 48_000.0 / 44_100.0 / (1.0 + device_ppm * 1e-6);
            assert!((ratio / expected - 1.0).abs() < 2e-6, "{ratio} {expected}");
        }
    }

    #[test]
    fn callbacks_that_come_early_and_late() {
        // The sound card asks up to 4 ms earlier or later than usual; the
        // time stamps of its buffers are exact.
        let mut scenario = Scenario::quiet(2.0 * HOUR);
        scenario.device_ppm = 60.0;
        scenario.callback_jitter_s = 0.004;
        scenario.callback_lead_s = 0.012;
        let outcome = run(&scenario);
        report("2 h, callbacks +-4 ms", &outcome);
        assert_uneventful(&outcome);
        assert_quiet_pitch(&outcome);
        assert_wait(&outcome, 40.0, 0.5);
    }

    #[test]
    fn time_stamps_that_are_off() {
        // A sound system that cannot say exactly when a buffer is due: the
        // time stamps are off by up to a millisecond either way.
        let mut scenario = Scenario::quiet(2.0 * HOUR);
        scenario.device_ppm = -60.0;
        scenario.timestamp_noise_s = 0.001;
        let outcome = run(&scenario);
        report("2 h, time stamps +-1 ms", &outcome);
        assert_uneventful(&outcome);
        assert_pitch_bound(&outcome);
        // The longest wait of a second is what is held at the target, so
        // noise in the stamps shifts the wait by about its own size.
        assert_wait(&outcome, 40.0, 1.5);
    }

    #[test]
    fn small_callbacks_and_large_packets() {
        // 128 frames per callback (2.7 ms), packets of 1024 frames (21 ms):
        // most callbacks see no new packet at all.
        let mut scenario = Scenario::quiet(2.0 * HOUR);
        scenario.device_ppm = 100.0;
        scenario.callback_frames = 128;
        scenario.callback_lead_s = 0.003;
        scenario.packet_frames = 1024;
        scenario.target_s = 0.050;
        let outcome = run(&scenario);
        report("2 h, 128-frame callbacks, 1024-frame packets", &outcome);
        assert_uneventful(&outcome);
        assert_pitch_bound(&outcome);
        assert_wait(&outcome, 50.0, 0.5);
    }

    #[test]
    fn eight_hours_over_a_network_that_holds_packets_up() {
        // Every packet up to 5 ms late, one in 200 up to 15 ms more.
        let mut scenario = Scenario::quiet(8.0 * HOUR);
        scenario.device_ppm = 100.0;
        scenario.jitter_s = 0.005;
        scenario.spike_one_in = 200;
        scenario.spike_s = 0.015;
        // A packet may be 20 ms late, which is more than the usual wait
        // leaves room for; on such a network the wait has to be longer.
        scenario.target_s = 0.060;
        let outcome = run(&scenario);
        report("8 h, jitter 5 ms, spikes 15 ms", &outcome);
        assert_uneventful(&outcome);
        assert_pitch_bound(&outcome);
        assert_wait(&outcome, 60.0, 1.0);
    }

    #[test]
    fn stalls_the_wait_has_room_for_go_unnoticed() {
        // Every minute the network delivers nothing for 10 ms.
        let mut scenario = Scenario::quiet(2.0 * HOUR);
        scenario.device_ppm = 50.0;
        scenario.stalls = (1..120)
            .map(|minute| (f64::from(minute) * 60.0, 0.010))
            .collect();
        let outcome = run(&scenario);
        report("2 h, 10 ms stalls every minute", &outcome);
        assert_uneventful(&outcome);
        assert_quiet_pitch(&outcome);
        assert_wait(&outcome, 40.0, 0.5);
    }

    #[test]
    fn after_a_long_stall_the_latency_is_back_at_the_target() {
        // A whole second without delivery, then the backlog at once.
        let mut scenario = Scenario::quiet(1200.0);
        scenario.device_ppm = 100.0;
        scenario.stalls = vec![(600.0, 1.0)];
        // Judged from one second after the stall has ended.
        scenario.settle_s = 602.0;
        let outcome = run(&scenario);
        report("20 min, one 1 s stall", &outcome);
        // One underrun, which is what a second without audio is; no more.
        assert_eq!(outcome.stats.underruns, 1, "{outcome:?}");
        assert_eq!(outcome.stats.starts, 2, "{outcome:?}");
        assert!(
            (outcome.stats.underrun_ms - 975.0).abs() < 30.0,
            "{outcome:?}"
        );
        // The second of backlog was let go when playing began again, not
        // played late: a second after the stall the wait is where it was,
        // and it stays there.
        assert!(outcome.stats.trimmed_frames > 45_000, "{outcome:?}");
        assert_eq!(outcome.underruns_after_settling, 0);
        assert_eq!(outcome.skips_after_settling, 0);
        assert_wait(&outcome, 40.0, 0.5);
        // What was learnt about the clocks survived the interruption, and
        // playing began again at the right wait: the pitch has nothing to
        // make up for.
        assert_pitch_bound(&outcome);
    }

    #[test]
    fn a_sound_card_that_warms_up_is_followed() {
        // The clock of the card moves from +20 to +45 ppm in steps of
        // 5 ppm, one every ten minutes.
        let mut scenario = Scenario::quiet(2.0 * HOUR);
        scenario.device_ppm = 20.0;
        scenario.device_ppm_changes = (1..=5)
            .map(|step| (f64::from(step) * 600.0, 20.0 + f64::from(step) * 5.0))
            .collect();
        scenario.settle_s = 4000.0;
        let outcome = run(&scenario);
        report("2 h, card drifting from +20 to +45 ppm", &outcome);
        assert_uneventful(&outcome);
        assert_quiet_pitch(&outcome);
        assert!((outcome.stats.drift_ppm - 45.0).abs() < 2.0, "{outcome:?}");
        assert_wait(&outcome, 40.0, 0.5);
    }

    #[test]
    fn the_correction_is_learnt_within_minutes() {
        let mut scenario = Scenario::quiet(600.0);
        scenario.device_ppm = 100.0;
        // Judge the wait from the very beginning.
        scenario.settle_s = 0.0;
        let outcome = run(&scenario);
        let by_minute = &outcome.correction_ppm_by_minute;
        println!("correction by minute: {by_minute:.1?}");
        // Most of it within the first minute, all but a few ppm after five.
        assert!(by_minute[0] > 45.0, "{by_minute:?}");
        assert!((by_minute[2] - 100.0).abs() < 12.0, "{by_minute:?}");
        assert!((by_minute[4] - 100.0).abs() < 3.0, "{by_minute:?}");
        // The wait never left the target by more than two milliseconds on
        // the way, although nothing was known about the clocks at first.
        let stats = &outcome.stats;
        assert!(stats.wait_min_ms.unwrap() > 38.0, "{stats:?}");
        assert!(stats.wait_max_ms.unwrap() < 42.0, "{stats:?}");
    }

    /// Runs real audio through and looks at what the sound card was given.
    fn tone_through(scenario: &Scenario) -> (Outcome, crate::probe::ToneReport) {
        let outcome = run_with_tone(scenario, 997.0).unwrap();
        let report = analyze_tone(&outcome.played, scenario.device_rate, 997.0).unwrap();
        println!(
            "tone: level {:.2} dBFS, pitch {:+.2} ppm ({:+.2}..{:+.2}), residual {:.1} dB, \
             dropouts {}, clicks {}",
            report.level_dbfs.unwrap_or(f64::NAN),
            report.pitch_ppm_mean,
            report.pitch_ppm_min,
            report.pitch_ppm_max,
            report.residual_db,
            report.dropouts,
            report.clicks
        );
        (outcome, report)
    }

    #[test]
    fn a_tone_comes_out_as_the_same_tone() {
        // Two minutes of a sine through the resampler that is used for
        // sound, with clocks 100 ppm apart and a network that holds packets
        // up.
        let mut scenario = Scenario::quiet(120.0);
        scenario.device_ppm = 100.0;
        scenario.jitter_s = 0.003;
        let (outcome, report) = tone_through(&scenario);
        assert_uneventful(&outcome);
        assert_eq!(report.dropouts, 0, "{report:?}");
        assert_eq!(report.clicks, 0, "{report:?}");
        assert!(
            (report.level_dbfs.unwrap() + 12.04).abs() < 0.05,
            "{report:?}"
        );
        // Nothing but the tone, to one part in ten thousand.
        assert!(report.residual_db < -80.0, "{report:?}");
        // The pitch of the recording moves with the correction: from 0 to
        // -100 ppm, as the card that plays it is 100 ppm fast, and on the
        // way there a little beyond. It glides; it never jumps.
        assert!(report.pitch_ppm_max < 30.0, "{report:?}");
        assert!(report.pitch_ppm_min > -130.0, "{report:?}");
    }

    #[test]
    fn a_tone_comes_out_as_the_same_tone_at_44100_hz() {
        let mut scenario = Scenario::quiet(60.0);
        scenario.device_rate = 44_100.0;
        scenario.device_ppm = -100.0;
        scenario.callback_lead_s = 0.012;
        let (outcome, report) = tone_through(&scenario);
        assert_uneventful(&outcome);
        assert_eq!((report.dropouts, report.clicks), (0, 0), "{report:?}");
        assert!(
            (report.level_dbfs.unwrap() + 12.04).abs() < 0.05,
            "{report:?}"
        );
        assert!(report.residual_db < -80.0, "{report:?}");
        assert!(report.pitch_ppm_min > -30.0, "{report:?}");
        assert!(report.pitch_ppm_max < 130.0, "{report:?}");
    }

    #[test]
    fn an_underrun_and_what_follows_leave_a_gap_but_no_click() {
        let mut scenario = Scenario::quiet(30.0);
        scenario.stalls = vec![(15.0, 0.3)];
        let (outcome, report) = tone_through(&scenario);
        assert_eq!(outcome.stats.underruns, 1, "{outcome:?}");
        // The silence is there, and it begins and ends softly.
        assert_eq!(report.dropouts, 1, "{report:?}");
        assert!((report.dropout_ms - 280.0).abs() < 30.0, "{report:?}");
        assert_eq!(report.clicks, 0, "{report:?}");
    }

    #[test]
    fn the_same_scenario_gives_the_same_run() {
        let mut scenario = Scenario::quiet(300.0);
        scenario.jitter_s = 0.004;
        scenario.callback_jitter_s = 0.002;
        scenario.settle_s = 100.0;
        let first = run(&scenario);
        let second = run(&scenario);
        assert_eq!(first, second);
        scenario.seed = 2;
        let other = run(&scenario);
        assert_ne!(first.pitch_error_ppm, other.pitch_error_ppm);
    }
}
