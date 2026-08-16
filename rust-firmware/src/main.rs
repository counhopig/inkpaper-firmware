mod alarms;
mod audio;
mod ble_control;
mod board;
mod button;
mod canvas;
mod control;
mod display;
mod font;
mod font8x16;
mod nfc;
mod power;
mod provision;
mod rtc;
mod screens;
mod storage;
mod sync;
mod todos;
mod ui;
mod usb_console;
mod watchdog;
mod wifi;

use std::thread;
use std::time::Duration;

use alarms::AlarmStore;
use anyhow::Result;
use board::Note4Board;
use button::{ButtonEvent, POLL_INTERVAL_MS};
use canvas::Rect;
use esp_idf_svc::systime::EspSystemTime;
use rtc::DateTime;
use storage::PersistedCounters;
use todos::TodoStore;

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

/// Hold DOWN for this long (wall-clock, not poll cycles - a poll-cycle
/// counter under-counts whenever an iteration blocks for a while, e.g. the
/// full EPD refresh the 1 s "long press" gesture triggers mid-hold) to
/// enter deep sleep. Intentionally longer than that 1 s long-press so the
/// two gestures don't collide.
const DEEP_SLEEP_HOLD: Duration = Duration::from_secs(3);

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
    log::info!("Power latch is high; rendering home screen");

    // Taken once and cloned into each store: `EspDefaultNvsPartition::take()`
    // is a true singleton (a global taken-flag, not a ref-counted "take a
    // new handle" call) and errors with `ESP_ERR_INVALID_STATE` if called
    // again while an earlier handle is still alive - three independent
    // `open()`s each calling `take()` themselves made every boot fail here
    // once `alarms.rs`/`todos.rs` were added, since `counters`'s handle was
    // still alive when `AlarmStore::open()` tried to take its own.
    let nvs_partition = esp_idf_svc::nvs::EspDefaultNvsPartition::take()
        .map_err(|e| anyhow::anyhow!("failed to initialise default NVS partition: {e}"))?;
    let counters = PersistedCounters::open(nvs_partition.clone())?;
    let alarm_store = AlarmStore::open(nvs_partition.clone())?;
    let todo_store = TodoStore::open(nvs_partition)?;
    let usb_console = usb_console::UsbConsole::start();

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

    // Ring before the normal boot render, if this boot is the RTC alarm
    // firing: latency to sound matters more than latency to the home
    // screen. `power::wake_cause()` reads `esp_sleep_get_wakeup_cause`
    // again (harmless, not a consuming read) rather than threading the
    // value through from `woke_from_deep_sleep` above, since that bool
    // collapses ENTER-wake and alarm-wake into the same case.
    if power::wake_cause() == power::WakeCause::RtcAlarm {
        log::info!("Woke from RTC alarm; ringing");
        ring_alarm_until_dismissed(&mut board)?;
        if let Err(err) = board.rtc.ack_alarm() {
            log::warn!("PCF8563 ack_alarm failed: {err}");
        }
        // A fired one-shot alarm is spent; drop it so it doesn't linger in
        // the store and confuse `alarms::next_due` on a future boot. Daily
        // alarms recur on their own and don't need this.
        if let (Ok(mut list), Some(dt)) = (alarm_store.load(), clock.as_ref()) {
            let before = list.len();
            list.retain(|a| !alarms::is_expired_once(a, dt));
            if list.len() != before {
                if let Err(err) = alarm_store.save(&list) {
                    log::warn!("Failed to save alarms after dropping fired one-shot: {err}");
                }
            }
        }
    }

    // Keep the PCF8563's single hardware alarm slot pointed at whichever
    // stored alarm is nearest, every boot: after arming/editing an alarm,
    // after a ring+ack above, or just because nothing armed it yet this
    // session (the RTC keeps its own alarm config across deep sleep, but a
    // fresh flash or an edit made while the device was off both need this).
    if let Some(dt) = clock.as_ref() {
        match alarm_store.load() {
            Ok(list) => {
                if let Err(err) = alarms::program_hardware_alarm(&mut board.rtc, &list, dt) {
                    log::warn!("Failed to program hardware alarm: {err}");
                }
            }
            Err(err) => log::warn!("Failed to load alarms: {err}"),
        }
    }

    render_home_now(&mut board, &alarm_store, &todo_store, clock.as_ref());
    board.display.refresh_full()?;
    log::info!("Initial display refresh completed");

    if board.audio.is_none() {
        log::warn!("ES8311 not available");
    }
    if board.nfc.is_none() {
        log::warn!("NFC not available");
    }

    report_power_state(&mut board)?;

    // Taken once up front (it's a singleton) so both the boot-time Wi-Fi
    // bring-up below and the on-device Wi-Fi setup wizard (triggered from
    // the main loop by holding UP, or from the menu) can use it.
    let sysloop = esp_idf_svc::eventloop::EspSystemEventLoop::take()?;

    // Optional Wi-Fi bring-up: connect with credentials stored in NVS
    // (`wifi_ssid` / `wifi_pass`), then sync the clock over NTP and push the
    // time into the PCF8563 so it keeps ticking while the device sleeps.
    // Failure to connect or sync only logs a warning; the rest of the UI
    // keeps working regardless. Skipped on a deep-sleep wake with a healthy
    // RTC so ENTER wakes the device up instantly instead of blocking on the
    // network for several seconds.
    let wifi_sta = if !needs_wifi_sync {
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
                                render_home_now(
                                    &mut board,
                                    &alarm_store,
                                    &todo_store,
                                    clock.as_ref(),
                                );
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
    // Drop the boot-time connection instead of holding the modem for the
    // rest of the program: nothing else here needs Wi-Fi to stay up, and
    // keeping it claimed would block the setup wizard (holding UP, or from
    // the menu) from ever constructing its own EspWifi - only one may exist
    // at a time.
    drop(wifi_sta);

    let mut led_on = false;
    let mut led_tick = 0u32;
    let mut status_tick = 0u32;
    let mut clock_tick = 0u32;
    let mut down_held_since: Option<Duration> = None;
    let mut up_held_since: Option<Duration> = None;
    let mut ble_control: Option<ble_control::BleControl> = None;
    loop {
        watchdog::feed();
        let now = EspSystemTime {}.now();

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

        // Poll USB console for incoming commands, dispatch them, and send replies.
        if let Some(cmd) = usb_console.poll_command() {
            let reply = control::dispatch(
                cmd,
                &mut board,
                &counters,
                &sysloop,
                &alarm_store,
                &todo_store,
                clock.as_ref(),
            );
            usb_console::write_reply(&reply);
        }

        // Poll BLE for incoming commands (if BLE is active), dispatch them, and send replies.
        if let Some(ble) = &ble_control {
            if let Some(cmd) = ble.poll_command() {
                let reply = control::dispatch(
                    cmd,
                    &mut board,
                    &counters,
                    &sysloop,
                    &alarm_store,
                    &todo_store,
                    clock.as_ref(),
                );
                ble.write_reply(&reply);
            }
        }

        if let Some(event) = board.key_enter.poll() {
            match event {
                ButtonEvent::Pressed => {
                    log::info!("ENTER pressed; opening menu");
                    screens::open_menu(
                        &mut board,
                        &counters,
                        &sysloop,
                        &alarm_store,
                        &todo_store,
                        clock.as_ref(),
                        &mut ble_control,
                    );
                    full_refresh = true;
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
                    up_held_since = Some(now);
                }
                ButtonEvent::LongPressed => {
                    log::info!("UP long pressed; full refresh to clean ghosting");
                    full_refresh = true;
                }
                ButtonEvent::Released => {
                    up_held_since = None;
                }
            }
        }
        // Wall-clock hold check, not a poll-cycle count: an iteration that
        // triggers a full EPD refresh (e.g. the long-press above) can block
        // for well over a second, and a cycle counter would undercount that
        // dead time instead of reflecting how long the button was actually
        // held.
        if let Some(since) = up_held_since {
            if now.saturating_sub(since) >= provision::ENTER_HOLD {
                log::info!("UP held for 3 s; entering Wi-Fi setup wizard");
                up_held_since = None;
                provision::run(&mut board, &counters, &sysloop);
                full_refresh = true;
            }
        }

        if let Some(event) = board.key_down.poll() {
            match event {
                ButtonEvent::Pressed => {
                    down_held_since = Some(now);
                }
                ButtonEvent::LongPressed => {
                    log::info!("DOWN long pressed; full refresh to clean ghosting");
                    full_refresh = true;
                }
                ButtonEvent::Released => {
                    down_held_since = None;
                }
            }
        }
        if let Some(since) = down_held_since {
            if now.saturating_sub(since) >= DEEP_SLEEP_HOLD {
                log::info!("DOWN held for 3 s; entering deep sleep");
                render_home_now(&mut board, &alarm_store, &todo_store, clock.as_ref());
                board.display.refresh_full()?;
                power::enter_deep_sleep_with_wakeups(None);
            }
        }

        if full_refresh {
            render_home_now(&mut board, &alarm_store, &todo_store, clock.as_ref());
            board.display.refresh_full()?;
            log::info!("Full display refresh completed");
        } else if !dirty.is_empty() {
            render_home_now(&mut board, &alarm_store, &todo_store, clock.as_ref());
            for rect in &dirty {
                board.display.refresh_partial(*rect)?;
            }
            log::info!("Partial display refresh completed");
        }

        thread::sleep(Duration::from_millis(POLL_INTERVAL_MS as u64));
    }
}

