//! A test tone, and what can be told from a recording of it.
//!
//! A steady sine is the easiest signal to check after it has been through
//! a resampler whose ratio moves: samples cannot be compared one by one any
//! more, but a sine that comes out as a sine of the same pitch and level,
//! without a jump and without a dip, has not been damaged.

use std::f64::consts::TAU;

use serde::Serialize;

/// A sine generator. The phase carries on from one call to the next.
#[derive(Debug, Clone)]
pub struct Tone {
    phase: f64,
    step: f64,
    amplitude: f64,
}

impl Tone {
    /// `amplitude` is the peak, 1.0 being full scale.
    pub fn new(freq_hz: f64, amplitude: f32, rate: f64) -> Self {
        Tone {
            phase: 0.0,
            step: TAU * freq_hz / rate,
            amplitude: f64::from(amplitude),
        }
    }

    /// Fills `out` with the next frames; every channel gets the same.
    pub fn fill(&mut self, out: &mut [f32], channels: usize) {
        for frame in out.chunks_exact_mut(channels.max(1)) {
            let value = (self.amplitude * self.phase.sin()) as f32;
            frame.fill(value);
            self.phase += self.step;
            if self.phase >= TAU {
                self.phase -= TAU;
            }
        }
    }
}

/// What a recording of the test tone shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToneReport {
    pub frames: usize,
    pub seconds: f64,
    /// Level of the tone, in dB relative to full scale. `None` when no tone
    /// was found.
    pub level_dbfs: Option<f64>,
    /// How far the pitch is from the nominal one, in parts per million:
    /// mean, lowest and highest over tenths of a second.
    pub pitch_ppm_mean: f64,
    pub pitch_ppm_min: f64,
    pub pitch_ppm_max: f64,
    /// Everything that is not the tone, relative to the tone, in dB (the
    /// median over tenths of a second).
    pub residual_db: f64,
    /// Stretches where the level fell to less than half: fades, silence.
    pub dropouts: usize,
    pub dropout_ms: f64,
    /// Places where the waveform jumps.
    pub clicks: usize,
}

