//! What the test application paints.
//!
//! [`Picture::Grey`] fills the window with the current shade. Nothing
//! compresses better: a frame that repeats it is a few dozen bytes, and one
//! that switches it a few hundred. Measurements with it say what a relay
//! costs at best.
//!
//! [`Picture::Text`] is what measurements are judged with. Lines of text
//! scroll up the window, as in a terminal or an editor at work, so that
//! every frame differs from the one before and the relay has a real picture
//! to carry. A square in the middle shows the current shade; that is where
//! the measuring tools look.

use istesso_measure::{Rgb, Shade};

/// What the window shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Picture {
    /// Scrolling text with a square in the middle that shows the shade
    Text,
    /// The whole window in the shade: the easiest picture there is
    Grey,
}

impl Picture {
    /// The name written into a log.
    pub fn name(self) -> &'static str {
        match self {
            Picture::Text => "text",
            Picture::Grey => "grey",
        }
    }
}

/// softbuffer's pixel layout: 0x00RRGGBB.
pub fn pixel(colour: Rgb) -> u32 {
    u32::from(colour.r) << 16 | u32::from(colour.g) << 8 | u32::from(colour.b)
}

// The colours of an editor with a dark theme.
/// What the text is written on, and the rim around the square with the shade.
pub const PAPER: Rgb = Rgb::new(0x1E, 0x20, 0x26);
const PLAIN: Rgb = Rgb::new(0xC8, 0xCC, 0xD4);
const KEYWORD: Rgb = Rgb::new(0xC6, 0x78, 0xDD);
const NUMBER: Rgb = Rgb::new(0xD1, 0x9A, 0x66);
const STRING: Rgb = Rgb::new(0x98, 0xC3, 0x79);
const CALL: Rgb = Rgb::new(0x61, 0xAF, 0xEF);
const MARK: Rgb = Rgb::new(0x7F, 0x84, 0x8E);

const KEYWORDS: &[&str] = &[
    "let", "fn", "for", "while", "if", "else", "match", "return", "pub", "use", "impl", "struct",
    "mut", "loop", "break", "const",
];
const NAMES: &[&str] = &[
    "window",
    "frame",
    "input",
    "latency",
    "encoder",
    "decoder",
    "buffer",
    "socket",
    "commit",
    "display",
    "refresh",
    "queue",
    "packet",
    "stream",
    "clock",
    "sample",
    "median",
    "host",
    "client",
    "pixel",
    "shade",
    "relay",
    "surface",
    "compositor",
    "percentile",
    "timestamp",
    "session",
    "capture",
];
const CALLS: &[&str] = &[
    "measure", "render", "present", "send", "decode", "encode", "wait", "now_ns", "toggled",
    "collect",
];
const NUMBERS: &[&str] = &[
    "0", "1", "16", "85", "120", "720", "1440", "0x20", "0xE0", "47811", "3.3", "11.76",
];
const STRINGS: &[&str] = &[
    "\"relayed\"",
    "\"local\"",
    "\"commit\"",
    "\"shown\"",
    "\"added latency\"",
    "\"host\"",
];
const MARKS: &[&str] = &[
    "=", "==", "+=", "->", "=>", "(", ")", "{", "}", ";", ",", "::", ".", "&", "<", ">", "[", "]",
];

/// A glyph is 5 by 7 font pixels and sits in a cell of 6 by 10.
const GLYPH_WIDTH: usize = 5;
const GLYPH_HEIGHT: usize = 7;
const CELL_WIDTH: usize = 6;
const CELL_HEIGHT: usize = 10;

