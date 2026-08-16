mod audio;
mod board;
mod button;
mod canvas;
mod display;
mod font;
mod nfc;
mod power;
mod provision;
mod rtc;
mod storage;
mod watchdog;
mod wifi;

use std::thread;
use std::time::Duration;

use anyhow::Result;
use board::Note4Board;
use button::{ButtonEvent, POLL_INTERVAL_MS};
use canvas::Rect;
use display::ButtonCounts;
use rtc::DateTime;
use storage::PersistedCounters;

/// Save the counters to NVS when at least this many idle polling cycles have
/// elapsed since the last key event. 50 cycles × 20 ms = 1 s of quiet.
const COUNTER_SAVE_IDLE_POLLS: u32 = 50;

/// Poll cycles between two consecutive `power status` log lines.
/// 50 × 20 ms = 1 s.
const STATUS_REPORT_INTERVAL_POLLS: u32 = 50;

/// Poll cycles between two consecutive PCF8563 re-reads.
/// 60 × 20 ms = 1.2 s, fast enough that the on-screen seconds tick at least
/// once per refresh.
const CLOCK_POLL_INTERVAL_POLLS: u32 = 60;

/// Bounding rect for the date/time + RTC status line drawn by
/// `display::EpdDisplay::draw_clock`. Matches `display.rs` (date 20,4
/// scale 1; time 20,14 scale 2; status 260,8 scale 1) with margin.
const CLOCK_RECT: Rect = Rect {
    x: 16,
    y: 0,
    width: 304,
    height: 32,
};