/// A sum of the signal against a turning pointer of the tone's frequency,
/// under a window that is zero at both ends (Hann).
fn correlate(samples: &[f32], first_index: usize, omega: f64) -> (f64, f64, f64) {
    let n = samples.len() as f64;
    let (mut re, mut im, mut power, mut weight, mut weight2) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (i, sample) in samples.iter().enumerate() {
        let window = 0.5 - 0.5 * (TAU * (i as f64 + 0.5) / n).cos();
        let angle = omega * (first_index + i) as f64;
        let value = f64::from(*sample);
        re += window * value * angle.cos();
        im -= window * value * angle.sin();
        power += window * window * value * value;
        weight += window;
        weight2 += window * window;
    }
    // Amplitude and phase of the tone, and the mean power under the window.
    let amplitude = 2.0 * (re * re + im * im).sqrt() / weight;
    (amplitude, im.atan2(re), power / weight2)
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// Looks at a recording of one channel of the test tone. `rate` is the
/// rate the recording is played at, `freq_hz` the pitch the tone was made
/// with. Returns `None` when the recording is too short to say anything.
///
/// Silence before the tone begins and after it ends is not held against it;
/// everything in between is.
pub fn analyze_tone(samples: &[f32], rate: f64, freq_hz: f64) -> Option<ToneReport> {
    let omega = TAU * freq_hz / rate;
    let short = (rate * 0.005) as usize;
    let long = (rate * 0.1) as usize;
    if samples.len() < 4 * long || short < 8 {
        return None;
    }

    // The level in steps of 2.5 ms, to find where the tone is and where it
    // dips.
    let hop = short / 2;
    let levels: Vec<f64> = (0..(samples.len() - short) / hop)
        .map(|w| correlate(&samples[w * hop..w * hop + short], w * hop, omega).0)
        .collect();
    // The level of the tone is that of the loudest tenth of the recording:
    // most of a recording may be silence before the tone begins.
    let mut sorted = levels.clone();
    sorted.sort_by(f64::total_cmp);
    let typical = sorted[sorted.len() * 9 / 10];
    let silent = ToneReport {
        frames: samples.len(),
        seconds: samples.len() as f64 / rate,
        level_dbfs: None,
        pitch_ppm_mean: 0.0,
        pitch_ppm_min: 0.0,
        pitch_ppm_max: 0.0,
        residual_db: 0.0,
        dropouts: 0,
        dropout_ms: 0.0,
        clicks: 0,
    };
    if typical < 1e-5 {
        return Some(silent);
    }
    let present = |level: f64| level >= 0.5 * typical;
    let (Some(first), Some(last)) = (
        levels.iter().position(|level| present(*level)),
        levels.iter().rposition(|level| present(*level)),
    ) else {
        return Some(silent);
    };

    let mut dropouts = 0;
    let mut dropout_windows = 0;
    let mut inside = false;
    for level in &levels[first..=last] {
        if present(*level) {
            inside = false;
        } else {
            dropout_windows += 1;
            if !inside {
                dropouts += 1;
            }
            inside = true;
        }
    }

    // Pitch in tenths of a second, over the part where the tone is at its
    // level throughout: how far its phase runs ahead of a pointer that
    // turns at the nominal pitch.
    let begin = first * hop + short;
    let end = (last * hop).min(samples.len());
    let mut phases: Vec<(usize, f64)> = Vec::new();
    let mut at = begin;
    while at + long <= end {
        let steady = levels[at / hop..((at + long) / hop).min(levels.len())]
            .iter()
            .all(|level| present(*level));
        if steady {
            phases.push((at, correlate(&samples[at..at + long], at, omega).1));
        }
        at += long;
    }
    // One pitch for every pair of neighbouring tenths, kept with the place
    // of the second one.
    let mut pitches: Vec<(usize, f64)> = Vec::new();
    for pair in phases.windows(2) {
        let ((before_at, before), (at, phase)) = (pair[0], pair[1]);
        if at != before_at + long {
            continue;
        }
        let mut turn = phase - before;
        if turn > TAU / 2.0 {
            turn -= TAU;
        } else if turn < -TAU / 2.0 {
            turn += TAU;
        }
        pitches.push((at, turn / (omega * long as f64) * 1e6));
    }
    let pitch_mean = if pitches.is_empty() {
        0.0
    } else {
        pitches.iter().map(|(_, pitch)| pitch).sum::<f64>() / pitches.len() as f64
    };

    // Purity: what is left of each tenth once the tone, at the pitch it
    // really has there, is taken away.
    let mut residuals = Vec::new();
    for (at, _) in &phases {
        let local = pitches
            .iter()
            .min_by_key(|(place, _)| place.abs_diff(*at))
            .map_or(pitch_mean, |(_, pitch)| *pitch);
        let window = &samples[*at..*at + long];
        let (amplitude, _, power) = correlate(window, *at, omega * (1.0 + local * 1e-6));
        let tone_power = amplitude * amplitude / 2.0;
        let rest = (power - tone_power).max(tone_power * 1e-14);
        residuals.push(10.0 * (rest / tone_power).log10());
    }

    // Jumps: three neighbouring samples of a sine obey
    // x[n-1] + x[n+1] = 2 cos(w) x[n]; a jump breaks that by its own size.
    // Fades do not: they change the level by a fraction of a percent per
    // sample.
    let two_cos = 2.0 * (omega * (1.0 + pitch_mean * 1e-6)).cos();
    let threshold = 0.02 * typical;
    let quiet_gap = (rate * 0.005) as usize;
    let mut clicks = 0;
    let mut last_click: Option<usize> = None;
    let from = (first * hop).max(1);
    let to = (last * hop + short).min(samples.len() - 1);
    for n in from..to {
        let broken =
            f64::from(samples[n - 1]) + f64::from(samples[n + 1]) - two_cos * f64::from(samples[n]);
        if broken.abs() > threshold {
            if last_click.is_none_or(|at| n - at > quiet_gap) {
                clicks += 1;
            }
            last_click = Some(n);
        }
    }

    let (pitch_min, pitch_max) = pitches.iter().fold(
        (f64::INFINITY, f64::NEG_INFINITY),
        |(low, high), (_, pitch)| (low.min(*pitch), high.max(*pitch)),
    );
    Some(ToneReport {
        frames: samples.len(),
        seconds: samples.len() as f64 / rate,
        level_dbfs: Some(20.0 * typical.log10()),
        pitch_ppm_mean: pitch_mean,
        pitch_ppm_min: if pitches.is_empty() { 0.0 } else { pitch_min },
        pitch_ppm_max: if pitches.is_empty() { 0.0 } else { pitch_max },
        residual_db: if residuals.is_empty() {
            0.0
        } else {
            median(&mut residuals)
        },
        dropouts,
        dropout_ms: dropout_windows as f64 * hop as f64 / rate * 1e3,
        clicks,
    })
}

/// What a long recording of the test tone showed, looked at as it was made.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToneSummary {
    pub seconds: f64,
    /// Pieces the recording was looked at in, and how many of them held no
    /// tone at all.
    pub pieces: usize,
    pub pieces_without_tone: usize,
    /// Lowest and highest level of the tone over the pieces, in dB relative
    /// to full scale. `None` when no piece held the tone.
    pub level_dbfs_min: Option<f64>,
    pub level_dbfs_max: Option<f64>,
    /// How far the pitch was from the nominal one, in parts per million.
    pub pitch_ppm_mean: f64,
    pub pitch_ppm_min: f64,
    pub pitch_ppm_max: f64,
    /// The worst piece's residual: everything that is not the tone,
    /// relative to the tone, in dB.
    pub residual_db: f64,
    /// Stretches where the level fell to less than half, once the tone had
    /// begun. A stretch that lasts until the recording ends is the end of
    /// the tone, not a dropout.
    pub dropouts: usize,
    pub dropout_ms: f64,
    /// Places where the waveform jumps. Pieces overlap a little, so a jump
    /// near the border of two pieces may be counted in both.
    pub clicks: usize,
}

