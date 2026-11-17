//! The pictures of the test application.
//!
//! The grey picture fills the window with the current shade. Nothing
//! compresses better: a frame that repeats it is a few dozen bytes, and one
//! that switches it a few hundred. Measurements with it say what a relay
//! costs at best.
//!
//! The text picture is what measurements are judged with. Lines of text
//! scroll up the window, as in a terminal or an editor at work, so that
//! every frame differs from the one before and the relay has a real picture
//! to carry. A square in the middle shows the current shade; that is where
//! the measuring tools look.
//!
//! The hard picture is for seeing what a relay does when the picture is not
//! kind to it: something like a photograph, two layers of it moving across
//! each other, with a grain that is new in every frame. No part of a frame
//! can be made by moving a part of the one before.
//!
//! Either moving picture can carry a stamp: a row of cells in its top left
//! corner that says how far the picture has moved. A frame caught on a
//! screen then says itself which frame it is.
//!
//! The pictures are made here, not in the application, because a tool that
//! judges what a relay did to a frame has to make the very same frame again
//! to compare with ([`crate::quality`]).

use std::sync::OnceLock;

use crate::flash::{Rgb, Shade};

/// Colours to show in place of the pictures' own, for one process: see [`show_colours_as`].
static SHOWN: OnceLock<Vec<(Rgb, Rgb)>> = OnceLock::new();

/// From now on, in this process, every colour named first in a pair is shown as the second.
///
/// For the guess test only: there the test application on the client has to look like the
/// relayed one, colours included, so that a difference in colour does not give away what a
/// difference in latency should. A tool that judges a relay's picture never calls this. Only
/// the first call counts.
pub fn show_colours_as(pairs: Vec<(Rgb, Rgb)>) {
    let _ = SHOWN.set(pairs);
}

/// The colour that is shown for `colour`: itself, unless [`show_colours_as`] said otherwise.
pub fn shown(colour: Rgb) -> Rgb {
    SHOWN
        .get()
        .and_then(|pairs| pairs.iter().find(|(from, _)| *from == colour))
        .map_or(colour, |(_, to)| *to)
}

