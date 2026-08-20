//! 1bpp frame buffer and drawing primitives, independent of the EPD driver.

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

    /// Draws a crisp rectangular outline without filling its interior.
    pub fn stroke_rect(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        thickness: usize,
    ) {
        if width == 0 || height == 0 || thickness == 0 {
            return;
        }
        let t = thickness.min(width).min(height);
        self.fill_rect(x, y, width, t, true);
        self.fill_rect(x, y + height.saturating_sub(t), width, t, true);
        self.fill_rect(x, y, t, height, true);
        self.fill_rect(x + width.saturating_sub(t), y, t, height, true);
    }

    /// Draws text with the proportional-width 8x16 font (`font8x16.rs`) -
    /// more legible than the fixed 5x7 grid `draw_text` uses, at the cost
    /// of a bigger glyph. Returns the pixel width drawn, so callers that
    /// need to right-align or center text don't have to duplicate the
    /// advance-width math.
    pub fn draw_text_prop(&mut self, x: usize, y: usize, scale: usize, text: &str) -> usize {
        self.draw_text_prop_ink(x, y, scale, text, true)
    }

    /// Draws text with the tiny 5x7 font (`font5x7.rs`) at scale 1. Used for
    /// dense columnar layouts (week view) where 16px type is too tall.
    pub fn draw_text_small(&mut self, x: usize, y: usize, text: &str) -> usize {
        let mut cursor = x;
        for character in text.chars() {
            let (rows, width) = crate::font5x7::glyph5x7(character);
            for (row, bits) in rows.iter().enumerate() {
                for column in 0..width as usize {
                    if bits & (1 << (4 - column)) != 0 {
                        self.set_pixel(cursor + column, y + row, true);
                    }
                }
            }
            cursor += width as usize + 1;
        }
        cursor - x
    }

    /// Pixel width `text` would occupy if drawn with
    /// [`Canvas::draw_text_small`], without drawing anything.
    pub fn text_small_width(text: &str) -> usize {
        text.chars()
            .map(|c| (crate::font5x7::glyph5x7(c).1 as usize) + 1)
            .sum()
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
