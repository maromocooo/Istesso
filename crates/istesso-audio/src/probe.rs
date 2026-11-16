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
