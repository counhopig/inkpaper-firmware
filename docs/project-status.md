# Inkpaper Project Status

Cross-repo snapshot of where things stand across all three parts of the
system, as of this session. Update this when the shape of any repo changes
significantly - it's meant to answer "what's built, what's tested, what's
left" without having to reconstruct it from commit history.

## System overview

Three repos, one device:

```
inkpaper-desktop (PC tool)          inkpaper-server (backend)
       |  USB serial / BLE                |  HTTPS GET (polling)
       |  (config only:                   |  (content: alarms[], todos[])
       |   wifi creds, server url+token)  |
       v                                  v
              inkpaper (firmware, this repo)
```

Design principle, confirmed with the user early on: the **device doesn't
own content authoring**. The PC tool only ever pushes configuration (Wi-Fi
credentials, server URL/token) over USB or BLE. Actual content (alarms,
todos) lives on the server and is pulled by the device as structured JSON
(not a pre-rendered bitmap) specifically so **alarms still ring with zero
network connectivity** - the firmware has to know alarm times itself, not
just display whatever picture the server sent.

## `inkpaper` (firmware) - most complete, most tested

Repo: this one. Latest commit: `e6b3a44`, pushed to `origin/main`.

Turned from a button-counter hardware smoke test into a real calendar /
alarm clock / todo device with USB and BLE remote configuration:

- **Calendar / Alarms / Todos / Menu screens** (`screens.rs`) replacing the
  old counter demo.
- **Offline-capable alarm**: PCF8563 hardware alarm registers + GPIO5
  deep-sleep wake + ES8311 tone (`rtc.rs`, `power.rs`, `alarms.rs`) - rings
  without Wi-Fi.
- **HTTPS sync client** (`sync.rs`) pulling alarms/todos from the server as
  JSON, with ETag/304 support.
- **USB control protocol** (`control.rs`, `usb_console.rs`): sentinel-framed
  JSON commands/replies over the existing USB-Serial-JTAG console port.
- **BLE control channel** (`ble_control.rs`): on-demand GATT service
  (~150KB RAM only while a pairing screen is open), same command schema as
  USB.
- Two spec docs for the other repos to build against:
  `docs/control-protocol.md` (USB/BLE) and `docs/sync-api.md` (HTTP sync).

**Tested on real hardware this session**:
- Full boot sequence, Wi-Fi/NTP sync, home screen rendering.
- USB command/reply round-trip (`get_status`, `set_server`, `sync_now`),
  including error-reply parsing - driven directly from this environment via
  raw serial I/O (no `espflash monitor` available headlessly).
- End-to-end: PC tool -> server registration -> pushed server config to the
  device over USB -> triggered a real sync -> device correctly requested,
  parsed, and would have applied the server's alarms/todos (this is what
  surfaced the Wi-Fi bug below).
- A serious Wi-Fi reconnect crash, found, root-caused, and fixed - see
  `docs/calendar-alarm-todo-plan.md`'s "Post-Phase-6" section for the full
  investigation. Confirmed non-crashing after the fix; **not yet confirmed
  by the agent that the device reliably comes back up and completes a
  retry after the restart it now triggers** - this needs a real
  `espflash monitor` session (the user was going to check this).

**Not yet tested on real hardware**:
- The alarm actually ringing + dismissing end-to-end (Phase 1's core
  feature - hardware alarm register logic is exercised and known to arm
  correctly, but the full wake -> ring -> ENTER-dismiss cycle wasn't
  confirmed by a human watching/listening to the device).
- Actual BLE pairing (GATT discovery, write/notify round-trip) - no BLE
  test client (phone app, etc.) was available in this environment.
- Cross-platform desktop app talking to the device over BLE (only USB was
  exercised).

## `inkpaper-desktop` (PC tool) - built, USB-tested, **not committed**

Repo: `/home/counhopig/workspace/inkpaper-desktop`. `git init`ed by
`cargo init` but **zero commits** - all ~1170 lines of Rust are currently
uncommitted working-tree files only.

Rust + `egui`/`eframe` (single toolchain, no Node/webview dependency,
native binaries for Linux/Windows/Mac via plain `cargo build`). Two tabs:

- **Device tab**: connect over USB (`serialport` crate) or BLE
  (`btleplug` crate), send Wi-Fi/server config, trigger sync, view status
  and a raw log.
- **Server tab**: register devices, manage each device's alarms/todos
  against `inkpaper-server`'s admin API.

Also has a headless CLI mode (`inkpaper-desktop --status <port>`) for
scripting/testing without the GUI.

**Tested**: USB transport against the real physical device from this
environment (`--status /dev/ttyACM0` correctly connected, sent
`get_status`, parsed the typed reply) - and the full config-push +
sync-trigger flow described above under the firmware section.

**Not tested**: BLE transport (no BLE adapter/peer available here), the
GUI itself beyond "it launches without crashing and stays up" (no way to
click through it in this environment), Windows/Mac builds (this
environment is Linux-only; a Windows cross-compile target is installed but
GUI crates typically need platform-specific system libraries not verified
here).

## `inkpaper-server` (backend) - built, fully tested, **not committed**

Repo: `/home/counhopig/workspace/inkpaper-server`. Same situation as
desktop: `git init`ed, zero commits, ~635 lines uncommitted.

Rust + `axum` + `rusqlite` (SQLite, `bundled` feature so no system
dependency). Single shared admin bearer token (personal-scale project, not
multi-tenant) guards device registration and alarm/todo CRUD; each device
gets its own bearer token for the read-only `/api/sync` endpoint matching
`docs/sync-api.md`.

**Tested**: full `curl`-driven test pass covering device registration,
alarm/todo creation (both `Daily` and `Once` repeat kinds), the sync
endpoint's exact JSON shape (byte-for-byte match against the spec doc's
example), ETag caching (200 then 304 on an unchanged request), and auth
rejection (missing/wrong token on both the admin and device-facing
surfaces). Also exercised for real against the physical device (see
firmware section above).

## Known issues / open items

1. **`inkpaper-desktop` and `inkpaper-server` have no commits yet.** Say
   the word and I'll commit them (mirroring how the firmware work was
   committed) - didn't do this unprompted since it wasn't asked for.
2. **Wi-Fi reconnect crash mitigation needs a real-monitor confirmation.**
   The crash itself is fixed and confirmed non-crashing across repeated
   tests; whether the device's restart-and-retry flow is fully smooth from
   a user's perspective needs an `espflash monitor` session (this
   environment's headless serial hacks weren't reliable enough to confirm
   the post-restart reconnect on their own).
3. **Physical alarm ring/dismiss test still pending** - the single most
   important feature of this whole build (offline alarm) has the
   underlying mechanism verified but not a full human-observed
   ring-to-dismiss cycle.
4. **BLE is entirely unverified end-to-end** - both the firmware's GATT
   server and the desktop app's `btleplug` client compile and are built to
   the same documented contract, but have never actually talked to each
   other or to a phone.

## Suggested next steps

- Commit `inkpaper-desktop`/`inkpaper-server` if you want them preserved
  (currently only exist as this session's working tree).
- Physical tests: alarm ring/dismiss, BLE pairing (device menu <-> desktop
  app or a phone BLE app), and the Wi-Fi-restart-retry flow via
  `espflash monitor`.
- If BLE testing surfaces its own issues, expect a similar depth of
  investigation to the Wi-Fi saga - it's the least-exercised subsystem in
  the whole stack right now.
