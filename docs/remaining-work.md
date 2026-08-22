# Firmware Remaining Work

Updated: 2026-08-22  
Flashed revision: `9e7e9b3` (`main`)

## Newly discovered from desktop/device logs

1. ~~**P0 — Todo reminder date cannot be persisted.**~~ **Fixed and verified
   on physical hardware** (2026-08-22): synced a real high-priority due-today
   Todo down from the server; the device rang `TODOS DUE` once with no
   `ESP_ERR_NVS_KEY_TOO_LONG` in the log. NVS key renamed
   `todo_reminded_date` (18 chars) -> `todo_rem_date` (13 chars); see
   `storage.rs`.
2. ~~**P0 — A persistence failure still presents the reminder.**~~ **Fixed.**
   `remind_due_todos` now returns without ringing when
   `set_todo_reminded_date` fails; see `reminders.rs`.
3. **P1 — Long reminder screens delay USB replies.** *Partially fixed:* the
   due-todo and urgent-inbox reminder loops in `reminders.rs` now poll the USB
   console and reply `{"status":"busy"}` (dropping, not queueing, the command)
   instead of leaving the client waiting in silence — see the `Busy` reply in
   `control-protocol.md`. Still open: the RTC alarm-ringing screen
   (`alarms.rs::ring_until_dismissed`) has the identical blocking pattern but
   was not wired up to the same fix (out of scope of this pass — it's called
   from a pre-`DeviceContext` boot path in `main.rs` in addition to the normal
   runtime path, so threading `UsbConsole` through it needs its own look).
   BLE is also untouched: `BleControl` isn't reachable from `reminders.rs`'s
   call path (it's owned by `main.rs`, not `DeviceContext`), so BLE commands
   during a reminder still get no reply at all, not even `busy`.
   **Verified on physical hardware** (2026-08-22): sent `get_status` over USB
   while the due-todo reminder from item 1 was actively ringing and got
   `{"status":"busy"}` back in a few seconds - well inside the 45s desktop
   timeout, and no more silent hang.
4. **P1 — Command correlation is implicit.** The serial protocol has no
   request ID, so a late reply can be consumed by whichever desktop request is
   currently waiting. Add request IDs to a backward-compatible protocol
   version, or enforce exactly one in-flight command and quarantine replies
   received after a timeout before permitting another command. Not started —
   this needs a coordinated change with `inkwash-desktop` (the client side),
   not just firmware.
5. **P2 — Startup status is requested more than once.** During the USB reset
   and boot sequence the desktop sends `get_status` twice and receives two
   valid status replies. This is not a firmware failure, but the UI/logging
   should coalesce identical startup probes so users do not mistake them for
   duplicate actions.
6. **P2 — `esp-idf-svc` 0.52.1 always associates with PMF advertised as
   unsupported, ignoring `ClientConfiguration.pmf_cfg`.** Root-caused
   2026-08-22 on hardware: a router with WPA2/WPA3-mixed + PMF-required
   rejected the device shortly after association (`assoc -> init` right
   after the `Wi-Fi connected` log line, before DHCP could run), while the
   same router in WPA2-only mode connected cleanly. Traced to
   `esp-idf-svc-0.52.1/src/wifi.rs`'s
   `TryFrom<&ClientConfiguration> for Newtype<wifi_sta_config_t>`, which
   hardcodes `wifi_pmf_config_t { capable: false, required: false }` and
   never reads `conf.pmf_cfg` — so nothing `wifi.rs::connect()` sets on the
   Rust side can change it. Workaround for now: use a WPA2-only network (or
   WPA2/WPA3-mixed with PMF optional, not required). A real fix needs either
   an `esp-idf-svc` upgrade past this bug, or bypassing `set_configuration()`
   for STA the same way `connect()`/`disconnect()` already bypass the
   wrapper - build and pass a `wifi_config_t` with `pmf_cfg.capable = true`
   directly to `esp_wifi_set_config()`. Not started.

The same log also confirms that consecutive scan-free Wi-Fi connections work
within one boot, HTTPS certificate validation succeeds, and a full sync applies
alarm/Todo/Inbox state. Those paths should remain unchanged while fixing the
issues above.

## Verification completed in this pass

- ESP32-S3 release build completed successfully.
- Firmware flashed through `/dev/tty.usbmodem1101` using DIO, 80 MHz and a
  16 MB flash layout.
- Boot ROM confirmed ESP32-S3 revision 0.2, DIO mode, 16 MB flash and 8 MB
  PSRAM; the PSRAM memory test passed.
- The application initialized the power latch, RTC, display and Wi-Fi stack
  without a watchdog reset or panic.
- The RTC retained a valid clock and the display completed full and partial
  refreshes.
- An aligned background network cycle connected, obtained DHCP and validated
  the HTTPS certificate after boot.

### 2026-08-22 hardware pass (items 1 and 3 above)

- `set_wifi` reconfigured live (previous stored network was unreachable from
  this location); verification correctly rejected a bad DHCP-timeout attempt
  before saving, then saved once a working network connected - see item 6 for
  the PMF root cause of the first attempt's failure.
- `sync_now` over USB pulled 2 Todos from the live server (0 alarms, 0 inbox);
  `Wi-Fi already used this boot session; attempting a second connect
  (scan-free)` fired twice more in the same boot (once for `sync_now`, once
  for the next urgent-poll boundary) and both reconnected and completed
  cleanly - the documented "multiple connects per boot are safe" fix in
  `wifi.rs` still holds.
- The due-Todo reminder fired exactly once (item 1) and, while it was
  actively ringing, a `get_status` sent over USB got `{"status":"busy"}`
  back (item 3) instead of hanging silently.
- Opening the USB serial port with a naive client (pyserial's default
  `Serial(port, ...)` constructor, which asserts DTR/RTS before the caller
  can change them) reliably resets the chip - a full reboot from the
  bootloader, not just the documented spurious-ENTER GPIO0 pull. Not a
  firmware bug (same auto-reset circuit `espflash` itself relies on to
  attach), but `control-protocol.md`'s existing warning undersells the
  effect; worth a doc pass for PC-tool authors on setting `dtr`/`rts` to
  `False` *before* opening the port.

## Required physical-device verification

1. **Alarm end-to-end:** test RTC alarm wake from deep sleep, audible ring,
   ENTER dismissal, AF acknowledgement, one-shot removal and re-arming of the
   next recurring alarm.
2. **Alerts outside Home:** verify alarm, urgent Inbox and high-importance Todo
   screens interrupt Calendar, Settings, list/detail and BLE pairing screens,
   then return to a correctly redrawn page.
3. **Connection soak:** repeatedly disconnect/reconnect BLE and unplug/replug
   USB over a long-running session; confirm BLE teardown does not affect later
   Wi-Fi synchronization.
4. **Repeated Wi-Fi cycles:** leave the device running across many urgent and
   full-sync boundaries and confirm there is no second-connect crash or heap
   degradation.
5. **Large data sets:** exercise maximum practical alarm, Todo and Inbox lists;
   check pagination, truncation, NVS capacity and e-paper ghosting.

## Remaining engineering work

1. Add host-runnable tests for alarm ordering/recurrence, scheduler boundary
   transitions, reminder de-duplication, ID allocation and sync merge rules.
2. Add a repeatable hardware smoke-test checklist or serial-log harness for
   boot, synchronization, alarm and BLE/USB recovery.
3. ~~Fix stale ESP-IDF application metadata.~~ *Worked around* (2026-08-22):
   the ESP-IDF app descriptor's `App version`/`Compile time` fields are still
   stale (confirmed on hardware: still printed `v0.3.0-14-g71062c9-dirty`
   after flashing `9e7e9b3`) and forcing them to refresh would mean touching
   `esp-idf-sys`'s CMake caching, out of scope here. Instead `build.rs` now
   embeds a fresh `git describe` as `GIT_REV`, logged once at boot
   (`main.rs`) - verified on hardware printing the correct
   `v0.3.0-18-g672e229-dirty` on the same boot where the ESP-IDF field was
   stale. Use the firmware's own log line, not `App version`, to identify
   what's actually running.
4. Replace machine-specific ESP-IDF, Python and rust-analyzer paths with a
   documented local bootstrap/configuration mechanism. *Partially done*
   (2026-08-22): `LIBCLANG_PATH` no longer needs a hardcoded per-developer
   path in `rust-firmware/.cargo/config.toml` - `scripts/build-rust.sh`/
   `.ps1` now locate it dynamically under the active `esp` rustup toolchain
   (verified end-to-end on Linux with a clean `target/` rebuild; the
   PowerShell side is unverified - no Windows machine available). Still
   open: `.vscode/settings.json` (rust-analyzer paths) and the `IDF_PATH`
   fallback in `config.toml` remain machine-specific; the former was
   untracked from git so per-developer edits stop showing up as diffs.
5. Design and implement OTA with signed images, health confirmation and a
   rollback partition before enabling remote firmware upgrades.

## Release blockers

- Do not claim the offline-alarm feature verified until item 1 in the physical
  test list passes on the actual NOTE4 hardware.
- Do not ship OTA until rollback and power-loss behavior are demonstrated.
- Continue flashing only ESP32-S3 NOTE4 images in DIO mode; NOTE4C firmware and
  QIO mode remain forbidden.
