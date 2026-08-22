# Hardware Smoke-Test Checklist

A repeatable checklist for exercising boot, sync, alarm, and BLE/USB
recovery on a physical NOTE4 after flashing. This is the manual counterpart
to `logic/`'s host-runnable tests (see "Remaining engineering work" #1 in
`remaining-work.md`): the `logic` crate locks down the *pure* scheduling,
validation, and dedup rules on every commit without hardware; this checklist
is for everything those tests cannot reach - real I2C timing, real Wi-Fi
association, real flash wear, real button debounce, a real watchdog.

Run this after any change touching `sync.rs`, `wifi.rs`, `alarms.rs`,
`reminders.rs`, `ctx.rs`, `main.rs`, `ble_control.rs`, or `usb_console.rs`,
and before writing "verified on hardware" in `remaining-work.md`. Record the
flashed revision (`git describe`, logged once at boot as `GIT_REV` - see
`remaining-work.md`'s "Remaining engineering work" #3) alongside results.

## 0. Flash and boot

- [ ] Build in release mode; flash via `scripts/build-rust.sh` /
      `espflash flash --monitor` in DIO mode, 80 MHz, 16 MB flash. NOTE4
      only - NOTE4C and QIO mode are forbidden (see Release blockers).
- [ ] Boot log shows: power latch high, RTC read (or a clear reseed-from-VL
      message), display full refresh, Wi-Fi stack init - with **no**
      watchdog reset or panic anywhere in the sequence.
- [ ] `GIT_REV` in the first boot log lines matches the commit under test
      (not the stale ESP-IDF `App version` field - see remaining-work.md
      item 3 in "Remaining engineering work").

## 1. Wi-Fi and sync

- [ ] `set_wifi` (USB) against a real AP saves only after a successful
      DHCP-timeout-free connect; a bad SSID/password is rejected without
      corrupting the previously-saved credentials.
- [ ] `sync_now` (USB, or the on-device menu) completes and applies
      alarms/todos/inbox; confirm via `get_status` or the on-device list.
- [ ] A **second** sync in the same boot session (USB `sync_now` again, or
      wait for the next urgent-poll boundary) also completes - regression
      guard for the old "second connect crashes" class of bug (see
      `wifi.rs`'s `WifiManager` doc comment).
- [ ] If testing the PMF workaround (item 6): connect to a PMF-required or
      PMF-optional AP that previously failed with `assoc -> init` right
      after `Wi-Fi connected`; confirm it now associates and reaches DHCP.
- [ ] **Long soak** (leave running, do not skip this one): let the device
      idle through several hours' worth of urgent-poll and full-sync
      boundaries in one boot session (no reboot). Watch for a task-watchdog
      abort - this is the reproduction shape for item -1; a clean multi-hour
      run with dozens of sync cycles and no reset is the bar for calling
      that item fixed, not just "not observed yet in 20 minutes."

## 2. Alarms

- [ ] Arm a one-shot (`Once`) alarm a few minutes out; let it fire from
      **deep sleep** (device asleep when the RTC alarm triggers, not
      already awake) - this exact path (`main.rs`'s `alarm_fired_at_boot`)
      is still unverified per remaining-work.md's physical-verification
      item 1.
- [ ] ENTER dismisses within ~1s of a short press; sound stops immediately.
- [ ] AF flag is acknowledged and the fired one-shot is removed from the
      store (`Alarm dismissed` -> `No enabled alarms; hardware alarm
      cleared`, or the next alarm re-armed if more are enabled).
- [ ] Arm a **recurring** (`Weekly`/`Monthly`/`Daily`) alarm, let it fire and
      dismiss it, and confirm the hardware alarm slot is **re-armed** for
      the next occurrence (not left cleared) - also still unverified per
      remaining-work.md.
- [ ] While the alarm is ringing, send a USB command and confirm
      `{"status":"busy"}` comes back within a few seconds (not silence).
- [ ] Repeat the busy-reply check over **BLE** (open the pairing screen in
      another test run, or verify via the BLE reply-during-ring path added
      for item 3) - confirms BLE is no longer silent during a blocking
      screen the way it was before this fix.

## 3. Reminders

- [ ] A due, high-importance Todo triggers the full-screen reminder exactly
      once per day, even across multiple boots on the same date.
- [ ] An urgent (`Alert`, `High` priority) inbox item triggers the siren
      reminder; dismiss with ENTER.
- [ ] Both reminder types interrupt whichever screen is open (Calendar,
      Settings, list/detail, BLE pairing) and return to a correctly redrawn
      page afterward - remaining-work.md's physical-verification item 2.

## 4. BLE and USB recovery

- [ ] Open the BLE pairing screen, connect a client, send a command, get a
      reply; leave the screen and confirm advertising stops (RAM reclaimed,
      not left running).
- [ ] Disconnect/reconnect BLE and unplug/replug USB repeatedly over a
      longer session; confirm BLE teardown doesn't affect a later Wi-Fi
      sync (remaining-work.md's physical-verification item 3).
- [ ] Opening the USB serial port with a naive client (default `pyserial`
      `Serial(...)`, DTR/RTS not pre-cleared) resets the chip - expected,
      not a regression; confirms the existing doc warning is still accurate.

## 5. Capacity

- [ ] Sync a maximal alarm list, Todo list, and Inbox (near NVS blob caps -
      see `alarms::BLOB_BUF_LEN` / `todos::BLOB_BUF_LEN` /
      `inbox::BLOB_BUF_LEN`); confirm pagination/truncation and no NVS
      write failure, and check the e-paper for visible ghosting after
      several full refreshes.

## Recording results

Append a dated entry to `remaining-work.md`'s "Verification completed in
this pass" (or a new dated subsection) naming exactly which checklist items
passed, on what hardware, against what flashed revision - matching the
existing entries' level of detail (specific log lines, timings, and repro
counts, not just "works"). An item silently skipped is not the same as one
verified; say which is which.
