mod alarms;
mod audio;
mod ble_control;
mod board;
mod button;
mod canvas;
mod control;
mod display;
mod font8x16;
mod icons;
mod nfc;
mod power;
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

/// Bounding rect for the large home clock and its date/status metadata.
const CLOCK_RECT: Rect = Rect {
    x: 16,
    y: 36,
    width: 368,
    height: 92,
};

const FULL_SCREEN_RECT: Rect = Rect {
    x: 0,
    y: 0,
    width: 400,
    height: 300,
};

/// Epoch seconds captured at firmware build time. Used as the fallback RTC
/// seed when PCF8563 reports `voltage_low = true` (battery was disconnected
/// or drained). Captured by `build.rs` so a rebuild refreshes the value;
/// once the RTC keeps time on its coin cell we stop consulting this.
const BUILD_EPOCH_SECS: u64 = build_epoch_secs();

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
    power::log_wakeup_cause();
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
    let mut usb_console = usb_console::UsbConsole::start();

    // Wi-Fi/NTP resync is needed only when the battery-backed RTC cannot be
    // trusted. A firmware flash or ordinary reset does not erase PCF8563
    // time, so connecting on every reset merely consumes the one safe Wi-Fi
    // session and forces the first user-triggered Sync Now to reboot.
    let mut needs_wifi_sync = false;
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
                let offset = counters.timezone_offset_minutes().unwrap_or(0);
                let seeded = DateTime::from_unix(BUILD_EPOCH_SECS).shifted_minutes(offset as i32);
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
    // again (harmless, not a consuming read); unlike the raw cause logged
    // above, this distinguishes ENTER-wake from alarm-wake.
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

    render_home_now(
        &mut board,
        &counters,
        &alarm_store,
        &todo_store,
        clock.as_ref(),
    );
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

    // Also created exactly once and reused for the rest of the program -
    // see `wifi::WifiManager`'s doc comment for why: a second
    // `EspWifi::new()` anywhere in the process reliably crashes
    // (`Guru Meditation Error: InstrFetchProhibited`), confirmed on real
    // hardware, so every Wi-Fi user (boot-time sync below, the setup
    // wizard, `sync::sync_now`) shares this one instance instead of each
    // creating and dropping its own.
    let mut wifi_mgr = wifi::WifiManager::new(&sysloop)?;

    // Optional Wi-Fi bring-up: connect with credentials stored in NVS
    // (`wifi_ssid` / `wifi_pass`), then sync the clock over NTP and push the
    // time into the PCF8563 so it keeps ticking while the device sleeps.
    // Failure to connect or sync only logs a warning; the rest of the UI
    // keeps working regardless. A healthy battery-backed RTC needs no boot
    // network traffic; periodic auto-sync (see the main loop below) brings
    // Wi-Fi up on its own schedule, and multiple connects per boot are safe
    // (see `wifi::WifiManager`).
    if !needs_wifi_sync {
        log::info!("RTC is healthy; skipping boot-time Wi-Fi/NTP resync");
    } else {
        match counters.wifi_creds() {
            Ok(Some(creds)) => match wifi_mgr.connect(&creds) {
                Ok(()) => {
                    let timezone_offset = counters.timezone_offset_minutes().unwrap_or(0);
                    match wifi::ntp_sync_and_set_rtc(&mut board.rtc, timezone_offset) {
                        Ok(()) => match board.rtc.read_time() {
                            Ok(dt) => {
                                clock = Some(dt);
                                render_home_now(
                                    &mut board,
                                    &counters,
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
                    wifi_mgr.disconnect();
                }
                Err(err) => log::warn!("Wi-Fi connect failed: {err}"),
            },
            Ok(None) => {
                log::info!(
                    "No Wi-Fi credentials in NVS; skipping connect (see scripts/gen-nvs-wifi.py)"
                );
            }
            Err(err) => log::warn!("Could not read Wi-Fi credentials from NVS: {err}"),
        }
    };

    let mut status_tick = 0u32;
    let mut clock_tick = 0u32;
    let mut auto_sync_tick = 0u32;
    let mut ble_control: Option<ble_control::BleControl> = None;
    loop {
        watchdog::feed();
        status_tick += 1;
        if status_tick >= STATUS_REPORT_INTERVAL_POLLS {
            status_tick = 0;
            if let Err(err) = report_power_state(&mut board) {
                log::warn!("Power status probe failed: {err}");
            }
        }

        // Periodic auto-sync check every 30 s (1500 × 20 ms). The check
        // itself is cheap (two NVS reads); the actual sync only runs when
        // the elapsed time since the last one exceeds the configured
        // interval (see `screens.rs`'s SYNC INTERVAL setting).
        auto_sync_tick += 1;
        if auto_sync_tick >= 1500 {
            auto_sync_tick = 0;
            maybe_auto_sync(
                &mut board,
                &counters,
                &mut wifi_mgr,
                &alarm_store,
                &todo_store,
                clock.as_ref(),
            );
        }

        let mut dirty: Vec<Rect> = Vec::new();

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
            let needs_full_redraw = matches!(cmd, control::Command::SyncNow);
            let reply = control::dispatch(
                cmd,
                &mut board,
                &counters,
                &mut wifi_mgr,
                &alarm_store,
                &todo_store,
                clock.as_ref(),
            );
            if needs_full_redraw && matches!(reply, control::Reply::Ok) {
                dirty.push(FULL_SCREEN_RECT);
            }
            usb_console::write_reply(&reply);
        }

        // Poll BLE for incoming commands (if BLE is active), dispatch them, and send replies.
        if let Some(ble) = &ble_control {
            if let Some(cmd) = ble.poll_command() {
                let needs_full_redraw = matches!(cmd, control::Command::SyncNow);
                let reply = control::dispatch(
                    cmd,
                    &mut board,
                    &counters,
                    &mut wifi_mgr,
                    &alarm_store,
                    &todo_store,
                    clock.as_ref(),
                );
                if needs_full_redraw && matches!(reply, control::Reply::Ok) {
                    dirty.push(FULL_SCREEN_RECT);
                }
                ble.write_reply(&reply);
            }
        }

        if let Some(event) = board.key_enter.poll() {
            match event {
                ButtonEvent::Pressed => {
                    // Home has no primary action. Settings is reached only
                    // through the long-UP/DOWN navigation drawer.
                }
                ButtonEvent::LongPressed => {
                    // Home is the root screen, so "back" stays on Home.
                    log::info!("ENTER long pressed on Home; already at root");
                }
                ButtonEvent::Released => {}
            }
        }
        if let Some(event) = board.key_up.poll() {
            match event {
                ButtonEvent::Pressed => {
                    // Home has no vertical selection.
                }
                ButtonEvent::LongPressed => {
                    log::info!("UP long pressed; opening navigation");
                    screens::open_navigation(
                        &mut board,
                        &counters,
                        &mut wifi_mgr,
                        &alarm_store,
                        &todo_store,
                        clock.as_ref(),
                        &mut ble_control,
                    );
                    dirty.push(FULL_SCREEN_RECT);
                }
                ButtonEvent::Released => {}
            }
        }

        if let Some(event) = board.key_down.poll() {
            match event {
                ButtonEvent::Pressed => {
                    // Home has no vertical selection.
                }
                ButtonEvent::LongPressed => {
                    log::info!("DOWN long pressed; opening navigation");
                    screens::open_navigation(
                        &mut board,
                        &counters,
                        &mut wifi_mgr,
                        &alarm_store,
                        &todo_store,
                        clock.as_ref(),
                        &mut ble_control,
                    );
                    dirty.push(FULL_SCREEN_RECT);
                }
                ButtonEvent::Released => {}
            }
        }

        if !dirty.is_empty() {
            render_home_now(
                &mut board,
                &counters,
                &alarm_store,
                &todo_store,
                clock.as_ref(),
            );
            if dirty
                .iter()
                .any(|rect| rect.width == 400 && rect.height == 300)
            {
                board.display.refresh_partial(FULL_SCREEN_RECT)?;
            } else {
                for rect in &dirty {
                    board.display.refresh_partial(*rect)?;
                }
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
    counters: &PersistedCounters,
    alarm_store: &AlarmStore,
    todo_store: &TodoStore,
    clock: Option<&DateTime>,
) {
    let next_alarm = clock.and_then(|dt| screens::next_alarm_label(alarm_store, dt));
    let todo_pending = screens::pending_todo_count(todo_store);
    let wifi_configured = counters
        .wifi_creds()
        .map(|creds| creds.is_some())
        .unwrap_or(false);
    let battery_percent = board
        .battery_millivolts()
        .ok()
        .map(board::battery_percent_from_mv);
    let charging = board.charging_state().0;
    board.display.render_home(
        clock,
        next_alarm.as_ref().map(|label| label.time.as_str()),
        next_alarm.as_ref().and_then(|label| label.date.as_deref()),
        todo_pending,
        wifi_configured,
        battery_percent,
        charging,
    );
}

/// Periodic auto-sync entry point, called every ~30 s from the main loop.
/// Runs a full sync (via `sync::sync_now`, which connects Wi-Fi on demand -
/// safe to do repeatedly now that `wifi::WifiManager::connect` no longer
/// scans) when the elapsed time since the last successful sync exceeds the
/// configured interval. Requires the device to be on Home (the main loop
/// blocks inside any menu screen, so this only fires while idle), and
/// requires server + Wi-Fi configuration to exist. Failures are logged and
/// retried on the next check.
fn maybe_auto_sync(
    board: &mut Note4Board,
    counters: &PersistedCounters,
    wifi_mgr: &mut wifi::WifiManager,
    alarm_store: &AlarmStore,
    todo_store: &TodoStore,
    clock: Option<&DateTime>,
) {
    let Some(now) = clock else {
        return;
    };
    let server_configured = counters
        .device_config()
        .map(|cfg| cfg.is_some())
        .unwrap_or(false);
    if !server_configured {
        return;
    }
    let wifi_configured = counters
        .wifi_creds()
        .map(|creds| creds.is_some())
        .unwrap_or(false);
    if !wifi_configured {
        return;
    }
    let interval = counters.sync_interval_minutes().unwrap_or(60) as u64;
    let due = match counters.last_sync_epoch().unwrap_or(None) {
        Some(last) => now.to_unix().saturating_sub(last) >= interval * 60,
        None => true,
    };
    if !due {
        return;
    }
    log::info!("Automatic sync due (interval {interval} min); syncing");
    match sync::sync_now(
        counters,
        wifi_mgr,
        alarm_store,
        todo_store,
        &mut board.rtc,
        now,
    ) {
        Ok(_) => {
            log::info!("Automatic periodic sync completed");
            render_home_now(board, counters, alarm_store, todo_store, clock);
            if let Err(err) = board.display.refresh_partial(FULL_SCREEN_RECT) {
                log::warn!("Failed to refresh display after auto sync: {err}");
            }
        }
        Err(err) => log::warn!("Automatic periodic sync failed: {err}"),
    }
}

fn report_power_state(board: &mut Note4Board) -> Result<()> {
    let (charging, charge_done) = board.charging_state();
    if let Err(err) = board.update_charging_led() {
        log::warn!("Charging LED update failed: {err}");
    }
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
            // Keep tone chunks short so the debouncer receives enough polls
            // while ENTER is held/released. A 300ms blocking tone made a
            // normal press almost impossible to observe.
            if let Err(err) = audio.play_sine_stereo(880.0, 0.05, 8000) {
                log::warn!("Alarm tone playback failed: {err}");
            }
        } else {
            thread::sleep(Duration::from_millis(POLL_INTERVAL_MS as u64));
        }
    }
    Ok(())
}
