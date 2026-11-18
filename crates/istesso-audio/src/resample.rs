//! Changes the rate of the audio by an amount that can be moved while it
//! plays.
//!
//! The playout asks for output in pieces of a fixed number of frames and
//! feeds as many input frames as each piece needs. [`Sinc`] does the real
//! work with the `rubato` crate. [`Counting`] only keeps the books; with it
//! the playout can be run for hours of simulated time in a moment.

use audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Adjustable, Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

/// A converter from the stream's rate to the sound card's.
pub trait Resample: Send {
    /// Frames each call of [`Resample::process`] writes.
    fn chunk_frames(&self) -> usize;

    /// Frames the next call of [`Resample::process`] reads. Changes when the
    /// ratio is set.
    fn input_frames_next(&self) -> usize;

    /// Upper bound of [`Resample::input_frames_next`].
    fn input_frames_max(&self) -> usize;

    /// Sets the ratio of output to input rate, as a multiple of the nominal
    /// ratio. The change is spread over the next piece, so that the pitch
    /// glides and does not step.
    fn set_relative_ratio(&mut self, relative: f64);

    /// Reads `input_frames_next()` frames and writes `chunk_frames()`
    /// frames, both with the channels interleaved. Returns `false` when the
    /// buffers had the wrong size; the output is then silence.
    fn process(&mut self, input: &[f32], output: &mut [f32]) -> bool;

    /// How many input frames have gone in without having come out yet, on
    /// average.
    fn latency_input_frames(&self) -> f64;

    /// Forgets the audio inside and goes back to the nominal ratio.
    fn reset(&mut self);
}

/// Length of the interpolation filter, in input frames. Half of it is the
/// delay through the resampler (1.3 ms at 48 kHz).
const SINC_LEN: usize = 128;

/// The resampler that is used for sound: band-limited interpolation.
pub struct Sinc {
    inner: Async<f32>,
    channels: usize,
    chunk: usize,
    max_relative: f64,
}

impl Sinc {
    /// `chunk` is the number of output frames per call; `max_relative` is
    /// how far the ratio may be moved from the nominal one (1.01 allows one
    /// percent either way).
    pub fn new(
        stream_rate: f64,
        device_rate: f64,
        channels: usize,
        chunk: usize,
        max_relative: f64,
    ) -> Result<Self, String> {
        let parameters =
            SincInterpolationParameters::new(SINC_LEN, WindowFunction::BlackmanHarris2)
                .oversampling_factor(256)
                .interpolation(SincInterpolationType::Cubic);
        let inner = Async::<f32>::new_sinc(
            device_rate / stream_rate,
            max_relative,
            &parameters,
            chunk,
            channels,
            FixedAsync::Output,
        )
        .map_err(|err| format!("cannot set up the resampler: {err}"))?;
        Ok(Sinc {
            inner,
            channels,
            chunk,
            max_relative,
        })
    }
}

impl Resample for Sinc {
    fn chunk_frames(&self) -> usize {
        self.chunk
    }

    fn input_frames_next(&self) -> usize {
        self.inner.input_frames_next()
    }

    fn input_frames_max(&self) -> usize {
        self.inner.input_frames_max()
    }

    fn set_relative_ratio(&mut self, relative: f64) {
        let relative = relative.clamp(1.0 / self.max_relative, self.max_relative);
        // Cannot fail: the value is inside the range given at construction.
        let _ = self.inner.set_resample_ratio_relative(relative, true);
    }

    fn process(&mut self, input: &[f32], output: &mut [f32]) -> bool {
        let frames_in = self.inner.input_frames_next();
        let done = (|| {
            let input = InterleavedSlice::new(input, self.channels, frames_in).ok()?;
            let mut output = InterleavedSlice::new_mut(output, self.channels, self.chunk).ok()?;
            self.inner
                .process_into_buffer(&input, &mut output, None)
                .ok()
        })();
        if done.is_none() {
            output.fill(0.0);
        }
        done.is_some()
    }

