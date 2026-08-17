# Inkpaper Project Status

Cross-repo snapshot of where things stand across all three parts of the
system, as of 2026-08-17. Update this when the shape of any repo changes
significantly - it's meant to answer "what's built, what's tested, what's
left" without having to reconstruct it from commit history.

> For the full narrative (design decisions, bugs found and fixed, real
> debugging sessions, deployment steps) see
> `../../INKPAPER_ENGINEERING_HISTORY.md` at the workspace root (outside all
> three git repos - not yet under version control). This file is the short
> version; that one is the long version. Where the two disagree, trust the
> engineering history doc and update this file to match.

## System overview

Three repos, one device, all committed and pushed to their own `origin/main`
(self-hosted git host, one remote per repo - see each repo's `git remote -v`):

```
inkpaper-desktop (PC tool)          inkpaper-server (backend)
       |  USB serial / BLE                |  HTTPS POST (device pushes
       |  (config only:                   |  local enabled/done flags,
       |   wifi creds, server url+token,  |  server merges + returns the
       |   timezone)                      |  authoritative alarms[]/todos[])
       v                                  v
              inkpaper (firmware, this repo)
```

Design principle, confirmed with the user early on: the **device doesn't
own content authoring**. The PC tool only ever pushes configuration (Wi-Fi
credentials, server URL/token, timezone) over USB or BLE - never text. Actual
content (alarms, todos) lives on the server; the device pulls/pushes it as
structured JSON (not a pre-rendered bitmap) specifically so **alarms still
ring with zero network connectivity** - the firmware has to know alarm times
itself, not just display whatever picture the server sent. The device can
locally flip an alarm's `enabled` or a todo's `done`, and uploads only those
mutable flags on the next sync; text, schedules, additions, and deletions are
never device-authored.

## `inkpaper` (firmware)

Repo: this one. Latest commit: `f41555b` (`feat: support local timezone
(set_timezone) and clear-all-alarms (clear_alarms)`), pushed to
`origin/main`.

A real calendar / alarm clock / todo device, not a hardware smoke test:

- **Home / Calendar / Alarms / Todos / Settings screens** (`screens.rs`),
  reached via a long-UP/DOWN navigation drawer (not a menu tree) - see
  `README.md` and `rust-firmware/AGENTS.md` for the current structure.
  Settings holds Sync Now / BLE Pairing / Sleep. There is no on-device
  Wi-Fi or server-config screen anymore - `provision.rs` (the on-device
  Wi-Fi wizard) and the status LED blink were both removed in the same
  refactor (`e8f7d5f`) that switched sync to POST and rewrote the UI
  around this drawer.
- **Offline-capable alarm**: PCF8563 hardware alarm registers + GPIO5
  deep-sleep wake + ES8311 tone (`rtc.rs`, `power.rs`, `alarms.rs`) - rings
  without Wi-Fi.
- **Bidirectional HTTPS sync client** (`sync.rs`): POSTs local alarm
  `enabled` / todo `done` flags, applies the server's merged, authoritative
  lists back. Legacy `GET` + `If-None-Match`/304 is still served by
  `inkpaper-server` for older firmware, but current firmware always POSTs.
- **USB control protocol** (`control.rs`, `usb_console.rs`): sentinel-framed
  JSON commands/replies over the existing USB-Serial-JTAG console port, six
  commands (`set_wifi`, `set_server`, `sync_now`, `get_status`,
  `clear_alarms`, `set_timezone`). `usb_console.rs` polls inline from the
  main loop now - no dedicated reader thread/channel.
- **BLE control channel** (`ble_control.rs`): on-demand GATT service
  (~150KB RAM only while the BLE Pairing screen is open), same command
  schema as USB.
- Two spec docs for the other repos to build against:
  `docs/control-protocol.md` (USB/BLE) and `docs/sync-api.md` (HTTP sync).
- **Known, worked-around issue**: a second in-process Wi-Fi `connect()`
  reliably crashes (unresolved upstream ESP-IDF/esp-idf-svc bug class, not
  an application-level bug - confirmed by testing raw `esp_wifi_connect()`
  FFI directly, still crashes). Worked around by tracking `used()` and
  restarting via `power::restart_via_deep_sleep` (never `esp_restart()`,
  which hits the same crash class) before any second connect attempt, so
  the crashing code path is never actually exercised. Full investigation:
  `docs/calendar-alarm-todo-plan.md`'s "Post-Phase-6" section and
  `INKPAPER_ENGINEERING_HISTORY.md` §5.2.

**Tested on real hardware** (see `INKPAPER_ENGINEERING_HISTORY.md` §10 for
the full list): boot sequence, Wi-Fi/NTP sync, home screen rendering, USB
command/reply round-trip (including `set_wifi`/`set_server` writes), a real
BLE connection with a working write + notify round-trip (`BLE connected` /
`OK` reply), an end-to-end sync (PC tool -> server registration -> pushed
server config over USB -> triggered sync -> device applied the server's
alarms/todos, with the server logging the POST), and the Wi-Fi
reconnect-crash workaround (confirmed non-crashing across repeated tests).

**Not yet tested on real hardware**:
- The alarm actually ringing + dismissing end-to-end (hardware alarm
  register logic is exercised and known to arm correctly, but the full wake
  -> ring -> ENTER-dismiss cycle hasn't been confirmed by a human
  watching/listening).
- Whether the device's restart-and-retry flow after the Wi-Fi
  already-used-this-boot restart is fully smooth from a user's perspective
  (needs an `espflash monitor` session watching the full cycle).
- Large alarm/todo lists: screen pagination and EPD ghosting behavior
  haven't been exercised with realistic list sizes.

## `inkpaper-desktop` (PC tool)

Repo: `../inkpaper-desktop`. Latest commit: `747fae4` (`docs: rewrite
README for Tauri + Vue architecture`), pushed to `origin/main`.

**Rewritten from egui/eframe to Tauri 2 + Vue 3 + TypeScript + Pinia**
(`807be6e` onward) - the egui UI worked but layout/visual polish was slow to
iterate on, so the stack was swapped rather than continuing to patch it (see
`INKPAPER_ENGINEERING_HISTORY.md` §7-8 for the full reasoning and the
egui-era bugs that motivated it). Four pages: Overview, Device (USB/BLE,
Wi-Fi/server/timezone config, Sync Now), Content (device registration,
alarm/todo management against `inkpaper-server`'s admin API), Logs
(real-time diagnostics, mirrored to a platform log directory, secrets
redacted). Also has a headless CLI mode (`--ble-scan`, `--ble-list`,
`--status <port>`, `--sync <port>`) for scripting without the GUI.

**Tested under the previous egui build**: USB transport against the real
device (status/config-push/sync-trigger), BLE transport against the real
device (a GATT connect + write + notify round-trip that previously hung
forever due to a worker-thread bug - fixed by returning success as soon as
the GATT handshake/subscribe completes, rather than waiting for the
long-lived notification loop to end, which it never does by design).

**Not yet re-tested under the current Tauri/Vue build**: neither the USB nor
the BLE real-device flow has been re-run since the egui -> Tauri/Vue
rewrite. The protocol/transport code (`src/protocol.rs`, `src/transport/`)
carried over largely unchanged, but this hasn't been confirmed against real
hardware post-rewrite. Also not yet tested: Windows/Linux Tauri bundle and
platform permission behavior (only macOS has been exercised).

## `inkpaper-server` (backend)

Repo: `../inkpaper-server`. Latest commit: `87ef1b2` (`feat: device push
sync, admin console, bulk clear, and validation hardening`), pushed to
`origin/main`.

Rust + `axum` + `rusqlite` (SQLite, `bundled` feature, no system
dependency). Single shared admin bearer token (personal-scale project, not
multi-tenant) guards device registration and alarm/todo CRUD; each device
gets its own bearer token for `/api/sync`. Both `GET /api/sync` (legacy,
ETag/304) and `POST /api/sync` (current firmware's bidirectional push) are
served; `POST` merges only known alarm/todo IDs' `enabled`/`done` flags and
never lets device data create new content. Built-in `/` admin console
(register devices, copy the one-time device token, manage alarms/todos) uses
the same paper-grey/ink-black design language as the Desktop app.

**Deployed**: running on a LAN host (`ssh office-linux-server-local`, repo
at `/home/tomzhu/Documents/mywork/inkpaper-server`, listening on
`0.0.0.0:8080`, reachable on the LAN at `http://192.168.31.29:8080`).
Deploy flow is `scp` changed sources, `cargo build --release` remotely,
restart the process, `curl -f http://127.0.0.1:8080/health`. `.env` and
`inkpaper.sqlite3` on the remote host must be preserved across deploys (not
overwritten by a local empty database). Not yet tested: public HTTPS with a
real certificate chain - current deployment is LAN-only plain HTTP.

**Tested**: full `curl`-driven pass covering device registration, alarm/todo
CRUD (both `Daily` and `Once` repeat kinds, bulk clear), the sync endpoint's
JSON shape (byte-for-byte match against the spec doc), ETag caching (200
then 304 on `GET`), auth rejection (admin and device-facing). Also exercised
for real against the physical device end-to-end (registration through the
admin console, `POST /api/sync` receiving and merging real device state -
see firmware section above), and the responsive admin console UI checked at
both desktop and phone widths.

## Known issues / open items

Consolidated from `INKPAPER_ENGINEERING_HISTORY.md` §11 - none of these are
safe to treat as done just because the relevant code compiles:

1. **Re-verify USB and BLE real-device flows under the current Tauri/Vue
   Desktop.** Both were verified end-to-end, but under the previous egui
   build; the rewrite hasn't been re-confirmed against real hardware.
2. **Alarm ring/dismiss has never been observed end-to-end on real
   hardware** - the single most important feature of this build (offline
   alarm). Register read/write logic is verified; the full
   wake-ring-dismiss cycle by a human is not.
3. **Wi-Fi reconnect-crash mitigation's restart-and-retry UX** needs a real
   `espflash monitor` session watching the full cycle (clean restart, then
   a successful reconnect+sync on retry).
4. **Long-running BLE disconnect and USB replug recovery** haven't been
   exercised.
5. **Public HTTPS + certificate chain for the server** is untested - current
   deployment is LAN-only HTTP.
6. **Windows/Linux Tauri bundles and platform permission behavior** are
   untested - only macOS has been exercised.
7. **Large alarm/todo lists**: screen pagination and EPD ghosting under
   realistic list sizes haven't been exercised.
8. **File system and OTA/rollback are not implemented** at all yet.

## Suggested next steps

- Physical tests, roughly in priority order: alarm ring/dismiss (the core
  offline-alarm promise), USB/BLE re-verification under Tauri/Vue Desktop,
  the Wi-Fi restart-retry UX via `espflash monitor`.
- File system + OTA/rollback design, once the above real-hardware gaps are
  closed (shipping OTA on top of unverified recovery paths is backwards).
- If BLE or Wi-Fi testing surfaces new issues, expect the same depth of
  investigation as the existing sagas in
  `docs/calendar-alarm-todo-plan.md`'s Post-Phase-6 section and
  `INKPAPER_ENGINEERING_HISTORY.md` §5 - these are the least forgiving
  subsystems in the whole stack.