/// softbuffer's pixel layout: 0x00RRGGBB.
pub fn pixel(colour: Rgb) -> u32 {
    let colour = shown(colour);
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

    /// The sheet of the hard picture: something like a photograph, as wide as
    /// the window and twice as tall. Its right edge goes on at its left one
    /// and its bottom at its top, so it can be moved round and round.
    pub fn photo(width: usize, window_height: usize) -> Sheet {
        let height = 2 * window_height.max(1);
        let size = (width, height);
        let mut pixels = vec![0u32; width * height];
        let threads = std::thread::available_parallelism()
            .map_or(1, |count| count.get())
            .min(8);
        let rows_each = height.div_ceil(threads).max(1);
        std::thread::scope(|scope| {
            for (part, rows) in pixels.chunks_mut(rows_each * width.max(1)).enumerate() {
                scope.spawn(move || {
                    for (i, pixel) in rows.iter_mut().enumerate() {
                        *pixel = photo_pixel(i % width, part * rows_each + i / width, size);
                    }
                });
            }
        });
        Sheet {
            width,
            window_height,
            scale: 0,
            height,
            pixels,
        }
    }

    /// Whether this sheet is the hard picture's for a window of this size.
    pub fn fits_photo(&self, width: usize, window_height: usize) -> bool {
        (self.width, self.window_height, self.scale) == (width, window_height, 0)
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

/// Mixes three numbers into one, the same on every machine.
fn mix(seed: u32, x: u32, y: u32) -> u32 {
    let mut h = seed ^ x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77);
    h = (h ^ (h >> 15)).wrapping_mul(0x2C1B_3C6D);
    h = (h ^ (h >> 12)).wrapping_mul(0x297A_2D39);
    h ^ (h >> 15)
}

/// Smooth noise between 0 and 1 that repeats after `period` cells each way.
/// Only additions, multiplications and rounding down: the same on every
/// machine.
fn noise(seed: u32, x: f32, y: f32, period: (u32, u32)) -> f32 {
    let (left, top) = (x.floor(), y.floor());
    let (fx, fy) = (x - left, y - top);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let at = |dx: u32, dy: u32| {
        let value = mix(
            seed,
            (left as u32 + dx) % period.0,
            (top as u32 + dy) % period.1,
        );
        (value >> 8) as f32 / 16_777_216.0
    };
    let upper = at(0, 0) + (at(1, 0) - at(0, 0)) * sx;
    let lower = at(0, 1) + (at(1, 1) - at(0, 1)) * sx;
    upper + (lower - upper) * sy
}

/// Several layers of noise, each twice as fine and half as strong as the
/// one before, as in a photograph: between 0 and 1.
fn layers(seed: u32, x: usize, y: usize, size: (usize, usize), cell: usize, count: u32) -> f32 {
    let (mut total, mut weight, mut strength) = (0.0, 0.0, 1.0);
    for layer in 0..count {
        let cell = (cell >> layer).max(1);
        let period = ((size.0 / cell).max(1) as u32, (size.1 / cell).max(1) as u32);
        total += strength
            * noise(
                seed.wrapping_add(layer.wrapping_mul(0x632B_E5AB)),
                x as f32 * period.0 as f32 / size.0 as f32,
                y as f32 * period.1 as f32 / size.1 as f32,
                period,
            );
        weight += strength;
        strength *= 0.5;
    }
    total / weight
}

/// One pixel of the hard picture's sheet: soft areas of sky, leaves, sand
/// and rock, light and dark in patches of every size, darker things with
/// hard edges in front, and a fine grit.
fn photo_pixel(x: usize, y: usize, size: (usize, usize)) -> u32 {
    const PALETTE: [[f32; 3]; 4] = [
        [120.0, 170.0, 225.0],
        [70.0, 125.0, 60.0],
        [205.0, 180.0, 140.0],
        [110.0, 100.0, 95.0],
    ];
    let light = layers(0x51A7_E001, x, y, size, 256, 6);
    let place = layers(0x51A7_E002, x, y, size, 512, 3);
    let thing = layers(0x51A7_E003, x, y, size, 96, 2);
    let grit = layers(0x51A7_E004, x, y, size, 3, 1);
    let spot = ((place - 0.5) * 2.4 + 0.5).clamp(0.0, 0.999) * 3.0;
    let (from, part) = (spot.floor() as usize, spot - spot.floor());
    let mut shade = 0.35 + 0.95 * ((light - 0.5) * 2.2 + 0.5).clamp(0.0, 1.0) + (grit - 0.5) * 0.12;
    if thing > 0.6 {
        shade *= 0.45;
    }
    let channel = |i: usize| {
        let colour = PALETTE[from][i] + (PALETTE[from + 1][i] - PALETTE[from][i]) * part;
        (colour * shade).clamp(0.0, 255.0) as u32
    };
    channel(0) << 16 | channel(1) << 8 | channel(2)
}

/// How many rows a picture has moved after `elapsed_ns` at `speed` rows per
/// second, since it started.
pub fn moved(elapsed_ns: u64, speed: u32) -> u64 {
    (u128::from(elapsed_ns) * u128::from(speed) / 1_000_000_000) as u64
}

/// How many rows the text has moved after `elapsed_ns` at `speed` rows per
/// second, counted round the sheet.
pub fn scrolled(elapsed_ns: u64, speed: u32, sheet_height: usize) -> usize {
    (moved(elapsed_ns, speed) % sheet_height.max(1) as u64) as usize
}

/// Side of a cell of the stamp, and where the stamp begins, in pixels.
pub const STAMP_CELL: usize = 12;
pub const STAMP_LEFT: usize = 8;
pub const STAMP_TOP: usize = 8;
/// Two cells that say "this is a stamp" (light, dark), 24 for the number and
/// 8 that have to fit the number.
pub const STAMP_CELLS: usize = 34;
const STAMP_LIGHT: u32 = 0x00F0_F0F0;
const STAMP_DARK: u32 = 0x0010_1010;

/// What the 32 cells after the two markers show, first cell in the highest
/// bit: the low 24 bits of `rows`, and 8 bits made from them. A stamp that
/// was read wrong, or caught between two frames, does not fit.
pub fn stamp_code(rows: u64) -> u32 {
    let value = (rows & 0x00FF_FFFF) as u32;
    let check = ((value >> 16) ^ (value >> 8) ^ value ^ 0x5A) & 0xFF;
    value << 8 | check
}

/// Whether a picture of this size has room for the stamp.
pub fn stamp_fits(width: usize, height: usize) -> bool {
    width >= 2 * STAMP_LEFT + STAMP_CELLS * STAMP_CELL && height >= 2 * STAMP_TOP + STAMP_CELL
}

/// Paints the stamp for a picture that has moved `rows` rows. Cells are a
/// good deal larger than what an encoder smears, and only light or dark, so
/// that they can be read off a relayed picture whatever happened to its
/// colours. A picture too small for it gets none.
pub fn paint_stamp(frame: &mut [u32], width: usize, height: usize, rows: u64) {
    if !stamp_fits(width, height) {
        return;
    }
    let code = stamp_code(rows);
    for cell in 0..STAMP_CELLS {
        let light = match cell {
            0 => true,
            1 => false,
            _ => code >> (33 - cell) & 1 == 1,
        };
        fill(
            frame,
            width,
            STAMP_LEFT + cell * STAMP_CELL,
            STAMP_TOP,
            STAMP_CELL,
            STAMP_CELL,
            if light { STAMP_LIGHT } else { STAMP_DARK },
        );
    }
}

/// Reads the stamp of a picture given as four bytes a pixel (blue, green,
/// red, unused): how many rows it had moved, as far as 24 bits say. `None`
/// when there is no stamp or it cannot be read.
pub fn read_stamp(frame: &[u8], width: usize, height: usize) -> Option<u64> {
    if !stamp_fits(width, height) || frame.len() < width * height * 4 {
        return None;
    }
    let mut code = 0u32;
    for cell in 0..STAMP_CELLS {
        // The middle of the cell: what is smeared at its edges stays out.
        let mut sum = 0u32;
        for y in STAMP_TOP + 3..STAMP_TOP + STAMP_CELL - 3 {
            for x in STAMP_LEFT + cell * STAMP_CELL + 3..STAMP_LEFT + (cell + 1) * STAMP_CELL - 3 {
                let at = (y * width + x) * 4;
                sum += (u32::from(frame[at])
                    + 2 * u32::from(frame[at + 1])
                    + u32::from(frame[at + 2]))
                    / 4;
            }
        }
        let light = sum / 36 >= 128;
        match cell {
            0 if !light => return None,
            1 if light => return None,
            0 | 1 => {}
            _ => code = code << 1 | u32::from(light),
        }
    }
    let rows = u64::from(code >> 8);
    (stamp_code(rows) == code).then_some(rows)
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
    paint_square(frame, width, height, shade);
}

/// The square in the middle: a rim of paper, and the shade inside it.
fn paint_square(frame: &mut [u32], width: usize, height: usize, shade: Shade) {
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

/// Paints one frame of the hard picture after it has moved `rows` rows, and
/// over it the square with the shade.
///
/// Two layers of the sheet lie on each other, three parts of one to one of
/// the other. The first moves up, as the text does; the second moves down
/// and sideways, slower. Over both lies a grain that is new in every frame.
/// The sheet has to be [`Sheet::photo`] for a frame of this size.
pub fn paint_hard(
    frame: &mut [u32],
    width: usize,
    height: usize,
    sheet: &Sheet,
    rows: u64,
    shade: Shade,
) {
    // Counted in 32 bits, as the program that draws on the GPU counts.
    let moved = rows as u32;
    let (across, down) = (sheet.width, sheet.height);
    let up = (moved % down as u32) as usize;
    let back = ((moved.wrapping_mul(2) / 5) % down as u32) as usize;
    let along = ((moved / 2) % across as u32) as usize;
    for (y, row) in frame.chunks_exact_mut(width).take(height).enumerate() {
        let first = sheet.row((y + up) % down);
        let second = sheet.row((y + down - back + down / 3) % down);
        for (x, pixel) in row.iter_mut().enumerate() {
            let other = second[(x + across / 2 + along) % across];
            *pixel = hard_pixel(first[x], other, x as u32, y as u32, moved);
        }
    }
    paint_square(frame, width, height, shade);
}

/// One pixel of the hard picture, from the pixels of the two layers under
/// it, where it is and how far the picture has moved. Whole numbers only:
/// the shader that draws the picture on the GPU does the same sums.
pub fn hard_pixel(first: u32, second: u32, x: u32, y: u32, moved: u32) -> u32 {
    let mut h = x
        .wrapping_mul(374_761_393)
        .wrapping_add(y.wrapping_mul(668_265_263))
        .wrapping_add(moved.wrapping_mul(2_246_822_519));
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^= h >> 16;
    let grain = (h % 5) as i32 - 2;
    let channel = |shift: u32| {
        let mixed = (3 * (first >> shift & 0xFF) + (second >> shift & 0xFF) + 2) / 4;
        (mixed as i32 + grain).clamp(0, 255) as u32
    };
    channel(16) << 16 | channel(8) << 8 | channel(0)
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
    use crate::flash::classify;
    use std::collections::BTreeSet;

    fn bytes(frame: &[u32]) -> Vec<u8> {
        frame.iter().flat_map(|pixel| pixel.to_le_bytes()).collect()
    }

    #[test]
    fn a_stamp_says_how_far_the_picture_has_moved() {
        let (width, height) = (640, 200);
        for rows in [0u64, 1, 2, 255, 256, 4097, 0x00AB_CDEF, 0x00FF_FFFF] {
            let mut frame = vec![pixel(PAPER); width * height];
            paint_stamp(&mut frame, width, height, rows);
            assert_eq!(
                read_stamp(&bytes(&frame), width, height),
                Some(rows),
                "{rows}"
            );
        }
        // Only 24 bits are written: after that the count starts again.
        let mut frame = vec![pixel(PAPER); width * height];
        paint_stamp(&mut frame, width, height, 0x0100_0005);
        assert_eq!(read_stamp(&bytes(&frame), width, height), Some(5));
    }

    #[test]
    fn a_stamp_is_read_through_blur_and_noise_and_refused_when_it_does_not_fit() {
        let (width, height) = (640, 200);
        let mut frame = vec![pixel(PAPER); width * height];
        paint_stamp(&mut frame, width, height, 123_456);
        let sharp = bytes(&frame);
        // Smeared over five pixels each way and lifted a little, as a key frame squeezed hard is.
        let mut blurred = sharp.clone();
        for y in 2..height - 2 {
            for x in 2..width - 2 {
                for channel in 0..3 {
                    let mut sum = 0u32;
                    for dy in 0..5 {
                        for dx in 0..5 {
                            sum +=
                                u32::from(sharp[((y + dy - 2) * width + x + dx - 2) * 4 + channel]);
                        }
                    }
                    blurred[(y * width + x) * 4 + channel] = (sum / 25 + 6).min(255) as u8;
                }
            }
        }
        assert_eq!(read_stamp(&blurred, width, height), Some(123_456));

        // No stamp at all: the first cell is not light.
        let plain = bytes(&vec![pixel(PAPER); width * height]);
        assert_eq!(read_stamp(&plain, width, height), None);
        // One cell of the number turned over: the last eight cells no longer fit it.
        let mut wrong = sharp.clone();
        let cell = 10;
        for y in STAMP_TOP..STAMP_TOP + STAMP_CELL {
            for x in STAMP_LEFT + cell * STAMP_CELL..STAMP_LEFT + (cell + 1) * STAMP_CELL {
                for channel in 0..3 {
                    wrong[(y * width + x) * 4 + channel] ^= 0xE0;
                }
            }
        }
        assert_eq!(read_stamp(&wrong, width, height), None);
        // A picture too small for a stamp gets none and gives none.
        let mut small = vec![pixel(PAPER); 300 * 100];
        paint_stamp(&mut small, 300, 100, 77);
        assert!(small.iter().all(|p| *p == pixel(PAPER)));
        assert_eq!(read_stamp(&bytes(&small), 300, 100), None);
    }

    #[test]
    fn the_hard_picture_is_the_same_every_time_and_never_stands_still() {
        let (width, height) = (320, 200);
        let sheet = Sheet::photo(width, height);
        assert!(sheet.fits_photo(width, height) && !sheet.fits(width, height, 2));
        assert_eq!(sheet.height(), 400);
        assert_eq!(Sheet::photo(width, height).pixels(), sheet.pixels());
        // Something like a photograph: many colours, light and dark.
        let colours: BTreeSet<u32> = sheet.pixels().iter().copied().collect();
        assert!(colours.len() > 2000, "{} colours", colours.len());
        let greens: Vec<u32> = sheet.pixels().iter().map(|p| (p >> 8) & 0xFF).collect();
        let (darkest, lightest) = (greens.iter().min().unwrap(), greens.iter().max().unwrap());
        assert!(lightest - darkest > 100, "{darkest} to {lightest}");

        let paint = |rows: u64, shade: Shade| {
            let mut frame = vec![0u32; width * height];
            paint_hard(&mut frame, width, height, &sheet, rows, shade);
            frame
        };
        assert_eq!(paint(700, Shade::Dark), paint(700, Shade::Dark));
        // From one row to the next nearly every pixel changes, and not by moving along.
        let (before, after) = (paint(700, Shade::Dark), paint(701, Shade::Dark));
        let changed = before.iter().zip(&after).filter(|(a, b)| a != b).count();
        assert!(
            changed > width * height * 7 / 10,
            "{changed} pixels changed"
        );
        let shifted = before[width..]
            .iter()
            .zip(&after[..width * (height - 1)])
            .filter(|(a, b)| a == b)
            .count();
        assert!(
            shifted < width * height / 2,
            "{shifted} pixels only moved up"
        );
        // The middle shows the shade, as in the other pictures.
        let middle = |frame: &[u32]| {
            let p = frame[(height / 2) * width + width / 2];
            Rgb::new((p >> 16) as u8, (p >> 8) as u8, p as u8)
        };
        assert_eq!(
            classify(middle(&paint(700, Shade::Dark))),
            Some(Shade::Dark)
        );
        assert_eq!(
            classify(middle(&paint(700, Shade::Light))),
            Some(Shade::Light)
        );
    }

    #[test]
    fn the_grain_of_the_hard_picture_stays_within_two_levels() {
        for (x, y, moved) in [(0u32, 0u32, 0u32), (17, 4, 9), (3439, 1439, 4_000_000_000)] {
            let out = hard_pixel(0x0080_8080, 0x0080_8080, x, y, moved);
            for shift in [16, 8, 0] {
                let value = (out >> shift & 0xFF) as i32;
                assert!((126..=130).contains(&value), "{value}");
            }
        }
        // Three parts of the first layer to one of the second, and never out of range.
        let dark = hard_pixel(0, 0, 5, 5, 5);
        let light = hard_pixel(0x00FF_FFFF, 0x00FF_FFFF, 5, 5, 5);
        assert!(dark & 0xFF <= 2 && light & 0xFF >= 253);
        let mixed = hard_pixel(0x0000_00C8, 0x0000_0000, 1, 2, 3) & 0xFF;
        assert!((148..=152).contains(&mixed), "{mixed}");
    }

    #[test]
    fn moved_counts_on_where_scrolled_starts_again() {
        assert_eq!(moved(1_000_000_000, 240), 240);
        assert_eq!(moved(60_000_000_000, 240), 14_400);
        assert_eq!(scrolled(60_000_000_000, 240, 1000), 400);
        assert_eq!(moved(5_000_000_000, 0), 0);
    }

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