    fn latency_input_frames(&self) -> f64 {
        // The filter looks half its length ahead of the point it computes.
        (SINC_LEN / 2) as f64 + 1.0
    }

    fn reset(&mut self) {
        self.inner.reset();
    }
}

/// Keeps count of the frames a resampler would read, without touching any
/// audio. For tests that run the playout against simulated clocks.
pub struct Counting {
    chunk: usize,
    nominal: f64,
    /// Ratio the last piece ended with, and the one the next ends with.
    current: f64,
    target: f64,
    /// Part of an input frame that was read but not used up yet.
    carry: f64,
}

impl Counting {
    pub fn new(stream_rate: f64, device_rate: f64, chunk: usize) -> Self {
        let nominal = device_rate / stream_rate;
        Counting {
            chunk,
            nominal,
            current: nominal,
            target: nominal,
            carry: 0.0,
        }
    }

    /// Input frames the next piece uses up, as a fraction.
    fn advance(&self) -> f64 {
        // The ratio glides from `current` to `target` over the piece.
        self.chunk as f64 * 0.5 * (1.0 / self.current + 1.0 / self.target)
    }
}

impl Resample for Counting {
    fn chunk_frames(&self) -> usize {
        self.chunk
    }

    fn input_frames_next(&self) -> usize {
        (self.advance() - self.carry).ceil().max(0.0) as usize
    }

    fn input_frames_max(&self) -> usize {
        (self.chunk as f64 / self.nominal * 1.1).ceil() as usize + 2
    }

    fn set_relative_ratio(&mut self, relative: f64) {
        self.target = self.nominal * relative;
    }

    fn process(&mut self, input: &[f32], output: &mut [f32]) -> bool {
        debug_assert!(input.is_empty());
        output.fill(0.0);
        let read = self.input_frames_next() as f64;
        self.carry += read - self.advance();
        self.current = self.target;
        true
    }

    fn latency_input_frames(&self) -> f64 {
        0.5
    }

