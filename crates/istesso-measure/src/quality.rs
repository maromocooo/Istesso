//! How much of a picture survives a relay.
//!
//! Latency can be bought with picture quality: a stream squeezed harder is
//! faster to encode, send and decode. So every latency figure is recorded
//! with a quality figure next to it (ADR 0002).
//!
//! The test application's moving text is the same on every machine and in
//! every run, so nothing has to be captured to know what was sent. A
//! [`Film`] makes the frames the application shows, one after the other; the
//! same frames are made again to compare with what came out of an encoder
//! and a decoder. The comparison is the structural similarity index (SSIM)
//! of the brightness and of the two colour differences, and the peak
//! signal-to-noise ratio (PSNR) of the brightness.
//!
//! Colour differences are compared at full resolution on purpose: a codec
//! that keeps only every fourth colour sample (4:2:0) smears coloured text,
//! and that is where it shows.

use serde::Serialize;

use crate::flash::Shade;
use crate::picture::{paint_text, scrolled, Sheet};
use crate::stats::percentile;

/// The frames the test application shows with its moving text, as a tool can
/// make them again: frame `k` is the picture at `k` frame intervals after
/// the start.
pub struct Film {
    sheet: Sheet,
    width: usize,
    height: usize,
    fps: u32,
    scroll: u32,
    toggle_every: u64,
}

impl Film {
    /// `scale` is the size of a font pixel in window pixels (2 on a display
    /// that is not scaled), `scroll` the speed of the text in rows a second.
    /// The shade in the middle switches every `toggle_every` frames, as it
    /// does when inputs arrive; 0 leaves it dark.
    pub fn new(
        width: usize,
        height: usize,
        scale: usize,
        fps: u32,
        scroll: u32,
        toggle_every: u64,
    ) -> Film {
        Film {
            sheet: Sheet::new(width, height, scale),
            width,
            height,
            fps: fps.max(1),
            scroll,
            toggle_every,
        }
    }

    /// Bytes in a frame: four per pixel, blue, green, red and one unused
    /// (GStreamer's `BGRx`, softbuffer's layout in memory).
    pub fn frame_len(&self) -> usize {
        self.width * self.height * 4
    }

    /// Paints frame `number` into `frame`, which holds [`Film::frame_len`] bytes.
    pub fn paint(&self, number: u64, frame: &mut [u8]) {
        let elapsed_ns = number * 1_000_000_000 / u64::from(self.fps);
        let offset = scrolled(elapsed_ns, self.scroll, self.sheet.height());
        let light = self.toggle_every > 0 && (number / self.toggle_every) % 2 == 1;
        let shade = if light { Shade::Light } else { Shade::Dark };
        let mut pixels = vec![0u32; self.width * self.height];
        paint_text(
            &mut pixels,
            self.width,
            self.height,
            &self.sheet,
            offset,
            shade,
        );
        for (bytes, pixel) in frame.chunks_exact_mut(4).zip(&pixels) {
            bytes.copy_from_slice(&pixel.to_le_bytes());
        }
    }
}

/// A picture as brightness and two colour differences (BT.709, full range),
/// each at full resolution.
pub struct Planes {
    pub luma: Vec<u8>,
    pub blue: Vec<u8>,
    pub red: Vec<u8>,
}

impl Planes {
    /// From `BGRx` bytes.
    pub fn of(frame: &[u8]) -> Planes {
        let pixels = frame.len() / 4;
        let mut planes = Planes {
            luma: Vec::with_capacity(pixels),
            blue: Vec::with_capacity(pixels),
            red: Vec::with_capacity(pixels),
        };
        for pixel in frame.chunks_exact(4) {
            let (b, g, r) = (
                f32::from(pixel[0]),
                f32::from(pixel[1]),
                f32::from(pixel[2]),
            );
            let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            planes.luma.push(y.round() as u8);
            planes
                .blue
                .push(((b - y) / 1.8556 + 128.0).round().clamp(0.0, 255.0) as u8);
            planes
                .red
                .push(((r - y) / 1.5748 + 128.0).round().clamp(0.0, 255.0) as u8);
        }
        planes
    }
}

