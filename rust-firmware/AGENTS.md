# rust-firmware — inkwash-note4 crate

## OVERVIEW
The single crate containing all product code: ESP32-S3 firmware with 21 flat src modules (no subdirectories, including `main.rs`) + 1 C++ EPD FFI component. Entry point `src/main.rs:85` (`[[bin]] inkwash-note4`).

## MODULE MAP (src/)
| Group | Module | Responsibility |
|------|------|------|
| Orchestration | `main.rs` | boot sequence + 20ms polling main loop; the two singletons (NVS handle L97-105, WifiManager L214) are created here |
| Hardware | `board.rs` | central assembly point `Note4Board::take()`; GPIO/ADC/shared I2C0 (single instance of `Rc<RefCell<I2cDriver>>`) |
| Hardware | `power.rs` | deep sleep/wake-up reasons/GPIO17 RTC hold; `enter_deep_sleep_with_wakeups` is the only safe restart path |
| Hardware | `rtc.rs` | PCF8563 driver + the single hardware alarm register (L165-170) |
| Hardware | `button.rs` / `watchdog.rs` / `audio.rs` / `nfc.rs` | debounce + short press/1s long press; task WDT; ES8311; GT23SC6699 |
| Rendering | `canvas.rs` / `font8x16.rs` / `home.rs` / `display.rs` | 1bpp framebuffer + stroke_rect outline; proportional-width glyphs; **pure home-screen layout (no EPD FFI, PC-previewable via `tools/preview`)**; EPD FFI wrapper + `render_home` delegation |
| UI | `ui.rs` / `screens.rs` | 3-button generic components (list selection, nav drawer); nav drawer/calendar/alarms/todos/settings menu (entered via long-press UP/DOWN, no on-device text input). `ui::header` carries the home screen's ink-square brand block; `pick_from_list` returns `PickResult::{Selected,Cancelled,OpenNav}` so long-press UP/DOWN opens the GO TO drawer from Settings too. |
| Services | `alarms.rs` / `todos.rs` / `storage.rs` | NVS store; alarms picks the soonest one to arm PCF8563 |
| Protocol | `control.rs` / `usb_console.rs` / `ble_control.rs` / `sync.rs` / `wifi.rs` | `>>IW `/`<<IW ` command protocol (USB/BLE share `control::dispatch`); HTTPS sync; WifiManager |

## FFI COMPONENT: components/zectrix_epd/
Official SSD2683 C++ driver (784 LOC `.cc`), generating the `zectrix_epd` module via Cargo.toml `extra_components` + bindgen. `private_include/ssd2683_waveform.h` is calibrated waveform data, **must not be deleted**. EPD power/timing must only go through the `zectrix_epd_power_on/off` FFI (`display.rs:92-136`).

## CONVENTIONS (crate-level)
- Error handling uniformly uses `anyhow::Result`; all peripheral initialization goes into `Note4Board::take()`, modules must not call `Peripherals::steal()` themselves.
- `main.rs` mod declarations are in alphabetical order.
- Glyph data tables use `#[rustfmt::skip]` (`font8x16.rs`).
- Build injection: `build.rs` emits `BUILD_EPOCH_SECS` (RTC fallback); rustflags include `--cfg espidf_time64`.