    fn reset(&mut self) {
        self.current = self.nominal;
        self.target = self.nominal;
        self.carry = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `resampler` over a sine of `freq` Hz and returns the output.
    fn convert(
        resampler: &mut Sinc,
        stream_rate: f64,
        freq: f64,
        pieces: usize,
        mut relative: impl FnMut(usize) -> f64,
    ) -> (Vec<f32>, u64) {
        let mut input = Vec::new();
        let mut output = Vec::new();
        let mut piece = vec![0.0f32; resampler.chunk_frames()];
        let mut position = 0u64;
        for n in 0..pieces {
            resampler.set_relative_ratio(relative(n));
            let need = resampler.input_frames_next();
            assert!(need <= resampler.input_frames_max());
            input.clear();
            input.extend((0..need as u64).map(|i| {
                let t = (position + i) as f64 / stream_rate;
                (0.5 * (2.0 * std::f64::consts::PI * freq * t).sin()) as f32
            }));
            position += need as u64;
            assert!(resampler.process(&input, &mut piece));
            output.extend_from_slice(&piece);
        }
        (output, position)
    }

    /// Frequency of a sine, from the number of times it crosses zero going
    /// up.
    fn frequency(samples: &[f32], rate: f64) -> f64 {
        let mut first = None;
        let mut last = 0.0;
        let mut count = 0u32;
        for (i, pair) in samples.windows(2).enumerate() {
            if pair[0] < 0.0 && pair[1] >= 0.0 {
                // Where between the two samples the line crosses zero.
                let at = i as f64 + f64::from(pair[0]) / f64::from(pair[0] - pair[1]);
                if first.is_none() {
                    first = Some(at);
                } else {
                    count += 1;
                }
                last = at;
            }
        }
        f64::from(count) * rate / (last - first.unwrap())
    }

    #[test]
    fn a_tone_keeps_its_pitch_and_level_at_the_same_rate() {
        let mut resampler = Sinc::new(48_000.0, 48_000.0, 1, 64, 1.01).unwrap();
        let (output, read) = convert(&mut resampler, 48_000.0, 997.0, 1500, |_| 1.0);
        assert_eq!(output.len(), 1500 * 64);
        // As many frames went in as came out, give or take the look-ahead.
        assert!((read as i64 - 96_000).abs() <= 2, "{read}");
        let settled = &output[4800..];
        assert!((frequency(settled, 48_000.0) - 997.0).abs() < 0.01);
        let peak = settled.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!((peak - 0.5).abs() < 0.001, "peak {peak}");
    }

    #[test]
    fn a_tone_keeps_its_pitch_on_a_44100_hz_device() {
        let mut resampler = Sinc::new(48_000.0, 44_100.0, 1, 64, 1.01).unwrap();
        let (output, read) = convert(&mut resampler, 48_000.0, 997.0, 1500, |_| 1.0);
        // 96000 output frames at 44.1 kHz are 104490 frames at 48 kHz.
        assert!((read as i64 - 104_490).abs() <= 2, "{read}");
        let settled = &output[4410..];
        assert!((frequency(settled, 44_100.0) - 997.0).abs() < 0.01);
    }

    #[test]
    fn moving_the_ratio_moves_the_pitch_by_as_much() {
        // 0.1% more output per input: the same audio lasts longer, so it
        // sounds 0.1% lower.
        let mut resampler = Sinc::new(48_000.0, 48_000.0, 1, 64, 1.01).unwrap();
        let (output, read) = convert(&mut resampler, 48_000.0, 997.0, 3000, |_| 1.001);
        let settled = &output[9600..];
        let measured = frequency(settled, 48_000.0);
        assert!((measured - 997.0 / 1.001).abs() < 0.01, "{measured}");
        let expected = 192_000.0 / 1.001;
        assert!((read as f64 - expected).abs() < 3.0, "{read} {expected}");
    }

    #[test]
    fn the_ratio_is_held_inside_its_limits() {
        let mut resampler = Sinc::new(48_000.0, 48_000.0, 1, 64, 1.01).unwrap();
        let (_, read) = convert(&mut resampler, 48_000.0, 997.0, 3000, |_| 2.0);
        // Asked for twice the output per input, given one percent.
        let expected = 192_000.0 / 1.01;
        assert!((read as f64 - expected).abs() < 3.0, "{read} {expected}");
    }

    #[test]
    fn a_wrong_buffer_gives_silence_not_a_panic() {
        let mut resampler = Sinc::new(48_000.0, 48_000.0, 2, 64, 1.01).unwrap();
        let mut output = vec![1.0f32; 64 * 2];
        assert!(!resampler.process(&[0.0; 6], &mut output));
        assert!(output.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn counting_reads_what_a_resampler_would_read() {
        for (device_rate, relative) in [(48_000.0, 1.0), (44_100.0, 1.0), (48_000.0, 1.0002)] {
            let mut sinc = Sinc::new(48_000.0, device_rate, 1, 64, 1.01).unwrap();
            let mut counting = Counting::new(48_000.0, device_rate, 64);
            let (_, read_sinc) = convert(&mut sinc, 48_000.0, 997.0, 20_000, |_| relative);
            let mut read_counting = 0u64;
            for _ in 0..20_000 {
                counting.set_relative_ratio(relative);
                read_counting += counting.input_frames_next() as u64;
                assert!(counting.process(&[], &mut []));
            }
            // Over 27 seconds the two agree to within the filter's
            // look-ahead.
            let difference = read_sinc as i64 - read_counting as i64;
            assert!(
                (0..=(SINC_LEN as i64 / 2 + 2)).contains(&difference),
                "{device_rate} {relative}: {read_sinc} {read_counting}"
            );
        }
    }
}
