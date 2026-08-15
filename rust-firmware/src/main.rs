mod board;
mod button;
mod display;

use std::thread;
use std::time::Duration;

use anyhow::Result;
use board::Note4Board;
use button::{ButtonEvent, POLL_INTERVAL_MS};
use display::{ButtonCounts, Rect};

fn main() -> Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    log::info!("Inkpaper NOTE4 Rust bring-up starting");
    let mut board = Note4Board::take()?;
    log::info!("Power latch is high; rendering Hello world");

    let mut counts = ButtonCounts {
        enter: 0,
        up: 0,
        down: 0,
    };
    board.display.render(&counts);
    board.display.refresh_full()?;
    log::info!("Initial display refresh completed");

    let (charging, charge_done) = board.charging_state();
    log::info!(
        "Power state: charging={} charge_done={}",
        charging,
        charge_done
    );

    let mut led_on = false;
    let mut led_tick = 0u32;
    loop {
        led_tick += 1;
        if led_tick >= 12 {
            led_tick = 0;
            led_on = !led_on;
            board.set_led(led_on)?;
        }

        let mut dirty: Vec<Rect> = Vec::new();
        let mut full_refresh = false;

        if let Some(event) = board.key_enter.poll() {
            match event {
                ButtonEvent::Pressed => {
                    counts.enter = counts.enter.saturating_add(1);
                    log::info!("ENTER pressed count={}", counts.enter);
                    dirty.push(count_rect(255, 108, counts.enter));
                }
                ButtonEvent::LongPressed => {
                    log::info!("ENTER long pressed; full refresh to clean ghosting");
                    full_refresh = true;
                }
                ButtonEvent::Released => {}
            }
        }
        if let Some(event) = board.key_up.poll() {
            match event {
                ButtonEvent::Pressed => {
                    counts.up = counts.up.saturating_add(1);
                    log::info!("UP pressed count={}", counts.up);
                    dirty.push(count_rect(255, 166, counts.up));
                }
                ButtonEvent::LongPressed => {
                    log::info!("UP long pressed; full refresh to clean ghosting");
                    full_refresh = true;
                }
                ButtonEvent::Released => {}
            }
        }
        if let Some(event) = board.key_down.poll() {
            match event {
                ButtonEvent::Pressed => {
                    counts.down = counts.down.saturating_add(1);
                    log::info!("DOWN pressed count={}", counts.down);
                    dirty.push(count_rect(255, 224, counts.down));
                }
                ButtonEvent::LongPressed => {
                    log::info!("DOWN long pressed; full refresh to clean ghosting");
                    full_refresh = true;
                }
                ButtonEvent::Released => {}
            }
        }

        if full_refresh {
            board.display.render(&counts);
            board.display.refresh_full()?;
            log::info!("Full display refresh completed");
        } else if !dirty.is_empty() {
            board.display.render(&counts);
            for rect in &dirty {
                board.display.refresh_partial(*rect)?;
            }
            log::info!("Partial display refresh completed");
        }

        thread::sleep(Duration::from_millis(POLL_INTERVAL_MS as u64));
    }
}

fn count_rect(x: u16, y: u16, count: u32) -> Rect {
    let digits = digit_count(count);
    Rect {
        x: x.saturating_sub(6),
        y: y.saturating_sub(3),
        width: digits.saturating_mul(18) + 12,
        height: 27,
    }
}

fn digit_count(mut n: u32) -> u16 {
    if n == 0 {
        return 1;
    }
    let mut digits = 0;
    while n > 0 {
        digits += 1;
        n /= 10;
    }
    digits
}
