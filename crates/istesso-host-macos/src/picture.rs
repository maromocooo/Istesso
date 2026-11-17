//! A picture to feed the encoder with when there is no window to capture:
//! lines of made-up text that scroll, so that every frame differs from the
//! one before, as a terminal or an editor does while it is used.
//!
//! It is not the test picture of `istesso-flash` (that crate is not in
//! `main` yet); it is the same kind of picture, drawn here.

/// Bytes per pixel: blue, green, red, alpha.
pub const BYTES_PER_PIXEL: usize = 4;

const GLYPH_COLUMNS: usize = 5;
const GLYPH_ROWS: usize = 7;
const SCALE: usize = 2;
const ADVANCE: usize = (GLYPH_COLUMNS + 1) * SCALE;
const LINE_PITCH: usize = (GLYPH_ROWS + 3) * SCALE;
const BACKGROUND: [u8; 4] = [0x1e, 0x1e, 0x1e, 0xff];
/// Mostly near-white text, with words in the colours an editor uses.
const INKS: [[u8; 4]; 6] = [
    [0xd4, 0xd4, 0xd4, 0xff],
    [0xd4, 0xd4, 0xd4, 0xff],
    [0xd4, 0xd4, 0xd4, 0xff],
    [0xd6, 0x9c, 0x56, 0xff],
    [0x78, 0x91, 0xce, 0xff],
    [0xb0, 0xc9, 0x4e, 0xff],
];

/// A page of text, taller than nothing and exactly as wide as the picture,
/// that frames are cut out of.
pub struct ScrollingText {
    width: usize,
    height: usize,
    page_rows: usize,
    page: Vec<u8>,
    /// Rows the text moves up from one frame to the next.
    pub step: usize,
}

struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        // xorshift64: enough to make glyphs that do not repeat.
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

impl ScrollingText {
    /// Draws the page once. `width` and `height` are the picture's size in
    /// pixels; both must be at least one line of text.
    pub fn new(width: usize, height: usize) -> Self {
        assert!(
            width >= ADVANCE && height >= LINE_PITCH,
            "too small for a line of text"
        );
        let page_rows = height.div_ceil(LINE_PITCH) * LINE_PITCH;
        let mut page = Vec::with_capacity(width * page_rows * BYTES_PER_PIXEL);
        for _ in 0..width * page_rows {
            page.extend_from_slice(&BACKGROUND);
        }
        let mut random = Random(0x9e37_79b9_7f4a_7c15);
        for line in 0..page_rows / LINE_PITCH {
            let top = line * LINE_PITCH + SCALE;
            let indent = (random.next() % 8) as usize * ADVANCE;
            let length =
                width / ADVANCE / 3 + (random.next() as usize % (width / ADVANCE / 2).max(1));
            let mut ink = INKS[0];
            let mut column = indent;
            for _ in 0..length {
                if column + ADVANCE > width {
                    break;
                }
                let bits = random.next();
                if bits % 6 == 0 {
                    // A space; the next word may have another colour.
                    ink = INKS[(bits >> 8) as usize % INKS.len()];
                } else {
                    Self::glyph(&mut page, width, column, top, bits >> 8, ink);
                }
                column += ADVANCE;
            }
        }
        ScrollingText {
            width,
            height,
            page_rows,
            page,
            step: 3,
        }
    }

    fn glyph(page: &mut [u8], width: usize, left: usize, top: usize, bits: u64, ink: [u8; 4]) {
        for row in 0..GLYPH_ROWS {
            for col in 0..GLYPH_COLUMNS {
                if (bits >> (row * GLYPH_COLUMNS + col)) & 1 == 0 {
                    continue;
                }
                for y in 0..SCALE {
                    for x in 0..SCALE {
                        let at = ((top + row * SCALE + y) * width + left + col * SCALE + x)
                            * BYTES_PER_PIXEL;
                        page[at..at + BYTES_PER_PIXEL].copy_from_slice(&ink);
                    }
                }
            }
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Writes frame number `frame` into `dst`, whose rows are `stride` bytes
    /// apart (at least `width * 4`).
    pub fn fill(&self, frame: u64, dst: &mut [u8], stride: usize) {
        let row_bytes = self.width * BYTES_PER_PIXEL;
        assert!(
            stride >= row_bytes,
            "rows closer together than a row is wide"
        );
        assert!(
            dst.len() >= stride * (self.height - 1) + row_bytes,
            "the buffer is too small"
        );
        let first = (frame as usize).wrapping_mul(self.step) % self.page_rows;
        for y in 0..self.height {
            let from = ((first + y) % self.page_rows) * row_bytes;
            dst[y * stride..y * stride + row_bytes]
                .copy_from_slice(&self.page[from..from + row_bytes]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(text: &ScrollingText, n: u64, stride: usize) -> Vec<u8> {
        let mut buffer = vec![0u8; stride * text.height()];
        text.fill(n, &mut buffer, stride);
        buffer
    }

    #[test]
    fn every_frame_differs_from_the_one_before() {
        let text = ScrollingText::new(320, 200);
        let stride = 320 * BYTES_PER_PIXEL;
        let mut before = frame(&text, 0, stride);
        for n in 1..40 {
            let now = frame(&text, n, stride);
            assert!(now != before, "frame {n} repeats frame {}", n - 1);
            before = now;
        }
    }

    #[test]
    fn the_picture_has_text_and_background() {
        let text = ScrollingText::new(320, 200);
        let buffer = frame(&text, 0, 320 * BYTES_PER_PIXEL);
        let background = buffer.chunks(4).filter(|p| *p == BACKGROUND).count();
        let all = 320 * 200;
        assert!(background > all / 2, "mostly background, as text is");
        assert!(background < all * 19 / 20, "but not empty");
    }

    #[test]
    fn padding_between_rows_is_left_alone() {
        let text = ScrollingText::new(64, 40);
        let stride = 64 * BYTES_PER_PIXEL + 16;
        let mut buffer = vec![0xabu8; stride * 40];
        text.fill(5, &mut buffer, stride);
        for y in 0..40 {
            let padding = &buffer[y * stride + 64 * BYTES_PER_PIXEL..(y + 1) * stride];
            assert!(padding.iter().all(|&b| b == 0xab));
        }
    }

    #[test]
    fn scrolling_wraps_around_the_page() {
        let text = ScrollingText::new(320, 200);
        let stride = 320 * BYTES_PER_PIXEL;
        // After as many frames as the page has rows, the text has moved by a whole
        // number of pages.
        assert!(frame(&text, 0, stride) == frame(&text, text.page_rows as u64, stride));
        assert!(frame(&text, 0, stride) != frame(&text, 1, stride));
    }
}