/// Looks at a recording of the test tone while it is being made, so that
/// hours of it need not be kept.
///
/// Pitch, purity and jumps are read off pieces of the recording
/// ([`analyze_tone`]); the pieces overlap so that nothing falls between
/// them. Dropouts are found in the level of the last few milliseconds as
/// the samples go by, which knows no pieces.
pub struct ToneWatch {
    rate: f64,
    freq_hz: f64,
    piece_frames: usize,
    overlap_frames: usize,
    piece: Vec<f32>,
    frames: u64,
    /// The last 5 ms, looked at every 2.5 ms.
    recent: Vec<f32>,
    recent_at: usize,
    until_look: usize,
    /// The frame at which the tone was first there.
    onset: Option<u64>,
    /// The level the tone has (root mean square), once it is known.
    reference: f64,
    settled: bool,
    quiet: bool,
    quiet_looks: usize,
    open_looks: usize,
    pitch_sum: f64,
    pitch_weight: f64,
    summary: ToneSummary,
}

/// Below this level (root mean square) there is no tone.
const NO_TONE: f64 = 7e-6;

impl ToneWatch {
    /// Looks at the recording in pieces of `piece_s` seconds.
    pub fn new(rate: f64, freq_hz: f64, piece_s: f64) -> Self {
        let piece_frames = ((piece_s * rate) as usize).max((rate * 1.0) as usize);
        let hop = ((rate * 0.0025) as usize).max(1);
        ToneWatch {
            rate,
            freq_hz,
            piece_frames,
            overlap_frames: ((rate * 1.0) as usize).min(piece_frames / 4),
            piece: Vec::with_capacity(piece_frames),
            frames: 0,
            recent: vec![0.0; 2 * hop],
            recent_at: 0,
            until_look: hop,
            onset: None,
            reference: 0.0,
            settled: false,
            quiet: false,
            quiet_looks: 0,
            open_looks: 0,
            pitch_sum: 0.0,
            pitch_weight: 0.0,
            summary: ToneSummary {
                seconds: 0.0,
                pieces: 0,
                pieces_without_tone: 0,
                level_dbfs_min: None,
                level_dbfs_max: None,
                pitch_ppm_mean: 0.0,
                pitch_ppm_min: 0.0,
                pitch_ppm_max: 0.0,
                residual_db: f64::NEG_INFINITY,
                dropouts: 0,
                dropout_ms: 0.0,
                clicks: 0,
            },
        }
    }