/// The printable ASCII characters from space to tilde. One byte per row, top
/// row first; bit 4 is the leftmost pixel.
#[rustfmt::skip]
const GLYPHS: [[u8; GLYPH_HEIGHT]; 95] = [
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // space
    [0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x04], // !
    [0x0A, 0x0A, 0x0A, 0x00, 0x00, 0x00, 0x00], // "
    [0x0A, 0x0A, 0x1F, 0x0A, 0x1F, 0x0A, 0x0A], // #
    [0x04, 0x0F, 0x14, 0x0E, 0x05, 0x1E, 0x04], // $
    [0x18, 0x19, 0x02, 0x04, 0x08, 0x13, 0x03], // %
    [0x0C, 0x12, 0x14, 0x08, 0x15, 0x12, 0x0D], // &
    [0x0C, 0x04, 0x08, 0x00, 0x00, 0x00, 0x00], // '
    [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02], // (
    [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08], // )
    [0x00, 0x04, 0x15, 0x0E, 0x15, 0x04, 0x00], // *
    [0x00, 0x04, 0x04, 0x1F, 0x04, 0x04, 0x00], // +
    [0x00, 0x00, 0x00, 0x00, 0x0C, 0x04, 0x08], // ,
    [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00], // -
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C], // .
    [0x00, 0x01, 0x02, 0x04, 0x08, 0x10, 0x00], // /
    [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E], // 0
    [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E], // 1
    [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F], // 2
    [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E], // 3
    [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02], // 4
    [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E], // 5
    [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E], // 6
    [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08], // 7
    [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E], // 8
    [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C], // 9
    [0x00, 0x0C, 0x0C, 0x00, 0x0C, 0x0C, 0x00], // :
    [0x00, 0x0C, 0x0C, 0x00, 0x0C, 0x04, 0x08], // ;
    [0x02, 0x04, 0x08, 0x10, 0x08, 0x04, 0x02], // <
    [0x00, 0x00, 0x1F, 0x00, 0x1F, 0x00, 0x00], // =
    [0x08, 0x04, 0x02, 0x01, 0x02, 0x04, 0x08], // >
    [0x0E, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04], // ?
    [0x0E, 0x11, 0x01, 0x0D, 0x15, 0x15, 0x0E], // @
    [0x0E, 0x11, 0x11, 0x11, 0x1F, 0x11, 0x11], // A
    [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E], // B
    [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E], // C
    [0x1C, 0x12, 0x11, 0x11, 0x11, 0x12, 0x1C], // D
    [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F], // E
    [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10], // F
    [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F], // G
    [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11], // H
    [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E], // I
    [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C], // J
    [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11], // K
    [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F], // L
    [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11], // M
    [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11], // N
    [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E], // O
    [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10], // P
    [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D], // Q
    [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11], // R
    [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E], // S
    [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04], // T
    [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E], // U
    [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04], // V
    [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A], // W
    [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11], // X
    [0x11, 0x11, 0x11, 0x0A, 0x04, 0x04, 0x04], // Y
    [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F], // Z
    [0x0E, 0x08, 0x08, 0x08, 0x08, 0x08, 0x0E], // [
    [0x00, 0x10, 0x08, 0x04, 0x02, 0x01, 0x00], // backslash
    [0x0E, 0x02, 0x02, 0x02, 0x02, 0x02, 0x0E], // ]
    [0x04, 0x0A, 0x11, 0x00, 0x00, 0x00, 0x00], // ^
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F], // _
    [0x08, 0x04, 0x02, 0x00, 0x00, 0x00, 0x00], // `
    [0x00, 0x00, 0x0E, 0x01, 0x0F, 0x11, 0x0F], // a
    [0x10, 0x10, 0x16, 0x19, 0x11, 0x11, 0x1E], // b
    [0x00, 0x00, 0x0E, 0x10, 0x10, 0x11, 0x0E], // c
    [0x01, 0x01, 0x0D, 0x13, 0x11, 0x11, 0x0F], // d
    [0x00, 0x00, 0x0E, 0x11, 0x1F, 0x10, 0x0E], // e
    [0x06, 0x09, 0x08, 0x1C, 0x08, 0x08, 0x08], // f
    [0x00, 0x0F, 0x11, 0x11, 0x0F, 0x01, 0x0E], // g
    [0x10, 0x10, 0x16, 0x19, 0x11, 0x11, 0x11], // h
    [0x04, 0x00, 0x0C, 0x04, 0x04, 0x04, 0x0E], // i
    [0x02, 0x00, 0x06, 0x02, 0x02, 0x12, 0x0C], // j
    [0x10, 0x10, 0x12, 0x14, 0x18, 0x14, 0x12], // k
    [0x0C, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E], // l
    [0x00, 0x00, 0x1A, 0x15, 0x15, 0x11, 0x11], // m
    [0x00, 0x00, 0x16, 0x19, 0x11, 0x11, 0x11], // n
    [0x00, 0x00, 0x0E, 0x11, 0x11, 0x11, 0x0E], // o
    [0x00, 0x00, 0x1E, 0x11, 0x1E, 0x10, 0x10], // p
    [0x00, 0x00, 0x0D, 0x13, 0x0F, 0x01, 0x01], // q
    [0x00, 0x00, 0x16, 0x19, 0x10, 0x10, 0x10], // r
    [0x00, 0x00, 0x0E, 0x10, 0x0E, 0x01, 0x1E], // s
    [0x08, 0x08, 0x1C, 0x08, 0x08, 0x09, 0x06], // t
    [0x00, 0x00, 0x11, 0x11, 0x11, 0x13, 0x0D], // u
    [0x00, 0x00, 0x11, 0x11, 0x11, 0x0A, 0x04], // v
    [0x00, 0x00, 0x11, 0x11, 0x15, 0x15, 0x0A], // w
    [0x00, 0x00, 0x11, 0x0A, 0x04, 0x0A, 0x11], // x
    [0x00, 0x00, 0x11, 0x11, 0x0F, 0x01, 0x0E], // y
    [0x00, 0x00, 0x1F, 0x02, 0x04, 0x08, 0x1F], // z
    [0x02, 0x04, 0x04, 0x08, 0x04, 0x04, 0x02], // {
    [0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04], // |
    [0x08, 0x04, 0x04, 0x02, 0x04, 0x04, 0x08], // }
    [0x00, 0x00, 0x08, 0x15, 0x02, 0x00, 0x00], // ~
];

