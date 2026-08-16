//! 1bpp frame buffer and drawing primitives, independent of the EPD driver.

use crate::font;
use crate::font8x16;

pub const WIDTH: usize = 400;
pub const HEIGHT: usize = 300;
const BYTES_PER_ROW: usize = WIDTH / 8;
const FRAME_SIZE: usize = BYTES_PER_ROW * HEIGHT;

#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

pub struct Canvas {
    frame: Vec<u8>,
}

impl Canvas {
    pub fn new() -> Self {
        Self {
            frame: vec![0xFF; FRAME_SIZE],
        }
    }

    pub fn frame(&self) -> &[u8] {
        &self.frame
    }

    pub fn clear(&mut self) {
        self.frame.fill(0xFF);
    }

    fn pixel_is_white(&self, x: usize, y: usize) -> bool {
        let index = y * BYTES_PER_ROW + x / 8;
        let mask = 1 << (7 - (x & 7));
        self.frame[index] & mask != 0
    }

    /// Packs the pixels inside `rect` into the row-padded 1bpp format the
    /// EPD partial-refresh API expects.
    pub fn pack_rect(&self, rect: Rect) -> Vec<u8> {
        let row_bytes = (rect.width as usize).div_ceil(8);
        let mut packed = vec![0u8; row_bytes * rect.height as usize];
        for (row, y) in (rect.y..rect.y + rect.height).enumerate() {
            for (column, x) in (rect.x..rect.x + rect.width).enumerate() {
                if self.pixel_is_white(x as usize, y as usize) {
                    packed[row * row_bytes + column / 8] |= 1 << (7 - (column & 7));
                }
            }
        }
        packed
    }

    pub fn set_pixel(&mut self, x: usize, y: usize, black: bool) {
        if x >= WIDTH || y >= HEIGHT {
            return;
        }
        let index = y * BYTES_PER_ROW + x / 8;
        let mask = 1 << (7 - (x & 7));
        if black {
            self.frame[index] &= !mask;
        } else {
            self.frame[index] |= mask;
        }
    }

    pub fn fill_rect(&mut self, x: usize, y: usize, width: usize, height: usize, black: bool) {
        for yy in y..y.saturating_add(height) {
            for xx in x..x.saturating_add(width) {
                self.set_pixel(xx, yy, black);
            }
        }
    }

    pub fn draw_text(&mut self, x: usize, y: usize, scale: usize, text: &str) {
        self.draw_text_ink(x, y, scale, text, true);
    }

    /// Draws text with white ink instead of black - e.g. a label sitting on
    /// top of a filled-black highlight bar (see [`Canvas::fill_rect`]).
    /// Not called anywhere yet now that provision.rs uses the proportional
    /// font's white variant instead - kept for any 5x7 use that comes up.
    #[allow(dead_code)]
    pub fn draw_text_white(&mut self, x: usize, y: usize, scale: usize, text: &str) {
        self.draw_text_ink(x, y, scale, text, false);
    }

    fn draw_text_ink(&mut self, x: usize, y: usize, scale: usize, text: &str, black: bool) {
        let mut cursor = x;
        for character in text.chars() {
            let glyph = font::glyph(character);
            for (row, bits) in glyph.iter().enumerate() {
                for column in 0..font::GLYPH_WIDTH {
                    if bits & (1 << (font::GLYPH_WIDTH - 1 - column)) != 0 {
                        self.fill_rect(
                            cursor + column * scale,
                            y + row * scale,
                            scale,
                            scale,
                            black,
                        );
                    }
                }
            }
            cursor += font::ADVANCE * scale;
        }
    }

    /// Draws text with the proportional-width 8x16 font (`font8x16.rs`) -
    /// more legible than the fixed 5x7 grid `draw_text` uses, at the cost
    /// of a bigger glyph. Returns the pixel width drawn, so callers that
    /// need to right-align or center text don't have to duplicate the
    /// advance-width math.
    pub fn draw_text_prop(&mut self, x: usize, y: usize, scale: usize, text: &str) -> usize {
        self.draw_text_prop_ink(x, y, scale, text, true)
    }

    /// White-ink version of [`Canvas::draw_text_prop`] - see
    /// [`Canvas::draw_text_white`] for when you'd want this.
    pub fn draw_text_prop_white(&mut self, x: usize, y: usize, scale: usize, text: &str) -> usize {
        self.draw_text_prop_ink(x, y, scale, text, false)
    }

    /// Pixel width `text` would occupy if drawn with [`Canvas::draw_text_prop`]
    /// at `scale`, without drawing anything - for right-aligning/centering
    /// before the content is known to fit.
    pub fn text_prop_width(text: &str, scale: usize) -> usize {
        text.chars()
            .map(|c| (font8x16::glyph(c).1 as usize + 1) * scale)
            .sum()
    }

    fn draw_text_prop_ink(
        &mut self,
        x: usize,
        y: usize,
        scale: usize,
        text: &str,
        black: bool,
    ) -> usize {
        let mut cursor = x;
        for character in text.chars() {
            let (rows, width) = font8x16::glyph(character);
            for (row, bits) in rows.iter().enumerate() {
                for column in 0..width as usize {
                    if bits & (1 << (15 - column)) != 0 {
                        self.fill_rect(
                            cursor + column * scale,
                            y + row * scale,
                            scale,
                            scale,
                            black,
                        );
                    }
                }
            }
            cursor += (width as usize + 1) * scale;
        }
        cursor - x
    }
}
