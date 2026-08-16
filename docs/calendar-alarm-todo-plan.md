# Calendar / Alarm-Clock / Todo Roadmap

Living plan for turning the NOTE4 firmware from a button-counter smoke test into
a real calendar / alarm clock / todo device. Update the phase checklist as work
lands; keep this in sync with what's actually implemented, not what's aspired to.

## Context

The firmware already has a solid hardware baseline (Wi-Fi STA + NTP, on-device
Wi-Fi provisioning wizard, PCF8563 RTC, battery ADC, ES8311 audio, NFC, deep
sleep with GPIO17 power-latch hold, a task watchdog) but no real application -
`main.rs` just increments and displays three button counters.

Product direction, confirmed with the user:

- The device does **not** own content authoring. A separate PC tool
  (`inkpaper-desktop`, not built yet) configures it over **USB serial or BLE**
  (user picks the transport) - that channel only ever carries Wi-Fi
  credentials + server URL/token, never content.
- A separate backend (`inkpaper-server`, not built yet) is the source of
  truth for alarms and todos. The device pulls this over Wi-Fi/HTTPS once
  configured.
- **Alarms must ring with zero connectivity.** This rules out a
  "device just displays a server-rendered bitmap" model - the firmware has
  to own structured alarm/todo data locally, cached in NVS, so ringing works
  from deep sleep with no Wi-Fi, no server, and no CPU-awake polling loop.

This repo (`inkpaper`) is scoped to the firmware only. `inkpaper-desktop` and
`inkpaper-server` are future work; the two spec docs below exist so those
repos have a stable contract to build against once they start.

## Architecture

```
inkpaper-desktop (future)          inkpaper-server (future)
       |  USB serial / BLE                |  HTTPS GET (polling)
       |  (config only:                   |  (content: alarms[], todos[])
       |   wifi creds, server url+token)  |
       v                                  v
  ┌────────────────────────────────────────────┐
  │              inkpaper firmware              │
  │  control.rs — shared command/reply schema   │
  │  usb_console.rs / ble_control.rs — transports│
  │  sync.rs — HTTPS pull, applies to stores     │
  │  alarms.rs / todos.rs — NVS-backed stores    │
  │  rtc.rs — PCF8563 hardware alarm registers   │
  │  power.rs — deep sleep wake on GPIO0 or GPIO5│
  │  screens.rs — Home/Menu/Calendar/Todos/Alarms│
  └────────────────────────────────────────────┘
```