## ANTI-PATTERNS (code-enforced)
Where the root AGENTS.md red lines are enforced in this crate:
- **Wi-Fi connections** (`wifi.rs`): ① **`connect()` may be called multiple times per boot** — it was long believed that only one connection was allowed per boot (a second connection crashed with `Guru Meditation`/`Unhandled debug exception`), until the culprit was found to be the blocking `esp_wifi_scan_start()` run before every connection (credentials come from desktop `SetWifi`, the scan is pure redundancy) and removed; on-device tests confirmed 3 consecutive syncs within the same boot all succeed. The raw FFI `esp_wifi_connect()`/`disconnect()` are kept (the esp-idf-svc wrapper's status tracking still panics with #503 on reuse). ② **Never call `esp_wifi_stop()`** (stop→start is the crash trigger point; `start()` runs once per process). ③ To restart, only use the deep-sleep path (`power::restart_via_deep_sleep`, pure timer wake-up, no ext1/RTC alarm), **not `esp_restart()`**. `restart_for_fresh_wifi_session` is now dead code (`#[allow(dead_code)]` escape hatch). Wi-Fi config must use `WIFI_STORAGE_RAM`. There is no longer an on-device Wi-Fi provisioning wizard; `SetWifi` can only come over USB/BLE.
- **Automatic periodic sync** (`main.rs` `maybe_auto_sync`): the main loop checks every ~30s and calls `sync::sync_now` when more than `sync_interval_minutes` (NVS default 60, SYNC INTERVAL in the settings menu offers 1/5/10/30/60) have passed since the last successful sync. The check only runs while the Home screen is idle (menu screens block the main loop); failures are logged and retried next time. Manual and automatic sync share the same code path.
- **BLE** (`ble_control.rs`): the NimBLE callback thread **must not touch `Note4Board`**, it only pushes to a channel; dispatch happens only in the main loop (L80-83); after `deinit_full()`, every `start()` must explicitly call `BLEDevice::init()` (L55-58). BLE and Wi-Fi share the radio and are never on at the same time — BLE only lives on the pairing screen.
- **RTC alarm**: there is only one hardware slot, so with multiple alarms the soonest one must always be written (`alarms::next_due`, `rtc.rs:165-170`); at boot, `clear_alarm()` clears any residue (`board.rs:92-94`).
- **Power-on sequencing**: GPIO42 (AVDD) must be pulled high before the I2C0 init (`board.rs:69-70, 87`); after wake-up, call `release_power_latch_hold()` first, then create the PinDriver on GPIO17 (`power.rs:33-36` → `board.rs:61`).
- **EPD**: after a 4bpp full refresh, a 1bpp full refresh must be performed once before partial refresh can resume; before a partial refresh, confirm power is managed via the FFI.
- **Alarm/todo `id`s must not collide within a list** (sync-api contract); USB commands are polled on Home and long-lived content pages, while short modal pickers may briefly delay them. BLE commands are drained on the pairing screen.
- **Remote-triggered state changes must trigger a home-screen redraw** (`main.rs:294-328`): `CLOCK_RECT` (y=36..128) must not cover the NEXT ALARM / OPEN TODOS panels (y=151..284, `home.rs`'s `CARD_TOP`/`CARD_H`). After `control::dispatch` rewrites NVS, the NVS-render pipeline does not dirty-mark automatically — any USB/BLE command that changes data shown on the home screen (`SyncNow`, `clear_alarms`, and future commands of the same kind) must `dirty.push(FULL_SCREEN_RECT)` in the `main` poll. The command variant is captured with `matches!` before dispatch (`Command` is not `Copy`; dispatch moves it), and the rect is only pushed when the reply is `Reply::Ok`; a failed reply must not waste a full refresh.

## COMMANDS
```bash
./scripts/build-rust.sh --release   # run from the repo root; a bare cargo build inside the crate requires an already-sourced ESP-IDF environment
cargo +esp fmt -- --check           # inside the crate directory
cargo clippy                        # requires sourcing the ESP-IDF environment first; currently zero warnings (browse_page has #[allow(clippy::too_many_arguments)])
cd tools/preview && cargo run --release   # PC preview: renders the home screen and sub-screen mockups to PNG without flashing (uses the real home.rs/canvas/font/icons via #[path])
```

## NOTES
- `IDF_PATH`/`LIBCLANG_PATH` in `.cargo/config.toml` are machine-specific; see the root AGENTS.md NOTES.
- rust-analyzer availability depends on the repo-root `.vscode/settings.json` + the `esp-ra` toolchain (root cause and rebuild procedure in the root AGENTS.md NOTES); after changing code you can see diagnostics directly in the editor.
- When changing `sdkconfig.defaults`, cross-check the root red line #2 (DIO) and the partitions.csv 6-level relative-path convention.
- No tests to run; verification = flash + monitor (root AGENTS.md COMMANDS). The home-screen layout (and any shared `ui::header`/row chrome) can be iterated in `tools/preview` first, then flashed.