fn glyph(character: char) -> &'static [u8; GLYPH_HEIGHT] {
    let unknown = '?' as usize - 0x20;
    let index = (character as usize)
        .checked_sub(0x20)
        .filter(|index| *index < GLYPHS.len())
        .unwrap_or(unknown);
    &GLYPHS[index]
}

/// SplitMix64: the text is the same on every machine and in every run.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }

    fn pick(&mut self, words: &[&'static str]) -> &'static str {
        words[self.below(words.len())]
    }
}

/// One line of text that looks like source code: words with their colours,
/// at most `columns` characters long. One line in seven is empty.
fn line(random: &mut Random, columns: usize) -> Vec<(&'static str, Rgb)> {
    let mut words = Vec::new();
    if columns < 8 || random.below(7) == 0 {
        return words;
    }
    let indent = 4 * random.below(4);
    let target = columns / 4 + random.below(columns * 7 / 10);
    let mut length = indent.min(columns / 2);
    words.push((&"                "[..length], PAPER));
    loop {
        let (word, colour) = match random.below(100) {
            0..=14 => (random.pick(KEYWORDS), KEYWORD),
            15..=54 => (random.pick(NAMES), PLAIN),
            55..=64 => (random.pick(CALLS), CALL),
            65..=72 => (random.pick(NUMBERS), NUMBER),
            73..=79 => (random.pick(STRINGS), STRING),
            _ => (random.pick(MARKS), MARK),
        };
        if length + word.len() + 1 > target.min(columns) {
            return words;
        }
        words.push((word, colour));
        words.push((" ", PAPER));
        length += word.len() + 1;
    }
}

/// The text, painted once: as wide as the window and at least twice as tall,
/// a whole number of lines. A frame is a window's worth of its rows; moving
/// on by a few rows from frame to frame is what makes the text scroll, and
/// after the last row comes the first again.
pub struct Sheet {
    width: usize,
    window_height: usize,
    scale: usize,
    height: usize,
    pixels: Vec<u32>,
}

impl Sheet {
    /// `scale` is the size of a font pixel in window pixels.
    pub fn new(width: usize, window_height: usize, scale: usize) -> Sheet {
        let scale = scale.max(1);
        let line_height = CELL_HEIGHT * scale;
        let lines = 2 * window_height.div_ceil(line_height) + 1;
        let height = lines * line_height;
        let columns = width / (CELL_WIDTH * scale);
        let mut pixels = vec![pixel(PAPER); width * height];
        let mut random = Random(0x1573_5E55_0F1A_5ED1);
        for number in 0..lines {
            let mut column = 0;
            for (word, colour) in line(&mut random, columns) {
                for character in word.chars() {
                    if colour != PAPER {
                        let left = column * CELL_WIDTH * scale;
                        // The glyph sits one font pixel below the top of its line.
                        let top = number * line_height + scale;
                        stamp(
                            &mut pixels,
                            width,
                            left,
                            top,
                            scale,
                            glyph(character),
                            pixel(colour),
                        );
                    }
                    column += 1;
                }
            }
        }
        Sheet {
            width,
            window_height,
            scale,
            height,
            pixels,
        }
    }

    /// Whether this sheet was painted for a window of this size.
    pub fn fits(&self, width: usize, window_height: usize, scale: usize) -> bool {
        (self.width, self.window_height, self.scale) == (width, window_height, scale.max(1))
    }

    /// Rows in the sheet.
    pub fn height(&self) -> usize {
        self.height
    }

    /// Pixels in a row.
    pub fn width(&self) -> usize {
        self.width
    }

    /// The whole sheet, row after row, in softbuffer's pixel layout.
    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    fn row(&self, number: usize) -> &[u32] {
        &self.pixels[number * self.width..(number + 1) * self.width]
    }
}

/// Paints one glyph, every font pixel as a square of `scale` window pixels.
fn stamp(
    pixels: &mut [u32],
    width: usize,
    left: usize,
    top: usize,
    scale: usize,
    glyph: &[u8; GLYPH_HEIGHT],
    colour: u32,
) {
    for (row, bits) in glyph.iter().enumerate() {
        for column in 0..GLYPH_WIDTH {
            if bits & (0x10 >> column) == 0 {
                continue;
            }
            for y in top + row * scale..top + (row + 1) * scale {
                let start = y * width + left + column * scale;
                pixels[start..start + scale].fill(colour);
            }
        }
    }
}

/// How many rows the text has moved after `elapsed_ns` at `speed` rows per
/// second, counted round the sheet.
pub fn scrolled(elapsed_ns: u64, speed: u32, sheet_height: usize) -> usize {
    let rows = u128::from(elapsed_ns) * u128::from(speed) / 1_000_000_000;
    (rows % sheet_height.max(1) as u128) as usize
}

/// Where the square that shows the shade is: left, top and side, in pixels.
///
/// It is a quarter of the window's shorter side and never smaller than 48
/// pixels, unless the window is. The measuring tools look at the middle of
/// the window, at an area of some 20 pixels across.
pub fn square(width: usize, height: usize) -> (usize, usize, usize) {
    let side = (width.min(height) / 4).max(48).min(width).min(height);
    ((width - side) / 2, (height - side) / 2, side)
}

/// How wide the rim of paper around the shade is, for a square of this side.
///
/// It keeps the text away from the shade, so that what a codec does to the
/// letters does not reach the middle.
pub fn rim(side: usize) -> usize {
    side / 8
}

/// Paints one frame of the text picture: the rows of the sheet from `offset`
/// on, and over them the square with the shade.
///
/// `frame` holds `height` rows of `width` pixels. The sheet has to be one
/// that [`Sheet::fits`] the frame.
pub fn paint_text(
    frame: &mut [u32],
    width: usize,
    height: usize,
    sheet: &Sheet,
    offset: usize,
    shade: Shade,
) {
    for (y, row) in frame.chunks_exact_mut(width).take(height).enumerate() {
        row.copy_from_slice(sheet.row((y + offset) % sheet.height));
    }
    let (left, top, side) = square(width, height);
    let rim = rim(side);
    fill(frame, width, left, top, side, side, pixel(PAPER));
    fill(
        frame,
        width,
        left + rim,
        top + rim,
        side - 2 * rim,
        side - 2 * rim,
        pixel(shade.rgb()),
    );
}

fn fill(
    frame: &mut [u32],
    width: usize,
    left: usize,
    top: usize,
    across: usize,
    down: usize,
    colour: u32,
) {
    for y in top..top + down {
        frame[y * width + left..y * width + left + across].fill(colour);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use istesso_measure::classify;
    use std::collections::BTreeSet;

    fn frame(sheet: &Sheet, width: usize, height: usize, offset: usize, shade: Shade) -> Vec<u32> {
        let mut frame = vec![0; width * height];
        paint_text(&mut frame, width, height, sheet, offset, shade);
        frame
    }

    fn colour(pixel: u32) -> Rgb {
        Rgb::new((pixel >> 16) as u8, (pixel >> 8) as u8, pixel as u8)
    }

    #[test]
    fn pixels_are_packed_as_softbuffer_expects() {
        assert_eq!(pixel(Rgb::new(0x12, 0x34, 0x56)), 0x0012_3456);
        assert_eq!(pixel(Shade::Dark.rgb()), 0x0020_2020);
        assert_eq!(pixel(Shade::Light.rgb()), 0x00E0_E0E0);
    }

    #[test]
    fn every_printable_character_has_its_own_glyph() {
        let glyphs: BTreeSet<[u8; GLYPH_HEIGHT]> = (' '..='~').map(|c| *glyph(c)).collect();
        assert_eq!(glyphs.len(), 95);
        assert_eq!(glyph(' '), &[0; GLYPH_HEIGHT]);
        // No glyph is wider than its five columns.
        assert!(GLYPHS.iter().flatten().all(|row| *row < 0x20));
        // What the font does not have is shown as a question mark.
        assert_eq!(glyph('é'), glyph('?'));
        assert_eq!(glyph('\n'), glyph('?'));
    }

    #[test]
    fn every_word_can_be_written() {
        for words in [KEYWORDS, NAMES, CALLS, NUMBERS, STRINGS, MARKS] {
            for word in words {
                assert!(word.chars().all(|c| (' '..='~').contains(&c)), "{word}");
            }
        }
    }

    #[test]
    fn the_text_is_the_same_every_time() {
        let first = Sheet::new(600, 400, 2);
        let second = Sheet::new(600, 400, 2);
        assert_eq!(first.pixels, second.pixels);
        assert!(first.fits(600, 400, 2));
        assert!(!first.fits(600, 401, 2));
        assert!(!first.fits(601, 400, 2));
        assert!(!first.fits(600, 400, 4));
    }

    #[test]
    fn the_sheet_is_at_least_two_windows_of_whole_lines() {
        for (width, height, scale) in [(600, 400, 2), (1720, 720, 2), (300, 37, 1), (1200, 800, 4)]
        {
            let sheet = Sheet::new(width, height, scale);
            assert!(sheet.height() >= 2 * height, "{width}x{height}");
            assert_eq!(sheet.height() % (CELL_HEIGHT * scale), 0);
            assert_eq!(sheet.pixels.len(), width * sheet.height());
        }
    }

    #[test]
    fn the_sheet_looks_like_text() {
        let sheet = Sheet::new(1720, 720, 2);
        let paper = pixel(PAPER);
        let inked = sheet.pixels.iter().filter(|p| **p != paper).count();
        let share = inked as f64 / sheet.pixels.len() as f64;
        // Letters cover a small part of a page, and far from none of it.
        assert!((0.03..0.30).contains(&share), "{share}");
        let colours: BTreeSet<u32> = sheet.pixels.iter().copied().collect();
        assert_eq!(colours.len(), 7, "paper and six kinds of word");
        // No line runs into the right edge, and lines differ in length.
        let line_height = CELL_HEIGHT * 2;
        let widths: BTreeSet<usize> = (0..sheet.height() / line_height)
            .map(|line| {
                (0..line_height)
                    .filter_map(|y| {
                        sheet
                            .row(line * line_height + y)
                            .iter()
                            .rposition(|p| *p != paper)
                    })
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        assert!(widths.len() > 20, "{widths:?}");
        assert!(*widths.iter().max().unwrap() < 1720);
        assert!(widths.contains(&0), "some lines are empty");
    }

    #[test]
    fn a_frame_is_a_window_of_the_sheet() {
        let (width, height) = (600, 400);
        let sheet = Sheet::new(width, height, 2);
        let (left, top, side) = square(width, height);
        for offset in [0, 1, 37, sheet.height() - 1] {
            let frame = frame(&sheet, width, height, offset, Shade::Dark);
            for y in [0, 1, top - 1, top + side, height - 1] {
                assert_eq!(
                    &frame[y * width..(y + 1) * width],
                    sheet.row((y + offset) % sheet.height()),
                    "row {y} at offset {offset}"
                );
            }
            // Beside the square the text goes on.
            let y = top + side / 2;
            assert_eq!(
                &frame[y * width..y * width + left],
                &sheet.row((y + offset) % sheet.height())[..left]
            );
        }
        // Once round the sheet is where it started.
        assert_eq!(
            frame(&sheet, width, height, 0, Shade::Light),
            frame(&sheet, width, height, sheet.height(), Shade::Light)
        );
        // One row further is another picture.
        assert_ne!(
            frame(&sheet, width, height, 0, Shade::Light),
            frame(&sheet, width, height, 1, Shade::Light)
        );
    }

    #[test]
    fn the_middle_shows_the_shade_whatever_the_text_does() {
        for (width, height) in [(600, 400), (1720, 720), (3440, 1440), (960, 600)] {
            let sheet = Sheet::new(width, height, 2);
            for shade in [Shade::Dark, Shade::Light] {
                for offset in [0, 13, 250] {
                    let frame = frame(&sheet, width, height, offset, shade);
                    // What the measuring tools look at, and a margin around it.
                    for y in height / 2 - 16..height / 2 + 16 {
                        for x in width / 2 - 16..width / 2 + 16 {
                            assert_eq!(
                                classify(colour(frame[y * width + x])),
                                Some(shade),
                                "{width}x{height} at {x},{y}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_square_has_a_rim_of_paper() {
        let (width, height) = (1720, 720);
        let (left, top, side) = square(width, height);
        assert_eq!((left, top, side), (770, 270, 180));
        let frame = frame(
            &Sheet::new(width, height, 2),
            width,
            height,
            5,
            Shade::Light,
        );
        let rim = rim(side);
        for step in 0..side {
            for inset in [0, rim - 1] {
                for (x, y) in [
                    (left + step, top + inset),
                    (left + step, top + side - 1 - inset),
                    (left + inset, top + step),
                    (left + side - 1 - inset, top + step),
                ] {
                    assert_eq!(frame[y * width + x], pixel(PAPER), "{x},{y}");
                }
            }
        }
        assert_eq!(
            frame[(top + rim) * width + left + rim],
            pixel(Shade::Light.rgb())
        );
    }

    #[test]
    fn small_windows_get_a_square_that_fits() {
        assert_eq!(square(600, 400), (250, 150, 100));
        // Never smaller than 48 pixels where there is room.
        assert_eq!(square(160, 120), (56, 36, 48));
        // And never larger than the window.
        assert_eq!(square(40, 30), (5, 0, 30));
        let frame = frame(&Sheet::new(40, 30, 2), 40, 30, 3, Shade::Light);
        assert_eq!(frame.len(), 40 * 30);
        assert_eq!(classify(colour(frame[15 * 40 + 20])), Some(Shade::Light));
    }

    #[test]
    fn the_text_moves_with_time_and_comes_round_again() {
        // 240 rows a second: one row every 1/240 s.
        assert_eq!(scrolled(0, 240, 1000), 0);
        assert_eq!(scrolled(1_000_000_000 / 240 + 1, 240, 1000), 1);
        assert_eq!(scrolled(1_000_000_000, 240, 1000), 240);
        assert_eq!(scrolled(5_000_000_000, 240, 1000), 200);
        // Standing still, and a sheet without rows, are not an accident waiting to happen.
        assert_eq!(scrolled(7_000_000_000, 0, 1000), 0);
        assert_eq!(scrolled(7_000_000_000, 240, 0), 0);
        // Days of running do not overflow.
        assert!(scrolled(u64::MAX, u32::MAX, 4320) < 4320);
    }
}
