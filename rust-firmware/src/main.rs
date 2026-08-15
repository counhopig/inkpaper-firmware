mod board;
mod button;
mod display;
mod rtc;
mod storage;

use std::thread;
use std::time::Duration;

use anyhow::Result;
use board::Note4Board;
use button::{ButtonEvent, POLL_INTERVAL_MS};
use display::{ButtonCounts, Rect};
use rtc::DateTime;
use storage::PersistedCounters;

/// Save the counters to NVS when at least this many idle polling cycles have
/// elapsed since the last key event. 50 cycles × 20 ms = 1 s of quiet.
const COUNTER_SAVE_IDLE_POLLS: u32 = 50;

/// Poll cycles between two consecutive `power status` log lines.
/// 50 × 20 ms = 1 s.
const STATUS_REPORT_INTERVAL_POLLS: u32 = 50;

/// Epoch seconds captured at firmware build time. Used as the fallback RTC
/// seed when PCF8563 reports `voltage_low = true` (battery was disconnected
/// or drained). Captured by `build.rs` so a rebuild refreshes the value;
/// once the RTC keeps time on its coin cell we stop consulting this.
const BUILD_EPOCH_SECS: u64 = build_epoch_secs();

#[allow(dead_code)]
const fn build_epoch_secs() -> u64 {
    let bytes: &[u8] = env!("BUILD_EPOCH_SECS").as_bytes();
    let mut i = 0;
    let mut value: u64 = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if !b.is_ascii_digit() {
            break;
        }
        value = value * 10 + (b - b'0') as u64;
        i += 1;
    }
    value
}

fn main() -> Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    log::info!("Inkpaper NOTE4 Rust bring-up starting");
    let mut board = Note4Board::take()?;
    log::info!("Power latch is high; rendering Hello world");

    let counters = PersistedCounters::open()?;
    let mut counts = match counters.load() {
        Ok(loaded) => {
            log::info!(
                "Loaded persisted counters: enter={} up={} down={}",
                loaded.enter,
                loaded.up,
                loaded.down
            );
            loaded
        }
        Err(err) => {
            log::warn!("Could not load persisted counters ({err}); starting from zero");
            ButtonCounts {
                enter: 0,
                up: 0,
                down: 0,
            }
        }
    };

    let clock = match board.rtc.read_time() {
        Ok(mut dt) => {
            log::info!(
                "PCF8563: {:04}-{:02}-{:02} {:02}:{:02}:{:02} vl={}",
                dt.year,
                dt.month,
                dt.day,
                dt.hour,
                dt.minute,
                dt.second,
                dt.voltage_low
            );
            if dt.voltage_low {
                log::warn!("PCF8563 VL set (RTC battery low/lost); reseeding from build time");
                let seeded = DateTime::from_unix(BUILD_EPOCH_SECS);
                if let Err(err) = board.rtc.write_time(&seeded) {
                    log::warn!("PCF8563 reseed failed: {err}");
                } else {
                    dt = seeded;
                }
            }
            Some(dt)
        }
        Err(err) => {
            log::warn!("PCF8563 read_time failed: {err}");
            None
        }
    };

    board.display.render_with_time(&counts, clock.as_ref());
    board.display.refresh_full()?;
    log::info!("Initial display refresh completed");

    report_power_state(&mut board)?;

    let mut led_on = false;
    let mut led_tick = 0u32;
    let mut status_tick = 0u32;
    let mut idle_since_save = COUNTER_SAVE_IDLE_POLLS; // mark counters as saved on boot
    loop {
        led_tick += 1;
        if led_tick >= 12 {
            led_tick = 0;
            led_on = !led_on;
            board.set_led(led_on)?;
        }

        status_tick += 1;
        if status_tick >= STATUS_REPORT_INTERVAL_POLLS {
            status_tick = 0;
            if let Err(err) = report_power_state(&mut board) {
                log::warn!("Power status probe failed: {err}");
            }
        }

        let mut dirty: Vec<Rect> = Vec::new();
        let mut full_refresh = false;

        if let Some(event) = board.key_enter.poll() {
            match event {
                ButtonEvent::Pressed => {
                    counts.enter = counts.enter.saturating_add(1);
                    log::info!("ENTER pressed count={}", counts.enter);
                    dirty.push(count_rect(255, 108, counts.enter));
                    idle_since_save = 0;
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
                    idle_since_save = 0;
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
                    idle_since_save = 0;
                }
                ButtonEvent::LongPressed => {
                    log::info!("DOWN long pressed; full refresh to clean ghosting");
                    full_refresh = true;
                }
                ButtonEvent::Released => {}
            }
        }

        if full_refresh {
            board.display.render_with_time(&counts, clock.as_ref());
            board.display.refresh_full()?;
            log::info!("Full display refresh completed");
        } else if !dirty.is_empty() {
            board.display.render_with_time(&counts, clock.as_ref());
            for rect in &dirty {
                board.display.refresh_partial(*rect)?;
            }
            log::info!("Partial display refresh completed");
        }

        if idle_since_save < COUNTER_SAVE_IDLE_POLLS {
            idle_since_save += 1;
            if idle_since_save == COUNTER_SAVE_IDLE_POLLS {
                match counters.save(&counts) {
                    Ok(()) => log::info!(
                        "Saved counters to NVS: enter={} up={} down={}",
                        counts.enter,
                        counts.up,
                        counts.down
                    ),
                    Err(err) => log::warn!("Failed to persist counters: {err}"),
                }
            }
        }

        thread::sleep(Duration::from_millis(POLL_INTERVAL_MS as u64));
    }
}

fn report_power_state(board: &mut Note4Board) -> Result<()> {
    let (charging, charge_done) = board.charging_state();
    match board.battery_millivolts() {
        Ok(vbat_mv) => log::info!(
            "Power state: charging={} charge_done={} vbat_mV={}",
            charging,
            charge_done,
            vbat_mv
        ),
        Err(err) => {
            log::warn!("Battery ADC read failed: {err}");
            log::info!(
                "Power state: charging={} charge_done={} vbat_mV=<n/a>",
                charging, charge_done
            );
        }
    }
    Ok(())
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