    /// Takes the next sample. When that completes a piece, what the piece
    /// showed is returned.
    pub fn push(&mut self, sample: f32) -> Option<ToneReport> {
        self.frames += 1;
        self.piece.push(sample);
        self.recent[self.recent_at] = sample;
        self.recent_at = (self.recent_at + 1) % self.recent.len();
        self.until_look -= 1;
        if self.until_look == 0 {
            self.until_look = self.recent.len() / 2;
            self.look();
        }
        if self.piece.len() < self.piece_frames {
            return None;
        }
        let report = analyze_tone(&self.piece, self.rate, self.freq_hz);
        // The end of this piece is the beginning of the next.
        let keep_from = self.piece.len() - self.overlap_frames;
        self.piece.drain(..keep_from);
        if let Some(report) = &report {
            self.fold(report);
        }
        report
    }

    /// The level of the last 5 ms against the level the tone has.
    fn look(&mut self) {
        if self.frames < self.recent.len() as u64 {
            return;
        }
        let power: f64 = self.recent.iter().map(|s| f64::from(*s).powi(2)).sum();
        let level = (power / self.recent.len() as f64).sqrt();
        match self.onset {
            None => {
                if level > NO_TONE {
                    self.onset = Some(self.frames);
                    self.reference = level;
                }
            }
            // The tone fades in; half a second later its level is known.
            Some(onset) if !self.settled => {
                self.reference = self.reference.max(level);
                self.settled = self.frames - onset >= (self.rate * 0.5) as u64;
            }
            Some(_) => {
                if level < 0.5 * self.reference {
                    if !self.quiet {
                        self.summary.dropouts += 1;
                        self.open_looks = 0;
                    }
                    self.quiet = true;
                    self.quiet_looks += 1;
                    self.open_looks += 1;
                } else {
                    self.quiet = false;
                }
            }
        }
    }

    fn fold(&mut self, report: &ToneReport) {
        let summary = &mut self.summary;
        summary.pieces += 1;
        let Some(level) = report.level_dbfs else {
            summary.pieces_without_tone += 1;
            return;
        };
        let first = summary.level_dbfs_min.is_none();
        summary.level_dbfs_min = Some(summary.level_dbfs_min.map_or(level, |low| low.min(level)));
        summary.level_dbfs_max = Some(summary.level_dbfs_max.map_or(level, |high| high.max(level)));
        if first {
            summary.pitch_ppm_min = report.pitch_ppm_min;
            summary.pitch_ppm_max = report.pitch_ppm_max;
        } else {
            summary.pitch_ppm_min = summary.pitch_ppm_min.min(report.pitch_ppm_min);
            summary.pitch_ppm_max = summary.pitch_ppm_max.max(report.pitch_ppm_max);
        }
        summary.residual_db = summary.residual_db.max(report.residual_db);
        summary.clicks += report.clicks;
        self.pitch_sum += report.pitch_ppm_mean * report.seconds;
        self.pitch_weight += report.seconds;
        // The level the pieces find is steadier than the one found at the
        // tone's beginning.
        if self.settled {
            self.reference = 10f64.powf(level / 20.0) / std::f64::consts::SQRT_2;
        }
    }

