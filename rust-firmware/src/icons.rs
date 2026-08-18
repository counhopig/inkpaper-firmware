//! Header status icon bitmaps, hand-tuned for the 1bpp EPD. Row bits are
//! left-aligned in a `u32` (bit 31 = leftmost column), mirroring
//! `font8x16.rs`'s row-bitmask convention but wider since these run up to
//! 18px - a text glyph's `u16` doesn't have room. No font file is read at
//! runtime; these are plain compiled-in constants.
//!
//! Shapes (generated once from scripts, not from any font):
//!
//! - Wi-Fi: a native 18x18 three-bar cellular-style glyph, bottom-aligned
//!   like the battery fill. Four-pixel bars with three-pixel gaps read
//!   cleanly at 1bpp; the arcs of the old Font-Awesome-style glyph turned
//!   to stair-step noise at this size.
//! - Battery: 12x18 outline with a top nub and a fill level that rises
//!   from the bottom (25/50/75/100%). Charging reuses the empty outline
//!   plus a centered bolt; the three charging variants share one shape
//!   since at this size the bolt itself carries the "charging" meaning
//!   and a fill bar would just obscure it.

use crate::canvas::Canvas;

pub struct Icon {
    pub width: u8,
    pub rows: &'static [u32],
}

/// Blits `icon` at `(x, y)`, one set bit per black pixel - the icon
/// equivalent of `Canvas::draw_text_prop`, but reading a bitmap row table
/// instead of `font8x16`'s glyph lookup.
pub fn draw_icon(canvas: &mut Canvas, x: usize, y: usize, icon: &Icon) {
    for (row, bits) in icon.rows.iter().enumerate() {
        for col in 0..icon.width as usize {
            if bits & (1 << (31 - col)) != 0 {
                canvas.set_pixel(x + col, y + row, true);
            }
        }
    }
}

pub const WIFI: Icon = Icon {
    width: 18,
    rows: &[
        0x0003c000, 0x0003c000, 0x0003c000, 0x0003c000, 0x0003c000, 0x0003c000, 0x01e3c000,
        0x01e3c000, 0x01e3c000, 0x01e3c000, 0x01e3c000, 0x01e3c000, 0xf1e3c000, 0xf1e3c000,
        0xf1e3c000, 0xf1e3c000, 0xf1e3c000, 0xf1e3c000,
    ],
};

pub const BATTERY_OUTLINE: Icon = Icon {
    width: 12,
    rows: &[
        0x1f800000, 0x1f800000, 0xfff00000, 0xc0300000, 0xc0300000, 0xc0300000, 0xc0300000,
        0xc0300000, 0xc0300000, 0xc0300000, 0xc0300000, 0xc0300000, 0xc0300000, 0xc0300000,
        0xc0300000, 0xc0300000, 0xc0300000, 0xfff00000,
    ],
};

pub const BATTERY_LOW: Icon = Icon {
    width: 12,
    rows: &[
        0x1f800000, 0x1f800000, 0xfff00000, 0xc0300000, 0xc0300000, 0xc0300000, 0xc0300000,
        0xc0300000, 0xc0300000, 0xc0300000, 0xc0300000, 0xc0300000, 0xc0300000, 0xfff00000,
        0xfff00000, 0xfff00000, 0xfff00000, 0xfff00000,
    ],
};

pub const BATTERY_MEDIUM: Icon = Icon {
    width: 12,
    rows: &[
        0x1f800000, 0x1f800000, 0xfff00000, 0xc0300000, 0xc0300000, 0xc0300000, 0xc0300000,
        0xc0300000, 0xc0300000, 0xc0300000, 0xfff00000, 0xfff00000, 0xfff00000, 0xfff00000,
        0xfff00000, 0xfff00000, 0xfff00000, 0xfff00000,
    ],
};

pub const BATTERY_HIGH: Icon = Icon {
    width: 12,
    rows: &[
        0x1f800000, 0x1f800000, 0xfff00000, 0xc0300000, 0xc0300000, 0xc0300000, 0xc0300000,
        0xfff00000, 0xfff00000, 0xfff00000, 0xfff00000, 0xfff00000, 0xfff00000, 0xfff00000,
        0xfff00000, 0xfff00000, 0xfff00000, 0xfff00000,
    ],
};

pub const BATTERY_FULL: Icon = Icon {
    width: 12,
    rows: &[
        0x1f800000, 0x1f800000, 0xfff00000, 0xfff00000, 0xfff00000, 0xfff00000, 0xfff00000,
        0xfff00000, 0xfff00000, 0xfff00000, 0xfff00000, 0xfff00000, 0xfff00000, 0xfff00000,
        0xfff00000, 0xfff00000, 0xfff00000, 0xfff00000,
    ],
};

pub const CHARGING_LOW: Icon = Icon {
    width: 12,
    rows: &[
        0x1f800000, 0x1f800000, 0xfff00000, 0xc0300000, 0xd8300000, 0xd8300000, 0xd8300000,
        0xcc300000, 0xcc300000, 0xcc300000, 0xc6300000, 0xc6300000, 0xc6300000, 0xc0300000,
        0xc0300000, 0xc0300000, 0xc0300000, 0xfff00000,
    ],
};

pub const CHARGING_MEDIUM: Icon = Icon {
    width: 12,
    rows: &[
        0x1f800000, 0x1f800000, 0xfff00000, 0xc0300000, 0xd8300000, 0xd8300000, 0xd8300000,
        0xcc300000, 0xcc300000, 0xcc300000, 0xc6300000, 0xc6300000, 0xc6300000, 0xc0300000,
        0xc0300000, 0xc0300000, 0xc0300000, 0xfff00000,
    ],
};

pub const CHARGING_HIGH: Icon = Icon {
    width: 12,
    rows: &[
        0x1f800000, 0x1f800000, 0xfff00000, 0xc0300000, 0xd8300000, 0xd8300000, 0xd8300000,
        0xcc300000, 0xcc300000, 0xcc300000, 0xc6300000, 0xc6300000, 0xc6300000, 0xc0300000,
        0xc0300000, 0xc0300000, 0xc0300000, 0xfff00000,
    ],
};
