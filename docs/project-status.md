# Inkpaper Project Status

Cross-repo snapshot of where things stand across the Inkpaper system, as of
2026-08-21 (v0.3.0). Update this when the shape of any repo changes
significantly - it's meant to answer "what's built, what's tested, what's
left" without having to reconstruct it from commit history.

## System overview

Four repos, one device, all public on GitHub under `counhopig`:

```
inkpaper-desktop (PC tool)          inkpaper-server (backend)
       |  USB serial / BLE                |  HTTPS POST (device pushes
       |  (config only:                   |  locally-changed flags,
       |   wifi creds, server url+token,  |  server merges + returns the
       |   timezone)                      |  authoritative lists)
       v                                  v
              inkpaper (firmware)    <-    inkpaper-mcp (MCP server)
              Zectrix Note 4              (any agent can push webhook
                                          notifications to the device)
```

Design principle: the **device doesn't own content authoring**. The PC tool
only pushes configuration over USB/BLE; content (alarms, todos, inbox) lives
on the server; the device syncs it as structured JSON - so **alarms still
ring with zero network connectivity**. Since v0.3.0 sync is **two-way**: the
device uploads only flags it changed locally (dirty-set tracking), so edits
made in the server/desktop UIs survive the next sync.

## `inkpaper` (firmware)

Repo: `counhopig/inkpaper-firmware`. Rust (esp-idf), 400×300 SSD2683 EPD.

Implemented and verified on real hardware:

- **Home / Calendar / Alarms / Todos / Settings / Inbox screens**, reached
  via a long-UP/DOWN navigation drawer. Full-screen URGENT reminders with a
  two-tone siren for high-priority alert messages (dismiss on ENTER press).
- **Offline-capable alarm**: PCF8563 hardware alarm + deep-sleep wake +
  ES8311 tone - rings without Wi-Fi.
- **Cron-aligned sync**: urgent polls at each :00/:30 wall-clock boundary
  (lightweight `X-Inkpaper-Poll`), full syncs at every `interval` boundary,
  once-per-day NTP resync of the PCF8563 (verified: RTC drift corrected).
- **Full GB2312 CJK fonts** (16×16 + 12×12, Noto Sans SC, 7445 chars) with
  width-aware truncation everywhere; mixed ASCII+CJK rendering verified on
  device with Chinese notifications.
- **Two-way sync** (dirty-set upload) verified end-to-end: a todo marked
  done on the server survives the device's next sync.
- **USB + BLE control protocol** (six commands), on-demand BLE pairing
  screen.
- **e-paper UI preview** (`tools/preview`) renders every screen to PNG on a
  PC using the real font/icon tables.

**Not yet verified on real hardware** (v0.3.0):

1. Alarm ring/dismiss end-to-end (wake → ring → ENTER-dismiss).
2. Long-running BLE disconnect and USB replug recovery.
3. Large alarm/todo lists: pagination and EPD ghosting at realistic sizes.
4. File system + OTA/rollback are not implemented at all.

Known, worked-around issues: a second in-process Wi-Fi `connect()` used to
crash (fixed by removing the redundant `esp_wifi_scan_start()`); the Wi-Fi
singleton constraints (`never esp_wifi_stop()`/`esp_restart()`) and the
deep-sleep restart path are documented in `rust-firmware/src/wifi.rs`.

## `inkpaper-server` (backend)

Repo: `counhopig/inkpaper-server`. Rust + axum + sqlx (`Any` driver: SQLite
or PostgreSQL), embedded Vue 3 admin console.

- Admin API (single `ADMIN_TOKEN` + console accounts with Argon2id
  passwords, per-account device ownership).
- Device-facing `POST /api/sync`: merges locally-changed flags, ETag/304
  caching, `inbox_read_acked` echo, `inbox_truncated` flag, and the
  lightweight `X-Inkpaper-Poll` urgent check.
- **Webhook channels**: per-device channels with one-time delivery tokens,
  `POST /api/channels/:id/messages` with idempotency keys, size limits and
  `priority: high` urgent flagging. Inbox admin endpoints.
- Console: dashboard / device (alarms+todos+channels+inbox) / account views.

Verified via curl-driven API passes and end-to-end against the physical
device. Deployment is LAN-only HTTP (a LAN Linux host); public HTTPS with a
real certificate chain remains untested.

## `inkpaper-desktop` (PC tool)

Repo: `counhopig/inkpaper-desktop`. Tauri 2 + Vue 3 + TypeScript + Pinia.

- Overview / Device (USB+BLE config push, sync trigger, status) / Content
  (device registration, alarm/todo authoring, **channels & inbox
  management**) / Logs (redacted, mirrored to disk).
- Headless CLI mode (`--status`, `--sync`, `--ble-scan`, `--ble-list`).

USB and BLE were verified against the real device under the previous egui
build; the Tauri/Vue rewrite was exercised with the real device for USB
status/sync during v0.2.0/v0.3.0 development. Windows/Linux bundles and
platform permission behavior remain untested (macOS only).

## `inkpaper-mcp` (MCP server)

Repo: `counhopig/inkpaper-mcp`. Node/TypeScript (Bun), stdio MCP server
exposing a single `notify` tool that posts to a channel webhook.

- `priority: high` → device shows the URGENT full-screen reminder + siren
  within one 30 s poll cycle (verified end-to-end).
- Generic across agents: opencode (global plugin with lifecycle hooks),
  Claude Code (hooks in `~/.claude/settings.json`), Codex (`notify` +
  `hooks.json`), or any MCP client / plain HTTP.
- Config via env vars only (`INKPAPER_SERVER_URL`, `INKPAPER_CHANNEL_ID`,
  `INKPAPER_WEBHOOK_TOKEN`) - no secrets in the repo.

## Known issues / open items

1. Alarm ring/dismiss end-to-end on real hardware - the core offline-alarm
   promise; register logic is verified, the human-observed cycle is not.
2. Public HTTPS + certificate chain for the server (LAN-only today).
3. Windows/Linux Tauri bundles and platform permission behavior.
4. Device-created alarms (on-device "ADD ALARM" is daily-only) and on-device
   content authoring remain intentionally limited.
5. File system + OTA/rollback are not implemented.

## Suggested next steps

- Physical tests, roughly in priority order: alarm ring/dismiss, BLE/USB
  long-running recovery, large-list EPD behavior.
- Public HTTPS deployment for the server (reverse proxy + cert).
- File system + OTA/rollback design once the real-hardware gaps are closed.