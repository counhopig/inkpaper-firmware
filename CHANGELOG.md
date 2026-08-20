# Changelog

All notable changes to **inkpaper-firmware** are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-08-20

### Added
- **PC preview tool** (`tools/preview`) — renders the home screen and
  sub-screen mockups to PNG on the host using the real `home.rs` /
  `canvas` / font / icon tables, so the on-device visual language can be
  iterated without flashing.
- **5×7 tiny font and rule-separated week view** — the calendar week
  view uses a compact 5×7 font with rule separators, and long todos wrap
  inside the week-view columns.
- **GO TO left-hand overlay bar** — the navigation drawer now slides out
  as a left-hand overlay bar covering the current page, with a border
  and selection box for the highlighted entry.
- **Unified home visual language** — all on-device screens (home,
  calendar, alarms, todos, settings) share one consistent paper + ink
  visual language.
- **Local release script** (`scripts/release.sh`) — builds the release
  ELF locally, tags, and publishes a GitHub Release with the firmware
  asset.

## [0.1.0] - 2026-08-19

### Added
- Initial release: complete offline-first calendar / alarm / todo
  firmware for the Zectrix Note 4 (ESP32-S3, 4.2″ SSD2683 e-paper).
- **Offline-capable alarms** — daily / weekly / monthly / one-shot
  schedules rung by the PCF8563 hardware alarm, with deep-sleep wake,
  no network dependency.
- **Interactive calendar** — month grid with due-today dots; pick a day
  to open a week view listing what's due.
- **Todos** — importance (low/med/high), due dates, repeat schedules,
  one-shot reminders for high-priority items.
- **Bidirectional HTTPS sync** — `POST /api/sync` uploads local
  `enabled`/`done` flags and applies the server's authoritative lists;
  periodic auto-sync (1/5/10/30/60 min).
- **USB + BLE control protocol** — `set_wifi`, `set_server`,
  `set_timezone`, `sync_now`, `get_status`, `clear_alarms` over USB
  Serial/JTAG or BLE GATT.
- **Deep sleep and wake** with GPIO17 power latch and ticking clock.
- Apache-2.0 license and open-source README with screenshots.