    /// Looks at what is left of the recording and returns the whole.
    pub fn finish(mut self) -> ToneSummary {
        // Only when something came after the last whole piece.
        if self.summary.pieces == 0 || self.piece.len() > self.overlap_frames {
            if let Some(report) = analyze_tone(&self.piece, self.rate, self.freq_hz) {
                self.fold(&report);
            }
        }
        if self.quiet {
            self.summary.dropouts -= 1;
            self.quiet_looks -= self.open_looks;
        }
        let hop = self.recent.len() / 2;
        let mut summary = self.summary;
        summary.seconds = self.frames as f64 / self.rate;
        summary.dropout_ms = self.quiet_looks as f64 * hop as f64 / self.rate * 1e3;
        if self.pitch_weight > 0.0 {
            summary.pitch_ppm_mean = self.pitch_sum / self.pitch_weight;
        }
        if summary.residual_db == f64::NEG_INFINITY {
            summary.residual_db = 0.0;
        }
        summary
    }
}

/// The spread of a set of measurements.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Spread {
    pub count: usize,
    pub min: f64,
    pub median: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

impl Spread {
    /// Sorts `values` and reads the spread off them. `None` for no values.
    pub fn of(values: &mut [f64]) -> Option<Spread> {
        if values.is_empty() {
            return None;
        }
        values.sort_by(f64::total_cmp);
        // The smallest value that at least this share of the values does
        // not exceed.
        let rank = |share: f64| {
            let index = (share * values.len() as f64).ceil() as usize;
            values[index.clamp(1, values.len()) - 1]
        };
        Some(Spread {
            count: values.len(),
            min: values[0],
            median: rank(0.5),
            p95: rank(0.95),
            p99: rank(0.99),
            max: values[values.len() - 1],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48_000.0;

    fn tone(freq: f64, amplitude: f32, seconds: f64) -> Vec<f32> {
        let mut samples = vec![0.0; (seconds * RATE) as usize];
        Tone::new(freq, amplitude, RATE).fill(&mut samples, 1);
        samples
    }

    #[test]
    fn the_generator_makes_the_tone_it_is_asked_for() {
        let mut left_right = vec![0.0f32; 96];
        let mut generator = Tone::new(1000.0, 0.5, RATE);
        generator.fill(&mut left_right, 2);
        assert_eq!(left_right[0], 0.0);
        // A quarter of a period later it is at its peak, on both channels.
        assert!((left_right[24] - 0.5).abs() < 1e-6);
        assert_eq!(left_right[24], left_right[25]);
        // The next call carries on where this one stopped.
        let mut more = vec![0.0f32; 2];
        generator.fill(&mut more, 2);
        assert!(more[0].abs() < 1e-5);
    }

    #[test]
    fn a_clean_tone_is_reported_clean() {
        let report = analyze_tone(&tone(997.0, 0.25, 3.0), RATE, 997.0).unwrap();
        assert!(
            (report.level_dbfs.unwrap() + 12.04).abs() < 0.05,
            "{report:?}"
        );
        assert!(report.pitch_ppm_mean.abs() < 0.01, "{report:?}");
        assert!(
            report.pitch_ppm_max - report.pitch_ppm_min < 0.05,
            "{report:?}"
        );
        assert!(report.residual_db < -90.0, "{report:?}");
        assert_eq!((report.dropouts, report.clicks), (0, 0));
    }

    #[test]
    fn a_pitch_that_is_off_is_measured_in_parts_per_million() {
        for ppm in [100.0, -100.0, 3.0] {
            let played = tone(997.0 * (1.0 + ppm * 1e-6), 0.25, 3.0);
            let report = analyze_tone(&played, RATE, 997.0).unwrap();
            assert!(
                (report.pitch_ppm_mean - ppm).abs() < 0.05,
                "{ppm}: {report:?}"
            );
            assert_eq!((report.dropouts, report.clicks), (0, 0));
        }
    }

    #[test]
    fn silence_around_the_tone_is_not_held_against_it() {
        let mut samples = vec![0.0f32; 4800];
        samples.extend(tone(997.0, 0.25, 2.0));
        samples.extend(vec![0.0f32; 9600]);
        let report = analyze_tone(&samples, RATE, 997.0).unwrap();
        assert_eq!(report.dropouts, 0, "{report:?}");
        assert!(report.pitch_ppm_mean.abs() < 0.05, "{report:?}");
    }

    #[test]
    fn a_gap_is_a_dropout_and_its_length_is_measured() {
        let mut samples = tone(997.0, 0.25, 3.0);
        // 20 ms of silence in the middle, with soft edges of 3 ms.
        for (i, sample) in samples[48_000..48_960].iter_mut().enumerate() {
            let edge = (i.min(959 - i) as f32 / 144.0).min(1.0);
            *sample *= 1.0 - edge;
        }
        let report = analyze_tone(&samples, RATE, 997.0).unwrap();
        assert_eq!(report.dropouts, 1, "{report:?}");
        assert!((report.dropout_ms - 20.0).abs() < 6.0, "{report:?}");
        // Soft edges are not jumps.
        assert_eq!(report.clicks, 0, "{report:?}");
    }

    #[test]
    fn a_jump_in_the_waveform_is_a_click() {
        // A hundred samples are missing: the wave carries on out of step.
        let mut samples = tone(997.0, 0.25, 1.5);
        let rest = tone(997.0, 0.25, 3.0);
        samples.extend_from_slice(&rest[72_100..]);
        let report = analyze_tone(&samples, RATE, 997.0).unwrap();
        assert_eq!(report.clicks, 1, "{report:?}");
        assert_eq!(report.dropouts, 0, "{report:?}");
    }

    #[test]
    fn noise_shows_in_the_residual() {
        let mut samples = tone(997.0, 0.25, 3.0);
        // A second, much quieter tone 60 dB down.
        let other = tone(3001.0, 0.00025, 3.0);
        for (sample, add) in samples.iter_mut().zip(&other) {
            *sample += *add;
        }
        let report = analyze_tone(&samples, RATE, 997.0).unwrap();
        assert!((report.residual_db + 60.0).abs() < 1.0, "{report:?}");
        assert_eq!(report.clicks, 0);
    }

    #[test]
    fn a_recording_without_the_tone_says_so() {
        let report = analyze_tone(&vec![0.0; 48_000], RATE, 997.0).unwrap();
        assert_eq!(report.level_dbfs, None);
        assert!(analyze_tone(&[0.0; 100], RATE, 997.0).is_none());
    }

    fn watch(samples: &[f32], piece_s: f64) -> (ToneSummary, usize) {
        let mut watch = ToneWatch::new(RATE, 997.0, piece_s);
        let pieces = samples.iter().filter_map(|s| watch.push(*s)).count();
        (watch.finish(), pieces)
    }

    #[test]
    fn a_long_clean_tone_is_clean_piece_by_piece() {
        // Silence before the tone begins and a fade into it, as when a
        // playout starts.
        let mut samples = vec![0.0f32; 9_600];
        let mut played = tone(997.0, 0.001, 9.8);
        for (i, sample) in played[..144].iter_mut().enumerate() {
            *sample *= i as f32 / 144.0;
        }
        samples.extend(played);
        let (summary, pieces) = watch(&samples, 2.0);
        // Pieces of two seconds that overlap by half a second: six whole
        // ones, and the second that is left at the end.
        assert_eq!(pieces, 6);
        assert_eq!(summary.pieces, 7, "{summary:?}");
        assert_eq!(summary.pieces_without_tone, 0);
        assert!((summary.seconds - 10.0).abs() < 1e-9);
        assert!(
            (summary.level_dbfs_min.unwrap() + 60.0).abs() < 0.05,
            "{summary:?}"
        );
        assert!(
            (summary.level_dbfs_max.unwrap() + 60.0).abs() < 0.05,
            "{summary:?}"
        );
        assert!(summary.pitch_ppm_mean.abs() < 0.05, "{summary:?}");
        assert!(
            summary.pitch_ppm_max - summary.pitch_ppm_min < 0.2,
            "{summary:?}"
        );
        assert!(summary.residual_db < -80.0, "{summary:?}");
        assert_eq!((summary.dropouts, summary.clicks), (0, 0), "{summary:?}");
    }

    #[test]
    fn a_gap_across_the_border_of_two_pieces_is_one_dropout() {
        let mut samples = tone(997.0, 0.25, 7.0);
        // 100 ms of silence around the end of the first piece, soft edges.
        let (from, to) = (93_600, 98_400);
        for (i, sample) in samples[from..to].iter_mut().enumerate() {
            let edge = (i.min(to - from - 1 - i) as f32 / 144.0).min(1.0);
            *sample *= 1.0 - edge;
        }
        let (summary, _) = watch(&samples, 2.0);
        assert_eq!(summary.dropouts, 1, "{summary:?}");
        assert!((summary.dropout_ms - 100.0).abs() < 8.0, "{summary:?}");
        assert_eq!(summary.clicks, 0, "{summary:?}");
    }

    #[test]
    fn a_short_dip_is_found_and_a_tone_that_ends_is_not_a_dropout() {
        let mut samples = tone(997.0, 0.25, 5.0);
        // 7.5 ms, as an underrun with its fades leaves.
        for (i, sample) in samples[120_000..120_360].iter_mut().enumerate() {
            let edge = (i.min(359 - i) as f32 / 144.0).min(1.0);
            *sample *= 1.0 - edge;
        }
        // The tone ends half a second before the recording does.
        samples.extend(vec![0.0f32; 24_000]);
        let (summary, _) = watch(&samples, 2.0);
        assert_eq!(summary.dropouts, 1, "{summary:?}");
        assert!(
            summary.dropout_ms > 2.0 && summary.dropout_ms < 12.0,
            "{summary:?}"
        );
    }

    #[test]
    fn a_jump_is_counted() {
        // A hundred samples are missing 3.3 s in: the wave carries on out
        // of step. That place lies in two pieces.
        let mut samples = tone(997.0, 0.25, 3.3);
        let rest = tone(997.0, 0.25, 8.0);
        samples.extend_from_slice(&rest[158_500..]);
        let (summary, _) = watch(&samples, 2.0);
        assert!((1..=2).contains(&summary.clicks), "{summary:?}");
        assert_eq!(summary.dropouts, 0, "{summary:?}");
    }

    #[test]
    fn the_pitch_is_followed_across_pieces() {
        let played = tone(997.0 * (1.0 + 80e-6), 0.25, 6.0);
        let (summary, _) = watch(&played, 2.0);
        assert!((summary.pitch_ppm_mean - 80.0).abs() < 0.1, "{summary:?}");
        assert!(
            summary.pitch_ppm_min > 79.5 && summary.pitch_ppm_max < 80.5,
            "{summary:?}"
        );
        assert_eq!((summary.dropouts, summary.clicks), (0, 0), "{summary:?}");
    }

    #[test]
    fn a_recording_without_the_tone_has_no_level() {
        let (summary, _) = watch(&vec![0.0f32; 144_000], 2.0);
        assert_eq!(summary.level_dbfs_min, None);
        assert_eq!(summary.pieces_without_tone, summary.pieces);
        assert_eq!((summary.dropouts, summary.clicks), (0, 0));
    }

    #[test]
    fn the_spread_is_read_off_sorted_values() {
        assert_eq!(Spread::of(&mut []), None);
        let mut values: Vec<f64> = (1..=100).rev().map(f64::from).collect();
        let spread = Spread::of(&mut values).unwrap();
        assert_eq!(spread.count, 100);
        assert_eq!((spread.min, spread.max), (1.0, 100.0));
        assert_eq!((spread.median, spread.p95, spread.p99), (50.0, 95.0, 99.0));
        let spread = Spread::of(&mut [7.0]).unwrap();
        assert_eq!((spread.median, spread.p99, spread.max), (7.0, 7.0, 7.0));
    }
}
