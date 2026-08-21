mod alarms;
mod audio;
mod ble_control;
mod board;
mod button;
mod canvas;
mod control;
mod ctx;
mod display;
mod font5x7;
mod font8x16;
mod font_cjk;
mod home;
mod icons;
mod inbox;
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
use canvas::{Canvas, Rect};
use ctx::DeviceContext;
use esp_idf_svc::systime::EspSystemTime;
use inbox::InboxStore;
use rtc::DateTime;
use storage::PersistedCounters;
use todos::{Importance, Todo, TodoStore};

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
    let todo_store = TodoStore::open(nvs_partition.clone())?;
    let inbox_store = InboxStore::open(nvs_partition)?;
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
        &inbox_store,
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
                                    &inbox_store,
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

    // Bundle the long-lived state into one context, then run the main loop
    // through it instead of threading board/stores/wifi individually.
    let mut ctx = DeviceContext {
        board: &mut board,
        counters: &counters,
        wifi_mgr: &mut wifi_mgr,
        alarm_store: &alarm_store,
        todo_store: &todo_store,
        inbox_store: &inbox_store,
    };

    let mut status_tick = 0u32;
    let mut clock_tick = 0u32;
    // Cron-style wall-clock alignment: sync decisions fire when the
    // boundary index *advances*, not when a boot-relative timer elapses -
    // so "every 30s" means at :00/:30 of each minute and "every 1h" means
    // at the top of the hour. Initialized to the boot-time boundary so the
    // first aligned boundary after boot fires (and a never-synced device
    // syncs on its first urgent-poll boundary).
    let sync_interval_minutes = ctx.counters.sync_interval_minutes().unwrap_or(60) as u64;
    let boot_unix = clock.map(|dt| dt.to_unix()).unwrap_or(0);
    let mut last_urgent_boundary = boot_unix / 30;
    let mut last_full_boundary = boot_unix / (sync_interval_minutes * 60);
    let never_synced = ctx.counters.last_sync_epoch().unwrap_or(None).is_none();
    let mut ble_control: Option<ble_control::BleControl> = None;
    loop {
        watchdog::feed();
        let mut dirty: Vec<Rect> = Vec::new();

        // Show any unread urgent inbox message as a full-screen reminder with
        // a persistent tone. Checked every loop iteration (cheap: an NVS read
        // that returns immediately when empty) so an urgent message surfaces
        // right away no matter which sync path fetched it - USB/CLI SyncNow,
        // the urgent poll, or an automatic sync. The reminder blocks the main
        // loop, so when it dismisses we must redraw the whole screen - a
        // plain clock tick would only repaint the clock region and leave
        // stale reminder pixels behind.
        if maybe_remind_urgent_inbox(&mut ctx) {
            dirty.push(FULL_SCREEN_RECT);
        }

        status_tick += 1;
        if status_tick >= STATUS_REPORT_INTERVAL_POLLS {
            status_tick = 0;
            if let Err(err) = report_power_state(ctx.board) {
                log::warn!("Power status probe failed: {err}");
            }
        }

        clock_tick += 1;
        if clock_tick >= CLOCK_POLL_INTERVAL_POLLS {
            clock_tick = 0;
            match ctx.board.rtc.read_time() {
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
                    // Cron-style sync decisions ride the fresh clock read
                    // (every 1.2 s): boundaries only fire once each, aligned
                    // to wall clock. Both checks are cheap NVS reads unless a
                    // boundary actually advanced, so this never adds Wi-Fi
                    // traffic beyond the intended cadence. The due-todo
                    // reminder shares the urgent-poll cadence (once per 30 s
                    // boundary, gated once/day anyway). Menus block the loop,
                    // so nothing fires mid-menu; a boundary crossed while
                    // blocked just fires on the next read.
                    maybe_auto_sync(
                        &mut ctx,
                        &dt,
                        &mut last_urgent_boundary,
                        &mut last_full_boundary,
                        never_synced,
                    );
                    if maybe_remind_due_todos(ctx.board, ctx.counters, ctx.todo_store, Some(&dt)) {
                        dirty.push(FULL_SCREEN_RECT);
                    }
                }
                Err(err) => log::warn!("PCF8563 read_time failed: {err}"),
            }
        }

        // Poll USB console for incoming commands, dispatch them, and send replies.
        if let Some(cmd) = usb_console.poll_command() {
            let needs_full_redraw = matches!(cmd, control::Command::SyncNow);
            let reply = control::dispatch(&mut ctx, cmd, clock.as_ref());
            if needs_full_redraw && matches!(reply, control::Reply::Ok) {
                dirty.push(FULL_SCREEN_RECT);
            }
            usb_console::write_reply(&reply);
        }

        // Poll BLE for incoming commands (if BLE is active), dispatch them, and send replies.
        if let Some(ble) = &ble_control {
            if let Some(cmd) = ble.poll_command() {
                let needs_full_redraw = matches!(cmd, control::Command::SyncNow);
                let reply = control::dispatch(&mut ctx, cmd, clock.as_ref());
                if needs_full_redraw && matches!(reply, control::Reply::Ok) {
                    dirty.push(FULL_SCREEN_RECT);
                }
                ble.write_reply(&reply);
            }
        }

        if let Some(event) = ctx.board.key_enter.poll() {
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
        if let Some(event) = ctx.board.key_up.poll() {
            match event {
                ButtonEvent::Pressed => {
                    // Home has no vertical selection.
                }
                ButtonEvent::LongPressed => {
                    log::info!("UP long pressed; opening navigation");
                    screens::open_navigation(&mut ctx, clock.as_ref(), &mut ble_control);
                    dirty.push(FULL_SCREEN_RECT);
                }
                ButtonEvent::Released => {}
            }
        }

        if let Some(event) = ctx.board.key_down.poll() {
            match event {
                ButtonEvent::Pressed => {
                    // Home has no vertical selection.
                }
                ButtonEvent::LongPressed => {
                    log::info!("DOWN long pressed; opening navigation");
                    screens::open_navigation(&mut ctx, clock.as_ref(), &mut ble_control);
                    dirty.push(FULL_SCREEN_RECT);
                }
                ButtonEvent::Released => {}
            }
        }

        if !dirty.is_empty() {
            render_home_now(
                ctx.board,
                ctx.counters,
                ctx.alarm_store,
                ctx.todo_store,
                ctx.inbox_store,
                clock.as_ref(),
            );
            if dirty
                .iter()
                .any(|rect| rect.width == 400 && rect.height == 300)
            {
                ctx.board.display.refresh_partial(FULL_SCREEN_RECT)?;
            } else {
                for rect in &dirty {
                    ctx.board.display.refresh_partial(*rect)?;
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
    inbox_store: &InboxStore,
    clock: Option<&DateTime>,
) {
    let next_alarm = clock.and_then(|dt| screens::next_alarm_label(alarm_store, dt));
    let todo_summary = screens::todo_summary(todo_store, clock);
    let unread_inbox = inbox_store.unread_count().unwrap_or(0);
    let wifi_configured = counters
        .wifi_creds()
        .map(|creds| creds.is_some())
        .unwrap_or(false);
    let battery_percent = board
        .battery_millivolts()
        .ok()
        .map(board::battery_percent_from_mv);
    let charge = board.charge_snapshot();
    board.display.render_home(
        clock,
        next_alarm.as_ref().map(|label| label.time.as_str()),
        next_alarm.as_ref().and_then(|label| label.date.as_deref()),
        next_alarm.as_ref().map(|label| label.days_left),
        todo_summary.pending,
        todo_summary.due_today,
        unread_inbox,
        wifi_configured,
        battery_percent,
        charge,
    );
}

/// Cron-style sync entry point, called on every fresh clock read (~1.2 s)
/// from the main loop. Fires only when a wall-clock *boundary advances*:
///   - An urgent poll when `unix / 30` advances - i.e. at :00/:30 of each
///     minute, not on a boot-relative 30 s timer. Lightweight connection,
///     the server answers immediately; if it reports an unread
///     high-priority message a full sync runs right away to fetch it.
///   - A full sync when `unix / (interval*60)` advances - every 1 h fires
///     at the top of the hour, every 30 m at :00/:30, every 5 m at the
///     :05 marks, etc. (normal content stays on the slow timer).
///
/// Both checks are cheap NVS reads unless a boundary actually advanced, so
/// the 1.2 s cadence never adds network traffic.
///
/// Requires Wi-Fi + server config; failures are logged and retried at the
/// next boundary. `never_synced` forces the first full sync on the first
/// urgent-poll boundary after boot so a fresh device gets content promptly.
fn maybe_auto_sync(
    ctx: &mut DeviceContext,
    now: &DateTime,
    last_urgent_boundary: &mut u64,
    last_full_boundary: &mut u64,
    never_synced: bool,
) {
    let server_configured = ctx
        .counters
        .device_config()
        .map(|cfg| cfg.is_some())
        .unwrap_or(false);
    if !server_configured {
        return;
    }
    let wifi_configured = ctx
        .counters
        .wifi_creds()
        .map(|creds| creds.is_some())
        .unwrap_or(false);
    if !wifi_configured {
        return;
    }
    let interval = ctx.counters.sync_interval_minutes().unwrap_or(60) as u64;
    let unix = now.to_unix();
    let urgent_boundary = unix / 30;
    let full_boundary = unix / (interval * 60);
    if urgent_boundary == *last_urgent_boundary && full_boundary == *last_full_boundary {
        return;
    }

    // Urgent poll at each 30 s boundary: short connection, immediate
    // answer. If the server reports an unread high-priority message, do a
    // full sync now to pull it in real time.
    let urgent_fired = if urgent_boundary != *last_urgent_boundary {
        *last_urgent_boundary = urgent_boundary;
        match sync::poll_urgent(ctx.counters, ctx.wifi_mgr) {
            Ok(true) => {
                log::info!("Urgent message available; syncing");
                run_full_sync(ctx, now);
                return;
            }
            Ok(false) => {}
            Err(err) => log::warn!("Urgent poll failed: {err}"),
        }
        true
    } else {
        false
    };

    // Full sync at each interval boundary. A never-synced device (fresh
    // flash / wiped NVS) syncs on its first urgent-poll boundary (~30 s
    // after boot) instead of waiting for the next interval boundary.
    if full_boundary != *last_full_boundary || (never_synced && urgent_fired) {
        *last_full_boundary = full_boundary;
        log::info!("Aligned sync due (interval {interval} min); syncing");
        run_full_sync(ctx, now);
    }
}

/// Runs a full `sync::sync_now` and refreshes the home screen on success.
/// (The once-per-day NTP RTC alignment lives inside `sync::sync_now`,
/// where Wi-Fi is still connected; `maybe_align_rtc` there keeps the
/// PCF8563's drift from pulling the cron sync boundaries off the real
/// wall clock.)
fn run_full_sync(ctx: &mut DeviceContext, now: &DateTime) {
    match sync::sync_now(
        ctx.counters,
        ctx.wifi_mgr,
        ctx.alarm_store,
        ctx.todo_store,
        ctx.inbox_store,
        &mut ctx.board.rtc,
        now,
    ) {
        Ok(_) => {
            log::info!("Full sync completed");
            render_home_now(
                ctx.board,
                ctx.counters,
                ctx.alarm_store,
                ctx.todo_store,
                ctx.inbox_store,
                Some(now),
            );
            if let Err(err) = ctx.board.display.refresh_partial(FULL_SCREEN_RECT) {
                log::warn!("Failed to refresh display after sync: {err}");
            }
        }
        Err(err) => log::warn!("Full sync failed: {err}"),
    }
}

fn report_power_state(board: &mut Note4Board) -> Result<()> {
    let charge = board.charging_state();
    if let Err(err) = board.update_charging_led(charge) {
        log::warn!("Charging LED update failed: {err}");
    }
    match board.battery_millivolts() {
        Ok(vbat_mv) => log::info!(
            "Power state: power_present={} charging={} full={} vbat_mV={} ({}%)",
            charge.power_present,
            charge.charging,
            charge.full,
            vbat_mv,
            board::battery_percent_from_mv(vbat_mv)
        ),
        Err(err) => {
            log::warn!("Battery ADC read failed: {err}");
            log::info!(
                "Power state: power_present={} charging={} full={} vbat_mV=<n/a>",
                charge.power_present,
                charge.charging,
                charge.full
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
    // Standard header for consistency; the big ALARM word below carries
    // the dramatic weight (the header title repeats it deliberately, like
    // a page whose content states its own subject).
    ui::header(canvas, "ALARM");
    let alarm_w = Canvas::text_prop_width("ALARM", 4);
    canvas.draw_text_prop(200usize.saturating_sub(alarm_w / 2), 92, 4, "ALARM");
    let hint = "ENTER = DISMISS";
    let hint_w = Canvas::text_prop_width(hint, 1);
    canvas.draw_text_prop(200usize.saturating_sub(hint_w / 2), 184, 1, hint);
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

/// Checks - once per calendar day - whether any `High`-importance todo is
/// due today and still open, and if so blocks the main loop on a "TODOS
/// DUE" screen with a short tone burst until ENTER. The reminder date is
/// recorded *before* the screen so a missed dismiss never re-rings on the
/// next 30 s check. Runs only while the main loop is idle on Home (menus
/// block the loop, so this can't interrupt them). Returns whether a screen
/// was shown, so the caller can force a full redraw afterwards.
fn maybe_remind_due_todos(
    board: &mut Note4Board,
    counters: &PersistedCounters,
    todo_store: &TodoStore,
    clock: Option<&DateTime>,
) -> bool {
    let Some(now) = clock else {
        return false;
    };
    let date_key = format!("{:04}{:02}{:02}", now.year, now.month, now.day);
    match counters.todo_reminded_date() {
        Ok(Some(prev)) if prev == date_key => return false,
        Ok(_) => {}
        Err(err) => log::warn!("Failed to read todo reminder date: {err}"),
    }

    let Ok(list) = todo_store.load() else {
        return false;
    };
    let due: Vec<&Todo> = list
        .iter()
        .filter(|t| {
            if t.done || t.importance != Importance::High {
                return false;
            }
            match &t.repeat {
                Some(r) => r.fires_on(now.year, now.month, now.day, now.weekday),
                None => t.due_date.is_some_and(|d| {
                    d.year == now.year && d.month == now.month && d.day == now.day
                }),
            }
        })
        .collect();
    if due.is_empty() {
        return false;
    }

    if let Err(err) = counters.set_todo_reminded_date(&date_key) {
        log::warn!("Failed to record todo reminder date: {err}");
    }
    log::info!("{} high-importance todo(s) due today; reminding", due.len());
    remind_due_todos_screen(board, &due);
    true
}

/// Full-screen "TODOS DUE" with up to 7 due items, a short 3-note tone
/// burst, then a blocking wait for ENTER (or long ENTER) - same shape as
/// `ring_alarm_until_dismissed`, without the 5-minute timeout since these
/// are far less urgent.
fn remind_due_todos_screen(board: &mut Note4Board, due: &[&Todo]) {
    let canvas = board.display.canvas_mut();
    canvas.clear();
    // Standard page header (brand + title + rule) so every full-screen
    // alert shares the visual language of the rest of the UI.
    ui::header(canvas, "TODOS DUE");
    for (i, todo) in due.iter().take(7).enumerate() {
        // Truncate long text by measured width so a CJK todo can't push
        // the row off the right edge.
        let text = screens::truncate_prop(&todo.text, 300);
        canvas.draw_text_prop(16, 48 + i * 24, 1, &format!("!! {text}"));
    }
    if due.len() > 7 {
        canvas.draw_text_prop(16, 268, 1, "MORE...");
    }
    canvas.draw_text_prop(16, 284, 1, "ENTER = DISMISS");
    let _ = board.display.refresh_full();

    if let Some(audio) = board.audio.as_mut() {
        for _ in 0..3 {
            if let Err(err) = audio.play_sine_stereo(1046.0, 0.15, 8000) {
                log::warn!("Todo reminder tone failed: {err}");
                break;
            }
            thread::sleep(Duration::from_millis(150));
        }
    }

    loop {
        watchdog::feed();
        if let Some(event) = board.key_enter.poll() {
            if matches!(event, ButtonEvent::Pressed | ButtonEvent::LongPressed) {
                break;
            }
        }
        thread::sleep(Duration::from_millis(POLL_INTERVAL_MS as u64));
    }
}

/// Shows unread urgent (high-priority alert) inbox items as a full-screen
/// reminder with an insistent tone, marking each read locally so it can't
/// re-ring. The urgent poll + full sync above fetch them; this renders them.
/// Runs only while idle on Home. Returns whether a reminder was shown, so
/// the caller can force a full redraw afterwards (the reminder's full-screen
/// render would otherwise leave stale pixels behind).
fn maybe_remind_urgent_inbox(ctx: &mut DeviceContext) -> bool {
    let Ok(list) = ctx.inbox_store.load() else {
        return false;
    };
    let urgent: Vec<u64> = ctx.inbox_store.unread_urgent().unwrap_or_default();
    if urgent.is_empty() {
        return false;
    }
    // Mark every urgent item read first. If any persist fails, skip showing
    // this batch entirely - otherwise a failed write would leave the item
    // unread and this function would re-ring it on every loop iteration,
    // looping forever.
    for seq in &urgent {
        if let Err(err) = ctx.inbox_store.mark_read(*seq) {
            log::warn!("Failed to mark urgent inbox read; not reminding: {err}");
            return false;
        }
    }
    let titles: Vec<String> = list
        .iter()
        .filter(|it| urgent.contains(&it.id))
        .map(|it| screens::truncate_prop(&it.title, 330))
        .collect();
    log::info!("{} urgent inbox message(s) to show", titles.len());
    remind_urgent_screen(ctx.board, &titles);
    true
}

/// Full-screen urgent reminder: title(s) with a persistent tone loop until
/// ENTER dismisses it. Distinct from the normal alert by a longer, repeated
/// tone and the "URGENT" page title so urgent messages are unmistakable.
fn remind_urgent_screen(board: &mut Note4Board, titles: &[String]) {
    let canvas = board.display.canvas_mut();
    canvas.clear();
    ui::header(canvas, "URGENT");
    for (i, title) in titles.iter().take(4).enumerate() {
        canvas.draw_text_prop(16, 48 + i * 24, 1, &format!("!! {title}"));
    }
    if titles.len() > 4 {
        canvas.draw_text_prop(16, 268, 1, "MORE IN INBOX...");
    }
    canvas.draw_text_prop(16, 284, 1, "ENTER = DISMISS");
    let _ = board.display.refresh_full();

    // Persistent tone bursts until dismissed - urgent messages keep beeping,
    // unlike the normal short alert. Crank the DAC volume to max, use a large
    // sine amplitude, and alternate high/low pitches so the urgent siren is
    // unmistakably loud and distinct from a plain single tone.
    //
    // Dismiss is driven by `is_pressed()` (the button going down), not by the
    // debounced release event - so a single short tap returns to Home the
    // moment ENTER goes down, exactly as requested. The siren is played as
    // short single notes with a tight press-poll window between each one, and
    // the whole reminder is bounded by a safety timeout so an unattended
    // device can't ring forever.
    if let Some(audio) = board.audio.as_mut() {
        if let Err(err) = audio.set_volume(255) {
            log::warn!("Urgent volume boost failed: {err}");
        }
    }
    // Two-note siren (F#6/C6) repeated until dismiss.
    const SIREN: [(f32, f32); 2] = [(1397.0, 0.12), (1046.0, 0.12)];
    let mut siren_step = 0usize;
    let ring_start = EspSystemTime {}.now();
    loop {
        watchdog::feed();
        // Keep the debouncer fed so a press is registered at all
        // (`is_pressed` only reflects the debounced state that `poll`
        // advances), and dismiss the moment ENTER is debounced-down,
        // without waiting for its release.
        board.key_enter.poll();
        // Dismiss as soon as ENTER goes down (raw press, no release wait).
        if board.key_enter.is_pressed() {
            // Drain the button so its eventual release can't emit a stray
            // `Pressed` that leaks into the main loop as a spurious action.
            while board.key_enter.poll().is_some() {}
            return;
        }
        if (EspSystemTime {}).now().saturating_sub(ring_start)
            >= Duration::from_secs(URGENT_RING_MAX_SECS)
        {
            log::warn!("Urgent reminder timed out after {URGENT_RING_MAX_SECS}s");
            return;
        }
        let (freq, dur) = SIREN[siren_step % SIREN.len()];
        if let Some(audio) = board.audio.as_mut() {
            if let Err(err) = audio.play_sine_stereo(freq, dur, 24000) {
                log::warn!("Urgent siren note failed: {err}");
            }
        } else {
            thread::sleep(Duration::from_millis(dur as u64 * 1000));
        }
        siren_step += 1;
        // Tight press-poll window after each note so a press is caught
        // promptly between siren notes (never during a long blocking play).
        let poll_deadline = EspSystemTime {}.now() + Duration::from_millis(400);
        loop {
            watchdog::feed();
            board.key_enter.poll();
            if board.key_enter.is_pressed() {
                while board.key_enter.poll().is_some() {}
                return;
            }
            let now = EspSystemTime {}.now();
            if now >= poll_deadline {
                break;
            }
            thread::sleep(Duration::from_millis(POLL_INTERVAL_MS as u64));
        }
    }
}

/// Safety bound so an unattended urgent reminder can't ring forever and drain
/// the battery; generous for an urgent alert.
const URGENT_RING_MAX_SECS: u64 = 120;
