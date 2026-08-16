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

## Post-Phase-6: Wi-Fi reconnect crash (found via real end-to-end testing)

Testing the full desktop -> server -> device sync loop against real hardware
(not just `cargo build`) surfaced a serious bug none of the earlier
phase-by-phase testing caught: **a second Wi-Fi connect within one boot
session reliably crashes** (`Guru Meditation Error`, three different
signatures across three different mitigation attempts - `InstrFetchProhibited`
on a second `EspWifi::new()`, the same on a second `esp_wifi_start()` after
`esp_wifi_stop()`, and `Unhandled debug exception`/`BREAK instr` partway
through a second connection's lifecycle with neither of those). Confirmed via
web research to match a class of known, unresolved upstream bugs
(espressif/esp-idf#7579, #11171; esp-rs/esp-idf-svc#503) with no official fix
in this ESP-IDF/esp-idf-svc version - not something fixable by calling the
API differently.

**Fix**: `wifi::WifiManager` now owns the one shared `EspWifi` instance for
the whole process and tracks whether it's been used; every non-boot Wi-Fi
user (`sync::sync_now`, `provision::run`, `control.rs`'s `SetWifi` handler)
checks this and, if Wi-Fi was already used this session, calls
`wifi::restart_for_fresh_wifi_session()` instead of reconnecting in-process -
so the next connect is always a fresh boot session's guaranteed-safe first
one. The obvious choice for that restart, `esp_restart()`, turned out to hit
the *same* crash class (apparently doing its own internal Wi-Fi-aware
teardown) - it goes through `power::enter_deep_sleep_with_wakeups` with a
~100ms timer wake instead, since deep sleep after an already-used Wi-Fi
session has been exercised reliably many times elsewhere in this project
(every DOWN-hold-3s sleep following a boot-time NTP sync).

Also fixed en route: pinned the USB console reader thread to Core0
(`esp_idf_hal::task::thread::ThreadSpawnConfiguration`), since a background
thread on the other core executing flash-cached code during an NVS/flash
write is a plausible contributor to this class of `Cache error` crash -
though pinning alone did not fully explain the crash (the `esp_restart()`
variant still crashed after pinning), so the deep-sleep-based restart is
the actual fix, not this.

**Also investigated and ruled out**: whether this was an esp-idf-svc
(Rust wrapper) bug specifically, following a matching-looking upstream
report (esp-rs/esp-idf-svc#503: a second `connect()` on one `EspWifi`
panics inside the wrapper's own state tracking). `WifiManager::connect`/
`disconnect` were rewritten to call raw `esp_wifi_connect()`/
`esp_wifi_disconnect()` FFI directly, bypassing `EspWifi`'s wrapper
methods entirely, matching the low-level sequence used by
github.com/qiujun8023/slate - a working C++ reference firmware for this
exact NOTE4 hardware that reconnects Wi-Fi repeatedly without crashing.
**Tested with a real second connect (the restart guard temporarily
disabled): identical crash, same PC address**, with or without the
wrapper involved - so it's a lower-level ESP-IDF/hardware issue, not
specific to esp-idf-svc. Slate's actual difference from this codebase is
that it reconnects synchronously from inside its own
`WIFI_EVENT_STA_DISCONNECTED` event handler, immediately on disconnect,
rather than from arbitrary application code potentially much later -
that context/timing difference is the leading remaining hypothesis for
why theirs works, but reproducing it would mean restructuring Wi-Fi
handling around the event loop rather than synchronous calls, which
hasn't been attempted (the restart-based workaround is the shipped fix).
The raw FFI calls were kept regardless, since they're no worse than the
wrapper and match Slate's proven low-level sequence.

**Trade-offs accepted, not solved**: `sync::sync_now`'s restart no longer
auto-resumes the sync after rebooting (an earlier version tried this,
reusing the boot's own fresh connection - that traded the reconnect crash
for a *third* crash signature, apparently from chaining Wi-Fi connect + NTP
+ HTTPS/TLS too tightly in one boot sequence with no settling time). The
caller (menu, USB, or BLE) just needs to retry after the restart completes.
`control.rs`'s `SetWifi` also skips its usual verify-before-save step in
this fallback path (no clean way to reply-then-restart over USB/BLE), so
credentials are saved unverified and implicitly checked by the next boot's
own connect attempt instead.

**Verified**: the crash itself is gone - reproduced non-crashing across
repeated `sync_now` calls after the fix (previously 100% reproducible).
**Not independently verified by the agent**: whether the device reliably
finishes the deep-sleep-based restart and resumes normal operation
afterward - headless testing via raw `termios`/DTR manipulation (no real
serial terminal was available in this environment) could not reliably
re-establish a serial connection after a genuine deep-sleep wake cycle
(the port node reliably reappears and `espflash` can always reach the
chip, but ad-hoc port reads afterward inconsistently returned no data -
suspected to be a test-methodology/USB-re-enumeration-timing artifact,
not a firmware issue, but not confirmed either way). **Needs a real
`espflash monitor` session to confirm**: trigger `sync_now` twice in a row
and watch the device restart cleanly and come back up between them.

## Verification (every phase)

- `cargo +esp fmt --manifest-path rust-firmware/Cargo.toml -- --check` and
  `./scripts/build-rust.sh --release`.
- An explicit real-hardware check (flash + observe), not just a clean build.
- Before touching `partitions.csv` (Phase 2), reconfirm the full-flash
  backup (`backups/note4-factory-*.bin`) is intact.