/// Structural similarity of two planes of the same size: 1 for identical
/// ones. Windows of 8 by 8 samples, four samples apart, as the usual tools
/// take them.
pub fn ssim(a: &[u8], b: &[u8], width: usize, height: usize) -> f64 {
    const C1: f64 = (0.01 * 255.0) * (0.01 * 255.0);
    const C2: f64 = (0.03 * 255.0) * (0.03 * 255.0);
    let mut total = 0.0;
    let mut windows = 0u64;
    let mut top = 0;
    while top + 8 <= height {
        let mut left = 0;
        while left + 8 <= width {
            let (mut sum_a, mut sum_b, mut sum_aa, mut sum_bb, mut sum_ab) =
                (0u32, 0u32, 0u32, 0u32, 0u32);
            for row in top..top + 8 {
                let start = row * width + left;
                for (p, q) in a[start..start + 8].iter().zip(&b[start..start + 8]) {
                    let (p, q) = (u32::from(*p), u32::from(*q));
                    sum_a += p;
                    sum_b += q;
                    sum_aa += p * p;
                    sum_bb += q * q;
                    sum_ab += p * q;
                }
            }
            let (mean_a, mean_b) = (f64::from(sum_a) / 64.0, f64::from(sum_b) / 64.0);
            let variance_a = f64::from(sum_aa) / 64.0 - mean_a * mean_a;
            let variance_b = f64::from(sum_bb) / 64.0 - mean_b * mean_b;
            let covariance = f64::from(sum_ab) / 64.0 - mean_a * mean_b;
            total += ((2.0 * mean_a * mean_b + C1) * (2.0 * covariance + C2))
                / ((mean_a * mean_a + mean_b * mean_b + C1) * (variance_a + variance_b + C2));
            windows += 1;
            left += 4;
        }
        top += 4;
    }
    if windows == 0 {
        1.0
    } else {
        total / windows as f64
    }
}

/// Peak signal-to-noise ratio of two planes in decibels; infinite for
/// identical ones.
pub fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let squares: u64 = a
        .iter()
        .zip(b)
        .map(|(p, q)| {
            let difference = u64::from(p.abs_diff(*q));
            difference * difference
        })
        .sum();
    if squares == 0 {
        return f64::INFINITY;
    }
    let mean = squares as f64 / a.len().max(1) as f64;
    10.0 * (255.0 * 255.0 / mean).log10()
}

/// One frame against the one that was sent.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct FrameQuality {
    pub ssim_luma: f64,
    pub ssim_blue: f64,
    pub ssim_red: f64,
    pub psnr_luma: f64,
}

impl FrameQuality {
    /// Both frames are `BGRx` bytes of `width` by `height` pixels.
    pub fn of(sent: &[u8], received: &[u8], width: usize, height: usize) -> FrameQuality {
        let (a, b) = (Planes::of(sent), Planes::of(received));
        FrameQuality {
            ssim_luma: ssim(&a.luma, &b.luma, width, height),
            ssim_blue: ssim(&a.blue, &b.blue, width, height),
            ssim_red: ssim(&a.red, &b.red, width, height),
            psnr_luma: psnr(&a.luma, &b.luma),
        }
    }
}

/// A figure over all frames. The low end is what matters: `p5` is the value
/// that one frame in twenty falls below.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Spread {
    pub mean: f64,
    pub median: f64,
    pub p5: f64,
    pub min: f64,
}

impl Spread {
    fn of(values: &[f64]) -> Option<Spread> {
        if values.is_empty() {
            return None;
        }
        let mut ordered = values.to_vec();
        ordered.sort_by(f64::total_cmp);
        Some(Spread {
            mean: ordered.iter().sum::<f64>() / ordered.len() as f64,
            median: percentile(&ordered, 50.0)?,
            p5: percentile(&ordered, 5.0)?,
            min: ordered[0],
        })
    }
}

/// The quality of a run of frames.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Quality {
    /// Frames the figures below are about.
    pub frames: usize,
    /// Frames at the start that are left out of them: an encoder needs a
    /// moment to settle after it starts.
    pub skipped: usize,
    pub width: usize,
    pub height: usize,
    pub ssim_luma: Option<Spread>,
    pub ssim_blue: Option<Spread>,
    pub ssim_red: Option<Spread>,
    /// Frames that came back exactly as sent have no finite value and are
    /// counted as 100 dB.
    pub psnr_luma: Option<Spread>,
    /// The frame whose brightness came through worst, counted from 0 at the
    /// first frame received.
    pub worst_frame: Option<usize>,
    /// Every frame, in order, the skipped ones too.
    pub per_frame: Vec<FrameQuality>,
}