/// Hold DOWN for this many poll cycles (× 20 ms) to enter deep sleep.
/// 150 × 20 ms = 3 s of continuous press, intentionally longer than the
/// 1 s "long press" already used for full-refresh / clean-ghosting so the
/// two gestures don't collide.
const DEEP_SLEEP_HOLD_POLLS: u32 = 150;

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
    if let Err(err) = watchdog::subscribe() {
        log::warn!("Task watchdog subscribe failed: {err}");
    }
    let woke_from_deep_sleep = power::log_wakeup_cause();
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

    // Wi-Fi/NTP resync is only needed when the RTC time cannot be trusted:
    // first boot after flashing, a real power-on reset, or the PCF8563
    // reporting its battery was lost. A wake from deep sleep with a healthy
    // RTC should stay off Wi-Fi so ENTER responds immediately.
    let mut needs_wifi_sync = !woke_from_deep_sleep;
    let mut clock = match board.rtc.read_time() {
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
                needs_wifi_sync = true;
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
            needs_wifi_sync = true;
            None
        }
    };

    board.display.render_with_time(&counts, clock.as_ref());
    board.display.refresh_full()?;
    log::info!("Initial display refresh completed");

    // TEMPORARY hardware bring-up check for the ES8311 codec: play a short
    // beep once at boot so it's audible without needing a dedicated gesture.
    // Remove or gate this behind a real trigger once confirmed working -
    // nobody wants a mandatory startup chime forever.
    match board.audio.as_mut() {
        Some(codec) => {
            // Two clearly distinct pitches with a gap between them: makes it
            // easy to tell "clean tone, pitch changed" from "hiss/noise, no
            // discernible pitch change" when listening. Streamed in small
            // chunks rather than one big buffer - see play_sine_stereo's
            // doc comment for why.
            let result = codec
                .play_sine_stereo(440.0, 0.6, 10000)
                .and_then(|()| {
                    thread::sleep(Duration::from_millis(250));
                    codec.play_sine_stereo(880.0, 0.6, 10000)
                });
            match result {
                Ok(()) => log::info!("ES8311 bring-up tone played"),
                Err(err) => log::warn!("ES8311 bring-up tone failed: {err}"),
            }
        }
        None => log::warn!("ES8311 not available; skipping bring-up tone"),
    }

    // TEMPORARY hardware bring-up check for the GT23SC6699 NFC tag: read the
    // UID block and log the field-detect pin once at boot. Move this behind
    // a real trigger (or drop it) once confirmed working.
    match board.nfc.as_mut() {
        Some(tag) => match tag.read_uid() {
            Ok(uid) => log::info!(
                "NFC UID: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X} field_present={}",
                uid[0],
                uid[1],
                uid[2],
                uid[3],
                uid[4],
                uid[5],
                uid[6],
                tag.field_present()
            ),
            Err(err) => log::warn!("NFC UID read failed: {err}"),
        },
        None => log::warn!("NFC not available; skipping bring-up check"),
    }

    report_power_state(&mut board)?;

    // Taken once up front (it's a singleton) so both the boot-time Wi-Fi
    // bring-up below and the on-device Wi-Fi setup wizard (triggered from
    // the main loop by holding UP) can use it.
    let sysloop = esp_idf_svc::eventloop::EspSystemEventLoop::take()?;

    // Optional Wi-Fi bring-up: connect with credentials stored in NVS
    // (`wifi_ssid` / `wifi_pass`), then sync the clock over NTP and push the
    // time into the PCF8563 so it keeps ticking while the device sleeps.
    // Failure to connect or sync only logs a warning; the rest of the UI
    // keeps working regardless. Skipped on a deep-sleep wake with a healthy
    // RTC so ENTER wakes the device up instantly instead of blocking on the
    // network for several seconds.
    let _wifi_sta = if !needs_wifi_sync {
        log::info!("Woke from deep sleep with a healthy RTC; skipping Wi-Fi/NTP resync");
        None
    } else {
        match counters.wifi_creds() {
            Ok(Some(creds)) => match wifi::WifiSta::connect(&creds, &sysloop) {
                Ok(sta) => {
                    match wifi::ntp_sync_and_set_rtc(&mut board.rtc) {
                        Ok(()) => match board.rtc.read_time() {
                            Ok(dt) => {
                                clock = Some(dt);
                                board.display.render_with_time(&counts, clock.as_ref());
                                board.display.refresh_partial(CLOCK_RECT)?;
                                log::info!("Clock region refreshed after NTP sync");
                            }
                            Err(err) => {
                                log::warn!("PCF8563 read_time after NTP sync failed: {err}")
                            }
                        },
                        Err(err) => log::warn!("NTP sync failed: {err}"),
                    }
                    Some(sta)
                }
                Err(err) => {
                    log::warn!("Wi-Fi connect failed: {err}");
                    None
                }
            },
            Ok(None) => {
                log::info!(
                    "No Wi-Fi credentials in NVS; skipping connect (see scripts/gen-nvs-wifi.py)"
                );
                None
            }
            Err(err) => {
                log::warn!("Could not read Wi-Fi credentials from NVS: {err}");
                None
            }
        }
    };

    let mut led_on = false;
    let mut led_tick = 0u32;
    let mut status_tick = 0u32;
    let mut clock_tick = 0u32;
    let mut idle_since_save = COUNTER_SAVE_IDLE_POLLS; // mark counters as saved on boot
    let mut down_held_polls: u32 = 0;
    let mut up_held_polls: u32 = 0;
    loop {
        watchdog::feed();

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

        clock_tick += 1;
        if clock_tick >= CLOCK_POLL_INTERVAL_POLLS {
            clock_tick = 0;
            match board.rtc.read_time() {
                Ok(dt) => {
                    let changed = clock
                        .as_ref()
                        .map(|prev| {
                            prev.second != dt.second
                                || prev.minute != dt.minute
                                || prev.hour != dt.hour
                        })
                        .unwrap_or(true);
                    if changed {
                        clock = Some(dt);
                        dirty.push(CLOCK_RECT);
                    }
                }
                Err(err) => log::warn!("PCF8563 read_time failed: {err}"),
            }
        }

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
                    up_held_polls = 1;
                }
                ButtonEvent::LongPressed => {
                    log::info!("UP long pressed; full refresh to clean ghosting");
                    full_refresh = true;
                    up_held_polls = up_held_polls.max(1);
                }
                ButtonEvent::Released => {
                    up_held_polls = 0;
                }
            }
        } else if up_held_polls > 0 {
            // Continue counting polls while UP stays low even if no new
            // button event fires this cycle.
            up_held_polls = up_held_polls.saturating_add(1);
        }

        if up_held_polls == provision::ENTER_HOLD_POLLS {
            log::info!("UP held for 3 s; entering Wi-Fi setup wizard");
            up_held_polls = 0;
            provision::run(&mut board, &counters, &sysloop);
            full_refresh = true;
        }
        if let Some(event) = board.key_down.poll() {
            match event {
                ButtonEvent::Pressed => {
                    counts.down = counts.down.saturating_add(1);
                    log::info!("DOWN pressed count={}", counts.down);
                    dirty.push(count_rect(255, 224, counts.down));
                    idle_since_save = 0;
                    down_held_polls = 1;
                }
                ButtonEvent::LongPressed => {
                    log::info!("DOWN long pressed; full refresh to clean ghosting");
                    full_refresh = true;
                    down_held_polls = down_held_polls.max(1);
                }
                ButtonEvent::Released => {
                    down_held_polls = 0;
                }
            }
        } else if down_held_polls > 0 {
            // Continue counting polls while DOWN stays low even if no new
            // button event fires this cycle.
            down_held_polls = down_held_polls.saturating_add(1);
        }

        if down_held_polls == DEEP_SLEEP_HOLD_POLLS {
            log::info!("DOWN held for 3 s; entering deep sleep");
            board.display.render_with_time(&counts, clock.as_ref());
            board.display.refresh_full()?;
            power::enter_deep_sleep_with_button_wake();
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