/// Renders the idle/background screen with a freshly-loaded next-alarm
/// label and pending-todo count. Called after every edit made in
/// `screens::open_menu` (via the caller re-rendering on return) and on
/// every clock tick, so these two NVS-backed reads happen fairly often;
/// both stores are tiny JSON blobs, cheap next to the EPD refresh itself.
fn render_home_now(
    board: &mut Note4Board,
    alarm_store: &AlarmStore,
    todo_store: &TodoStore,
    clock: Option<&DateTime>,
) {
    let next_alarm = clock.and_then(|dt| screens::next_alarm_label(alarm_store, dt));
    let todo_pending = screens::pending_todo_count(todo_store);
    board
        .display
        .render_home(clock, next_alarm.as_deref(), todo_pending);
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
                charging,
                charge_done
            );
        }
    }
    Ok(())
}

/// Safety bound so an unattended/stuck-button alarm can't ring forever and
/// drain the battery; generous for a bedside alarm.
const MAX_RING_SECS: u64 = 300;

/// Draws the alarm screen, then alternates short tone bursts with polling
/// ENTER, until dismissed or `MAX_RING_SECS` elapses. Blocks the main loop
/// for the whole ring, so it feeds the watchdog every iteration.
fn ring_alarm_until_dismissed(board: &mut Note4Board) -> Result<()> {
    let canvas = board.display.canvas_mut();
    canvas.clear();
    canvas.draw_text_prop(40, 100, 4, "ALARM");
    canvas.draw_text_prop(40, 160, 1, "ENTER = DISMISS");
    let _ = board.display.refresh_full();

    let start = EspSystemTime {}.now();
    loop {
        watchdog::feed();
        if let Some(ButtonEvent::Pressed) = board.key_enter.poll() {
            log::info!("Alarm dismissed");
            break;
        }
        let elapsed = EspSystemTime {}.now().saturating_sub(start);
        if elapsed >= Duration::from_secs(MAX_RING_SECS) {
            log::warn!("Alarm ring timed out after {MAX_RING_SECS}s with no dismiss");
            break;
        }
        if let Some(audio) = board.audio.as_mut() {
            if let Err(err) = audio.play_sine_stereo(880.0, 0.3, 8000) {
                log::warn!("Alarm tone playback failed: {err}");
            }
        } else {
            thread::sleep(Duration::from_millis(POLL_INTERVAL_MS as u64));
        }
    }
    Ok(())
}