The offline-alarm requirement is met by always keeping the PCF8563's single
hardware alarm slot programmed to whichever locally-stored alarm is
chronologically nearest, and waking deep sleep on **either** GPIO0 (ENTER)
**or** GPIO5 (`RTC_INT`, the PCF8563's alarm interrupt line) via
`esp_sleep_enable_ext1_wakeup`.

## Spec docs (write once the relevant phase lands)

- `docs/control-protocol.md` - USB/BLE command schema for `inkpaper-desktop`.
- `docs/sync-api.md` - HTTP sync contract for `inkpaper-server`.

## Phases

- [x] **Phase 1 - RTC alarm + deep-sleep ring** (highest risk/value, no
  networking). `rtc.rs` (`AlarmRegs`, `set_alarm`, `alarm_flag`, `ack_alarm`),
  `power.rs` (`enter_deep_sleep_with_wakeups`, `wake_cause` via ext1 on
  GPIO0|GPIO5), `main.rs` (`ring_alarm_until_dismissed`, `arm_test_alarm` -
  a hardcoded "+2 min" test alarm, superseded by Phase 2's real store).
  Implemented, builds clean, flashed to hardware 2026-08-16. **Awaiting
  on-device confirmation**: arm → DOWN-hold-3s sleep → wake+ring → ENTER
  dismiss → rearm, observed physically (tone + screen) or via
  `espflash monitor` log lines `Test alarm armed for HH:MM` /
  `Woke from RTC alarm; ringing`.
- [x] **Phase 2 - Alarms/todos stores + screens.** `alarms.rs` (`StoredAlarm`,
  `Repeat::{Daily,Once}`, `next_due`, `program_hardware_alarm`,
  `is_expired_once`), `todos.rs` (`Todo`), `screens.rs` (menu + Calendar +
  Alarms + Todos screens, following `provision.rs`'s self-contained-function
  + wheel-selector convention - factored the shared bits, `Nav`/`poll_nav`/
  `pick_from_list`/`enter_text`/`pick_number`, into a new `ui.rs` so
  `provision.rs` and `screens.rs` don't duplicate them). Replaces the
  counter-demo body in `main.rs` and `display.rs`'s `render_with_time`
  (now `render_home`: clock + next-alarm + pending-todo summary).
  Implemented, builds clean. **`partitions.csv` NVS resize skipped**: actual
  footprint (Wi-Fi creds + alarm/todo JSON blobs) is ~3KB against the
  existing 24KB NVS partition - no need to touch flash offsets yet; revisit
  once Phase 3's server config data adds real pressure. **Awaiting
  on-device verification**: add/toggle alarms and todos from the on-device
  menu, confirm the home screen's next-alarm/pending-todo summary updates,
  confirm a newly-added alarm still rings offline per the Phase 1 cycle.
- [x] **Phase 3 - HTTPS sync client.** `sync.rs` (`fetch_and_apply` via
  `embedded_svc::http::client::Client` + `EspHttpConnection` +
  `esp_crt_bundle_attach`, `Authorization: Bearer`/`If-None-Match`/304/ETag),
  `storage.rs` additions (`DeviceConfig { server_url, auth_token }` +
  `sync_etag`, mirroring the existing `WifiCreds` pattern), two new menu
  items in `screens.rs` (SERVER SETUP - two `ui::enter_text` prompts; SYNC
  NOW - calls `sync::fetch_and_apply` and shows the result). `docs/sync-api.md`
  written. Implemented, builds clean, flashed. Note: `serde`/`serde_json`
  were already added in Phase 2 (the alarm/todo NVS blobs needed them);
  this phase added `embedded-svc` as a direct dependency for the HTTP
  client traits. **Awaiting on-device/mock-server verification**: point
  SERVER SETUP at a `python -m http.server`-hosted static JSON file
  matching `docs/sync-api.md`'s shape, run SYNC NOW, confirm the alarms/todos
  screens reflect the fetched data and a newly-synced alarm still rings
  offline per the Phase 1 cycle.
- [x] **Phase 4 - USB control protocol.** `control.rs` (`Command`/`Reply`,
  `parse_command`/`dispatch`), `usb_console.rs` (reader thread + bounded
  `mpsc` channel, main loop drains it non-blockingly each poll cycle,
  sentinel-framed `>>IP `/`<<IP ` JSON lines so commands don't collide with
  `log::info!` output on the shared port). `docs/control-protocol.md`
  written. **Verified end-to-end on real hardware** (opened the port with
  raw `termios`/`fcntl` since no serial terminal was available headlessly,
  sent `{"cmd":"get_status"}`, got the correct typed JSON reply back).
  Two real bugs were caught only by this hardware test, both now fixed:
  1. **Boot crash.** `AlarmStore`/`TodoStore` (added in Phase 2) each
     independently called `EspDefaultNvsPartition::take()`, which is a true
     singleton (a global taken-flag, not a ref-counted handle) - the second
     and third calls failed with `ESP_ERR_INVALID_STATE` while
     `PersistedCounters`'s handle was still alive, and `main()` returned
     `Err` before ever rendering the home screen. Every boot since Phase 2
     landed was silently broken; a clean `cargo build` gave no signal of
     this. Fixed by taking the partition once in `main.rs` and cloning it
     (`EspNvsPartition` is `Arc`-backed and `Clone`) into all three
     `open()` calls.
  2. **Busy-loop log flood.** The USB reader thread used `BufReader::lines()`
     assuming blocking-until-newline semantics; this console's fd is
     actually non-blocking, so every "no data yet" surfaced as an `EAGAIN`
     `Err`, logged as a warning, in a tight loop with no backoff -
     over a million bytes of log spam in a few seconds and no commands ever
     successfully read. Fixed by reading into a byte buffer directly and
     sleeping on `ErrorKind::WouldBlock` instead of treating it as an error.
  Also discovered (documented in `docs/control-protocol.md`'s Limitations,
  not fixed - architectural, not a bug): commands are only polled from the
  Home screen's loop, so the device is unresponsive to USB commands while
  sitting in any on-device menu screen; and this board's DTR/RTS auto-reset
  wiring means opening the serial port can itself trigger a spurious ENTER
  press (observed directly during testing).
- [x] **Phase 5 - BLE control (on-demand pairing screen).** `ble_control.rs`
  (`esp32-nimble` GATT service, one WRITE + one NOTIFY characteristic,
  routed through the same `control::dispatch`), `screens.rs` "BLE PAIRING"
  menu entry, `sdkconfig.defaults` additions (`CONFIG_BT_ENABLED=y`,
  `CONFIG_BT_BLE_ENABLED=y`, `CONFIG_BT_BLUEDROID_ENABLED=n`,
  `CONFIG_BT_NIMBLE_ENABLED=y`, `CONFIG_BT_NIMBLE_HOST_TASK_STACK_SIZE=5120`),
  `docs/control-protocol.md`'s "BLE Framing" section written. NimBLE
  integration itself was clean (no build conflicts with the existing
  `zectrix_epd` C++ component), but the first pass had two silent gaps the
  agent itself flagged rather than hid - both fixed on review:
  1. **Replies never actually sent.** The notify characteristic was created
     but not stored anywhere, so `write_reply` only logged the JSON instead
     of sending it. Fixed by storing the `Arc<Mutex<BLECharacteristic>>`
     and calling `.set_value(json).notify()`.
  2. **Teardown was a no-op.** `Drop for BleControl` had a comment claiming
     "the device and server are dropped here," but the struct never
     actually held them - `BLEDevice`/`BLEServer`/`BLEAdvertising` are all
     `&'static` singleton handles in this crate, not owned values Rust's
     Drop can reclaim. Left as-is, BLE would have stayed initialized
     (~150KB RAM + radio) for the rest of the process once started once,
     defeating the entire on-demand design point. Fixed by calling the
     crate's real `BLEDevice::deinit_full()` in `Drop`, and by explicitly
     calling `BLEDevice::init()` on every `start()` (its own `take()`
     forces a process-wide `Lazy` that only runs init the *first* time -
     after a `deinit_full()`, a bare `take()` alone would silently reuse a
     stopped stack on re-entry).
  Verified on real hardware: clean build, clean boot, no regressions to
  any earlier phase with the BLE code compiled in but not started.
  **Not verified: actual BLE connectivity** (GATT discovery, write/notify
  round-trip) - no BLE test client (phone app, etc.) was available in this
  session. This still needs a real pairing test, e.g. with nRF Connect.
- [x] **Phase 6 - Finalize.** Counter demo was already fully retired back in
  Phase 2. Pairing screen wired to Phases 4/5 as they landed. Both spec
  docs (`docs/control-protocol.md`, `docs/sync-api.md`) reflect what was
  actually built, not the original plan. Added the last piece:
  `display.rs`'s `refresh_partial` now silently promotes to a full refresh
  after `PARTIAL_REFRESH_PROMOTE_LIMIT` (8) consecutive partial ones,
  matching the upstream demo's ghosting-control policy. Builds clean,
  flashed, boots clean (Wi-Fi/NTP/home screen all still working).

## Open risks

- NimBLE + this project's existing `esp-idf-sys` `extra_components`
  (the C++ `zectrix_epd` component) interaction is unverified.
- BLE's ~150KB RAM cost against actual free heap (PSRAM is caps-alloc only;
  internal SRAM is shared with I2S DMA + the EPD framebuffer) needs
  profiling in Phase 5.
- Non-blocking stdin read on `USB_SERIAL_JTAG` while the same port is also
  the `log` output has no prior art here - needs a concrete poll-vs-thread
  decision in Phase 4.
- `esp_sleep_enable_timer_wakeup` (periodic background resync) combined with
  ext1 wake and the GPIO17 hold together is untested; resync interval vs.
  battery life isn't decided (default to something conservative, e.g.
  30-60 min, and tune later).
- mbedTLS/cert-bundle defaults are expected already-on for IDF 5.5 but not
  yet confirmed against this project's actual generated sdkconfig - check
  in Phase 3 before writing `sync.rs`.

## Verification (every phase)

- `cargo +esp fmt --manifest-path rust-firmware/Cargo.toml -- --check` and
  `./scripts/build-rust.sh --release`.
- An explicit real-hardware check (flash + observe), not just a clean build.
- Before touching `partitions.csv` (Phase 2), reconfirm the full-flash
  backup (`backups/note4-factory-*.bin`) is intact.
