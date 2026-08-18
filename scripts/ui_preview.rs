//! Dependency-free status-bar preview for the firmware's 1bpp icon tables.
//!
//! Run from the `inkpaper` repository root:
//!   rustc scripts/ui_preview.rs -o /tmp/ui-preview
//!   /tmp/ui-preview rust-firmware/src/icons.rs ui_preview.png

use std::{env, fs, io, path::Path};

const LOGICAL_WIDTH: usize = 400;
const LOGICAL_HEIGHT: usize = 30;
const SCALE: usize = 4;

#[derive(Debug)]
struct Icon {
    width: usize,
    rows: Vec<u32>,
}

fn parse_icon(source: &str, name: &str) -> Icon {
    let marker = format!("pub const {name}: Icon");
    let block = source
        .split_once(&marker)
        .unwrap_or_else(|| panic!("icon {} not found", name))
        .1
        .split_once("};")
        .expect("unterminated icon block")
        .0;
    let width = block
        .split_once("width:")
        .expect("missing width")
        .1
        .split(',')
        .next()
        .unwrap()
        .trim()
        .parse()
        .expect("invalid width");
    let row_text = block
        .split_once("rows: &[")
        .expect("missing rows")
        .1
        .split_once(']')
        .expect("unterminated rows")
        .0;
    let rows = row_text
        .split(',')
        .filter_map(|value| {
            let value = value.trim();
            (!value.is_empty()).then(|| {
                u32::from_str_radix(value.trim_start_matches("0x"), 16)
                    .expect("invalid bitmap row")
            })
        })
        .collect();
    Icon { width, rows }
}

fn draw_icon(pixels: &mut [u8], x: usize, y: usize, icon: &Icon) {
    for (row, bits) in icon.rows.iter().enumerate() {
        for col in 0..icon.width {
            if bits & (1 << (31 - col)) != 0 {
                pixels[(y + row) * LOGICAL_WIDTH + x + col] = 0;
            }
        }
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320u32 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for byte in bytes {
        a = (a + *byte as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    let mut checked = Vec::with_capacity(4 + data.len());
    checked.extend_from_slice(kind);
    checked.extend_from_slice(data);
    png.extend_from_slice(&crc32(&checked).to_be_bytes());
}

fn write_png(path: &Path, logical: &[u8]) -> io::Result<()> {
    let width = LOGICAL_WIDTH * SCALE;
    let height = LOGICAL_HEIGHT * SCALE;
    let mut raw = Vec::with_capacity((width + 1) * height);
    for y in 0..height {
        raw.push(0); // PNG filter: None
        for x in 0..width {
            raw.push(logical[(y / SCALE) * LOGICAL_WIDTH + x / SCALE]);
        }
    }

    // A zlib stream containing uncompressed DEFLATE blocks.
    let mut zlib = vec![0x78, 0x01];
    for (index, block) in raw.chunks(65_535).enumerate() {
        let final_block = index == raw.len().div_ceil(65_535) - 1;
        zlib.push(u8::from(final_block));
        let len = block.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]); // 8-bit grayscale
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    fs::write(path, png)
}

fn main() -> io::Result<()> {
    let args: Vec<_> = env::args().collect();
    let source_path = args.get(1).map(String::as_str).unwrap_or("rust-firmware/src/icons.rs");
    let output_path = args.get(2).map(String::as_str).unwrap_or("ui_preview.png");
    let source = fs::read_to_string(source_path)?;
    let wifi = parse_icon(&source, "WIFI");
    let battery = parse_icon(&source, "BATTERY_HIGH");

    let mut pixels = vec![255; LOGICAL_WIDTH * LOGICAL_HEIGHT];
    let battery_x = 384 - battery.width;
    let battery_y = 7;
    let wifi_x = battery_x - 8 - wifi.width;
    let wifi_y = battery_y + battery.rows.len() - wifi.rows.len();
    draw_icon(&mut pixels, battery_x, battery_y, &battery);
    draw_icon(&mut pixels, wifi_x, wifi_y, &wifi);
    for x in 16..384 {
        pixels[29 * LOGICAL_WIDTH + x] = 0;
    }

    write_png(Path::new(output_path), &pixels)?;
    println!("wrote {output_path} ({wifi_x},{wifi_y}) wifi; ({battery_x},{battery_y}) battery");
    Ok(())
}