impl Quality {
    /// `skip` frames at the start are compared like the others but left out
    /// of the figures.
    pub fn of(per_frame: Vec<FrameQuality>, skip: usize, width: usize, height: usize) -> Quality {
        let skipped = skip.min(per_frame.len());
        let counted = &per_frame[skipped..];
        let column =
            |pick: fn(&FrameQuality) -> f64| -> Vec<f64> { counted.iter().map(pick).collect() };
        let worst_frame = counted
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.ssim_luma.total_cmp(&b.1.ssim_luma))
            .map(|(number, _)| skipped + number);
        Quality {
            frames: counted.len(),
            skipped,
            width,
            height,
            ssim_luma: Spread::of(&column(|f| f.ssim_luma)),
            ssim_blue: Spread::of(&column(|f| f.ssim_blue)),
            ssim_red: Spread::of(&column(|f| f.ssim_red)),
            psnr_luma: Spread::of(&column(|f| f.psnr_luma.min(100.0))),
            worst_frame,
            per_frame,
        }
    }

    /// The figures as text for the terminal.
    pub fn render(&self) -> String {
        let mut out = format!(
            "Frames compared: {} of {}x{}",
            self.frames, self.width, self.height
        );
        if self.skipped > 0 {
            out.push_str(&format!(" (the first {} left out)", self.skipped));
        }
        out.push('\n');
        out.push_str(&format!(
            "{:<34}{:>9}{:>9}{:>9}{:>9}\n",
            "", "mean", "median", "5%", "min"
        ));
        let mut row = |label: &str, spread: &Option<Spread>, digits: usize| match spread {
            Some(s) => out.push_str(&format!(
                "{label:<34}{:>9.digits$}{:>9.digits$}{:>9.digits$}{:>9.digits$}\n",
                s.mean, s.median, s.p5, s.min
            )),
            None => out.push_str(&format!("{label:<34}{:>9}\n", "-")),
        };
        row("SSIM, brightness", &self.ssim_luma, 4);
        row("SSIM, colour (blue difference)", &self.ssim_blue, 4);
        row("SSIM, colour (red difference)", &self.ssim_red, 4);
        row("PSNR, brightness (dB)", &self.psnr_luma, 1);
        if let Some(number) = self.worst_frame {
            out.push_str(&format!(
                "Brightness came through worst in frame {number}.\n"
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flash::classify;
    use crate::flash::Rgb;

    fn film() -> Film {
        Film::new(320, 200, 2, 85, 240, 36)
    }

    fn frame(film: &Film, number: u64) -> Vec<u8> {
        let mut frame = vec![0; film.frame_len()];
        film.paint(number, &mut frame);
        frame
    }

    fn middle(frame: &[u8], width: usize, height: usize) -> Rgb {
        let at = ((height / 2) * width + width / 2) * 4;
        Rgb::new(frame[at + 2], frame[at + 1], frame[at])
    }

    #[test]
    fn a_film_is_the_same_every_time_and_moves() {
        let (first, second) = (film(), film());
        assert_eq!(frame(&first, 7), frame(&second, 7));
        assert_ne!(frame(&first, 7), frame(&first, 8));
        // 240 rows a second at 85 frames a second: frame 85 is 240 rows on.
        let sheet = Sheet::new(320, 200, 2);
        let mut pixels = vec![0u32; 320 * 200];
        paint_text(
            &mut pixels,
            320,
            200,
            &sheet,
            240 % sheet.height(),
            Shade::Dark,
        );
        let expected: Vec<u8> = pixels.iter().flat_map(|p| p.to_le_bytes()).collect();
        // Frame 85 is in the third stretch of 36 frames: the shade is dark again.
        assert_eq!(frame(&first, 85), expected);
    }

    #[test]
    fn the_shade_switches_as_asked() {
        let film = film();
        for (number, shade) in [
            (0, Shade::Dark),
            (35, Shade::Dark),
            (36, Shade::Light),
            (71, Shade::Light),
            (72, Shade::Dark),
        ] {
            assert_eq!(
                classify(middle(&frame(&film, number), 320, 200)),
                Some(shade),
                "frame {number}"
            );
        }
        let still = Film::new(320, 200, 2, 85, 240, 0);
        assert_eq!(
            classify(middle(&frame(&still, 500), 320, 200)),
            Some(Shade::Dark)
        );
    }

    #[test]
    fn a_frame_that_comes_back_as_sent_is_perfect() {
        let sent = frame(&film(), 3);
        let quality = FrameQuality::of(&sent, &sent, 320, 200);
        assert_eq!(
            (quality.ssim_luma, quality.ssim_blue, quality.ssim_red),
            (1.0, 1.0, 1.0)
        );
        assert_eq!(quality.psnr_luma, f64::INFINITY);
    }

    #[test]
    fn a_blurred_frame_scores_lower_than_a_slightly_noisy_one() {
        let sent = frame(&film(), 3);
        // Every pixel one step brighter: hardly a difference.
        let nudged: Vec<u8> = sent.iter().map(|v| v.saturating_add(1)).collect();
        // Every pixel the mean of itself and its right neighbour: letters smear.
        let mut blurred = sent.clone();
        for at in (0..sent.len() - 4).step_by(4) {
            for channel in 0..3 {
                blurred[at + channel] =
                    ((u16::from(sent[at + channel]) + u16::from(sent[at + 4 + channel])) / 2) as u8;
            }
        }
        let nudged = FrameQuality::of(&sent, &nudged, 320, 200);
        let blurred = FrameQuality::of(&sent, &blurred, 320, 200);
        assert!(nudged.ssim_luma > 0.99, "{nudged:?}");
        assert!(blurred.ssim_luma < 0.98, "{blurred:?}");
        assert!(blurred.ssim_luma < nudged.ssim_luma);
        assert!(blurred.psnr_luma < nudged.psnr_luma);
        // One step in every channel is 48 dB.
        assert!((nudged.psnr_luma - 48.13).abs() < 0.1, "{nudged:?}");
    }

    #[test]
    fn smeared_colour_shows_in_the_colour_differences_and_not_in_the_brightness() {
        // Font pixels one window pixel wide. (At two, the letters sit exactly on the
        // blocks of four that are averaged below, and nothing would be lost.)
        let sent = frame(&Film::new(320, 200, 1, 85, 240, 36), 3);
        let planes = Planes::of(&sent);
        // What a codec does that keeps one colour sample for four pixels:
        // every 2 by 2 block gets the mean of its colour differences.
        let (width, height) = (320, 200);
        let mut smeared = Planes {
            luma: planes.luma.clone(),
            blue: planes.blue.clone(),
            red: planes.red.clone(),
        };
        for top in (0..height).step_by(2) {
            for left in (0..width).step_by(2) {
                let block = [
                    top * width + left,
                    top * width + left + 1,
                    (top + 1) * width + left,
                    (top + 1) * width + left + 1,
                ];
                for plane in [&mut smeared.blue, &mut smeared.red] {
                    let mean = block.iter().map(|at| u32::from(plane[*at])).sum::<u32>() / 4;
                    for at in block {
                        plane[at] = mean as u8;
                    }
                }
            }
        }
        assert_eq!(ssim(&planes.luma, &smeared.luma, width, height), 1.0);
        let blue = ssim(&planes.blue, &smeared.blue, width, height);
        let red = ssim(&planes.red, &smeared.red, width, height);
        assert!(blue < 0.97 && red < 0.97, "{blue} {red}");
    }

    #[test]
    fn grey_has_no_colour_and_primaries_have_the_most() {
        let planes = Planes::of(&[
            0x80, 0x80, 0x80, 0, // grey
            0xFF, 0x00, 0x00, 0, // blue
            0x00, 0x00, 0xFF, 0, // red
            0xFF, 0xFF, 0xFF, 0, // white
        ]);
        assert_eq!(planes.luma, [128, 18, 54, 255]);
        assert_eq!(planes.blue, [128, 255, 99, 128]);
        assert_eq!(planes.red, [128, 116, 255, 128]);
    }

    #[test]
    fn the_summary_looks_at_the_low_end() {
        let frames: Vec<FrameQuality> = (0..100)
            .map(|i| FrameQuality {
                ssim_luma: if i == 40 { 0.5 } else { 0.99 },
                ssim_blue: 0.9,
                ssim_red: 0.8,
                psnr_luma: if i == 3 { f64::INFINITY } else { 40.0 },
            })
            .collect();
        let quality = Quality::of(frames.clone(), 0, 320, 200);
        assert_eq!((quality.frames, quality.worst_frame), (100, Some(40)));
        let luma = quality.ssim_luma.unwrap();
        assert_eq!((luma.min, luma.p5, luma.median), (0.5, 0.99, 0.99));
        // A frame that came back as sent does not make the mean infinite.
        assert!((quality.psnr_luma.unwrap().mean - 40.6).abs() < 1e-9);
        let text = quality.render();
        assert!(text.contains("Frames compared: 100 of 320x200"), "{text}");
        assert!(
            text.contains("Brightness came through worst in frame 40."),
            "{text}"
        );

        // Without the first 41 frames the bad one is gone, and the frames keep their numbers.
        let later = Quality::of(frames.clone(), 41, 320, 200);
        assert_eq!(
            (later.frames, later.skipped, later.per_frame.len()),
            (59, 41, 100)
        );
        assert_eq!(later.ssim_luma.unwrap().min, 0.99);
        assert_eq!(later.worst_frame, Some(41));
        assert!(later
            .render()
            .contains("Frames compared: 59 of 320x200 (the first 41 left out)"));

        let nothing = Quality::of(Vec::new(), 0, 320, 200);
        assert_eq!((nothing.ssim_luma, nothing.worst_frame), (None, None));
        assert!(nothing.render().contains("Frames compared: 0"));
        // More to skip than there is: nothing is left, and nothing breaks.
        let all_skipped = Quality::of(frames, 500, 320, 200);
        assert_eq!((all_skipped.frames, all_skipped.skipped), (0, 100));
        assert_eq!(all_skipped.ssim_luma, None);
    }
}
