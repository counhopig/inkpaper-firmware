# ZECTRIX NOTE4 Firmware Development Guide

This document records the hardware information, development environment, build and flashing workflow, e-paper display driver approach, and commonly repeated pitfalls required for taking over this repository.

> This repository has only been verified on the ZECTRIX NOTE4 **black-and-white display version**. The display hardware, firmware, and waveforms of the NOTE4C and NOTE4 are incompatible — **never cross-flash them**.

## 1. Project Scope and Current Status

Target device: **ZECTRIX NOTE4 black-and-white display version** (i.e., the hardware corresponding to `itopinion/zectrix-note4-epd-demo`).

### Verified on Real Hardware (These Behaviors Must Be Preserved)

- ESP32-S3 revision v0.2, connected via USB Serial/JTAG.
- 16 MB Flash; the boot image must use **DIO** mode.
- After cold boot, the Rust firmware keeps the whole device powered (GPIO17 soft latch) and blinks the green LED heartbeat.
- The three keys ENTER / UP / DOWN (short press + 1 s long press) have been verified on real hardware.
- Button debouncing: 20 ms sampling, 4-sample confirmation (`rust-firmware/src/button.rs`).
- The official SSD2683 EPD driver can perform 400×300 black-and-white full refresh and partial refresh; only refreshing the numeric region was verified on real hardware; long-press ghost clearing has also been verified.
- The factory 16 MiB Flash has been fully backed up to `backups/note4-factory-20260815-213553.bin` (SHA-256 `dbe8b1504710d6b76dee0136505bc952013023db29fdcd1a3d3bfb4c6d9d182a`), which can be used to restore this device.

### Current Implementation Status

The firmware is now a usable calendar/alarm/todo device, no longer the button-counter demo from when this section was first written: Wi-Fi STA+NTP, audio (ES8311), RTC (PCF8563, including hardware alarm registers), NFC (GT23SC6699), battery management and ADC, deep sleep (GPIO17 RTC hold), USB/BLE control protocol, and HTTPS two-way sync are all implemented, with their basic paths verified on real hardware. See `rust-firmware/AGENTS.md` for the current module responsibility table (more up to date and complete than the early architecture diagram in section 4).

Not yet done: file system, OTA, rollback. Not yet fully manually verified on real hardware: the full alarm ringing → ENTER dismiss flow, BLE pairing end-to-end connectivity — see the "Not Yet Done / Not Yet Verified" section of the root README for details.

## 2. Safety Matters (Non-Negotiable)

1. **Confirm the device is the NOTE4 black-and-white display version.** The NOTE4C's display hardware and firmware differ; do not cross-flash.
2. **Before the first partition modification or flash, read the full 16 MiB Flash and save its SHA-256.** Copies of the backup should be stored in at least one physically isolated location.
3. **The factory backup contains device-unique data, credentials, and calibration information** and must not be committed publicly. `backups/*.bin` is already excluded by `.gitignore`.
4. **The DIO boot mode must be used.** Both the NOTE4 factory boot logs and this project's verification results show `mode:DIO`; a QIO image repeatedly triggers watchdog resets before the application starts.
5. **The e-paper display must use waveforms and power sequencing matching the panel.** Do not replace the official driver with a generic SSD1683 example and then repeatedly refresh.
6. **GPIO17 (PWR_ON) must be pulled high early in boot**, otherwise releasing the power key powers off the whole device; an RTC GPIO hold must also be designed for deep sleep.
7. Close the monitor, serial terminals, and other IDEs occupying the serial port before flashing.

## 3. Verified Hardware

See [note4-hardware.md](note4-hardware.md) for the complete GPIO table. The current example directly uses the following signals:

| GPIO | Purpose | Notes |
| --- | --- | --- |
| 0 | ENTER / BOOT | Active-low press, RTC-capable wake |
| 3 | Green LED | Lit when low |
| 6 | EPD_PWR_EN | Managed by the official driver |
| 8 | EPD_BUSY | Active-low busy |
| 9 | EPD_NRES | Managed by the official driver |
| 10 | EPD_NDC | Managed by the official driver |
| 11 | EPD_NCS | SPI3 |
| 12 | EPD_SCK | SPI3 |
| 13 | EPD_SDA / MOSI | SPI3 |
| 17 | PWR_ON main power latch | Pull high early in boot |
| 18 | DOWN / KEY_DET | Active-low press |
| 39 | UP | Active-low press, not RTC wake-capable |
| 42 | PA_PWR_EN (AVDD) | Kept off in the current example |

GPIO 26-37 are occupied by Octal PSRAM and cannot be used as normal GPIOs.

## 4. Software Architecture

The application core is written in Rust, built on top of ESP-IDF (5.5.5):

```text
Rust application (main.rs, 20 flat mod modules — see rust-firmware/AGENTS.md)
  |
  +-- Board ownership, buttons, RTC, audio, NFC, ADC (board.rs / esp-idf-hal)
  |
  +-- 1bpp framebuffer and proportional glyph renderer (canvas.rs / font8x16.rs / display.rs)
         |
         +-- generated C bindings (esp-idf-sys bindgen)
                 |
                 +-- official zectrix_epd C++ component (vendor)
                         |
                         +-- ESP-IDF GPIO/SPI drivers + SSD2683 waveform
```

See the MODULE MAP in [`rust-firmware/AGENTS.md`](../rust-firmware/AGENTS.md) for the complete module responsibility table (21 src files, including application-layer modules such as UI/alarm/todo/sync/USB/BLE) — the rest of this section only keeps the build details of the EPD FFI component itself and does not duplicate the full module list.

| File | Role |
| --- | --- |
| `rust-firmware/components/zectrix_epd/` | Official NOTE4 EPD C++ driver + SSD2683 waveform table |
| `rust-firmware/Cargo.toml` | Rust dependencies + esp-idf-sys extra_components configuration (generates the `zectrix_epd` FFI) |
| `rust-firmware/sdkconfig.defaults` | ESP32-S3, DIO, PSRAM, serial port, etc. configuration |
| `rust-firmware/partitions.csv` | 16 MB Flash partition table |
| `scripts/build-rust.sh` | Sources the local ESP-IDF environment and builds the Rust firmware (Linux) |

### Official EPD Driver Source

- Official repository: <https://github.com/itopinion/zectrix-note4-epd-demo>
- This project uses its `components/zectrix_epd`.

Do not delete `rust-firmware/components/zectrix_epd/private_include/ssd2683_waveform.h`. It contains the complete waveform data and is the key fix for the "the program reports a successful refresh but the screen still shows the old image" issue. When updating the upstream component, preserve its directory structure and rebuild completely.

In `Cargo.toml`:

```toml
[[package.metadata.esp-idf-sys.extra_components]]
component_dirs = ["components/zectrix_epd"]
bindings_header = "components/zectrix_epd/include/zectrix_epd.h"
bindings_module = "zectrix_epd"
```

This section accomplishes two things at once: it makes CMake compile the official component and generates a standalone Rust FFI module from `zectrix_epd.h` (9 `zectrix_epd_*` symbols have been observed in `out/bindings.rs` in this repository's release artifacts).

After building, the ELF exports the following C ABI symbols (verified with `xtensa-esp32s3-elf-nm`):

```
T zectrix_epd_del
T zectrix_epd_get_default_config
T zectrix_epd_new
T zectrix_epd_power_off
T zectrix_epd_power_on
T zectrix_epd_refresh_full_1bpp
T zectrix_epd_refresh_partial_1bpp
```

## 5. Arch Linux Development Environment

Verified combination:

| Item | Version |
| --- | --- |
| Operating system | Arch Linux x86-64 |
| ESP-IDF | 5.5.5 (git clone `v5.5.5` to `~/esp/esp-idf`, `alias get_idf='. $HOME/esp/esp-idf/export.sh'`) |
| Python | Managed by ESP-IDF (`~/.espressif/python_env/idf5.5_py3.14_env`) |
| Rust stable | Installed via pacman (`rustup`) |
| Rust Xtensa | Toolchain `esp` (installed via `espup install` to `~/.rustup/toolchains/esp`, includes rust-src) |
| `espflash` / `cargo-espflash` | 4.5.0 (the `espflash` package in the extra repository) |
| USB serial port | USB Serial/JTAG → `/dev/ttyACM0`; the user must be a member of the `uucp` group |

Verify the environment:

```bash
get_idf && idf.py --version
rustup toolchain list
rustc +esp --version
espflash --version
```

You should see ESP-IDF 5.5.x, a toolchain named `esp`, and a runnable `espflash`. If anything is missing:

```bash
sudo pacman -S espflash espup rustup
espup install     # installs the Xtensa Rust toolchain + xtensa-esp-elf GCC + clang
```

If the local ESP-IDF path or toolchain version differs, modify `scripts/build-rust.sh` and `rust-firmware/.cargo/config.toml` (`IDF_PATH`, `LIBCLANG_PATH`).

> **espup troubleshooting**: espup downloads the Xtensa toolchain from GitHub, while RISC-V targets go through `rustup`. If installation fails, first manually add the RISC-V target: `rustup target add riscv32imc-unknown-none-elf`, then manually download the Xtensa toolchain (`rust-<ver>-x86_64-unknown-linux-gnu.tar.xz` from the `esp-rs/rust-build` releases), extract it, and install it with the included `install.sh --prefix=~/.rustup/toolchains/esp`; also add `rust-src-<ver>.tar.xz` (extract with `--strip-components=2` into the same toolchain directory for build-std).

## 6. First Connection and Full Backup

Connect with a USB-C cable that supports data transfer, then:

```bash
espflash board-info --port /dev/ttyACM0
```

Full backup:

```bash
espflash save-image \
  --port /dev/ttyACM0 --chip esp32s3 \
  --flash-size 16mb --flash-mode dio --flash-freq 80mhz \
  backups/note4-factory-$(date +%Y%m%d-%H%M%S).bin
sha256sum backups/*.bin > backups/SHA256SUMS
```

The expected size of `backups\note4-factory-YYYYMMDD-HHMMSS.bin` is `16777216` bytes. The backup and SHA-256 should be copied to at least one physically isolated location.

## 7. Building the Rust Firmware

From a plain shell, run in the repository root:

```bash
./scripts/build-rust.sh --release
```

The script sources the ESP-IDF environment and then runs `cargo build --release` in `rust-firmware`. The artifact is at `rust-firmware/target/xtensa-esp32s3-espidf/release/inkwash-note4` (Linux has no Windows path length limit, so a fixed output directory is no longer needed).

After modifying the extra component configuration in `Cargo.toml`, if the new FFI module does not appear, clean `esp-idf-sys` and rebuild:

```bash
cd rust-firmware
cargo clean -p esp-idf-sys
cd ..
./scripts/build-rust.sh --release
```

## 8. Flashing and Serial Monitoring

Run from the repository root:

```bash
espflash flash \
  --port /dev/ttyACM0 \
  --chip esp32s3 \
  --flash-size 16mb \
  --flash-mode dio \
  --flash-freq 80mhz \
  --partition-table rust-firmware/partitions.csv \
  rust-firmware/target/xtensa-esp32s3-espidf/release/inkwash-note4

espflash monitor --port /dev/ttyACM0
```

Exit the monitor with `Ctrl+C`. If flashing fails, exit the monitor first, then rerun the flash command.

### Success Criteria

1. After boot, the device stays powered when the power key is released.
2. The green LED blinks periodically (the `led_tick` outside `button.rs` toggles every ~0.5 s in the main loop).
3. The screen displays `Hello world` after an obvious full-refresh process.
4. The screen shows the three counters for ENTER / UP / DOWN.
5. Each press of the corresponding key prints the key name to the serial log and increments the on-screen counter once (partial refresh).

In the current example, each key press refreshes only the changed numeric region using the official partial-refresh API; full refresh is only performed at boot and for long-press ghost clearing.

## 9. Display Data Conventions

| Item | Value |
| --- | --- |
| Resolution | 400 × 300 |
| 1bpp frame size | `400 * 300 / 8 = 15000` bytes |
| Row-major | MSB-first |
| Colors | `1` = white, `0` = black |
| BUSY | Active-low busy |
| Controller | SSD2683 |
| Official APIs | Full refresh, partial refresh, 16-level grayscale full refresh |
| After 4bpp full refresh | Partial refresh cannot be used again until another 1bpp full refresh completes |

Typical full-refresh lifecycle:

```text
zectrix_epd_power_on
zectrix_epd_refresh_full_1bpp
zectrix_epd_power_off
```

An e-paper display retains its last image after power-off; this is normal. Seeing the "old image" does not prove that the new firmware is not running — combine the serial log and the actual refresh flicker to judge.

## 10. Current Implementation Highlights (Source Index)

> This section only collects **low-level implementation details** that do not fit into the module table of `rust-firmware/AGENTS.md`
> (ADC read strategy, I2C power-up sequencing, RTC register layout, etc.); application-layer behavior (menu structure, command
> protocol, sync flow) is governed by the root README and `rust-firmware/AGENTS.md` and is not duplicated here.

- The `main.rs` main loop polls the three keys at `POLL_INTERVAL_MS = 20` (ms); key events are carried via `Option<ButtonEvent>`. A short press does nothing on Home; a long press of UP/DOWN opens the navigation drawer (`screens::open_navigation`); when a redraw is needed, the corresponding `Rect` is collected into the `dirty` list, and one partial/full refresh is issued after the round ends.
- `board.rs::take()` centralizes initialization: the power latch is pulled high, AVDD is pulled low (off; pulled high again before I2C0 init), keys use `Pull::Up`, `charging` uses `Pull::Up`, and `charge_done` is a floating input. The status LED (`GPIO3`, active-low, officially named `ZECTRIX_POWER_LED`) is driven by `update_charging_led` as an external-power indicator: on when the charger is plugged in, off when unplugged.

### Storage and Power State

- `storage.rs` wraps the default NVS partition (a `nvs` 24 KiB partition is declared in `partitions.csv`) under the `inkwash` namespace. `PersistedCounters` (historically evolved from button-counter persistence; the name stuck) now stores six keys: `wifi_ssid` / `wifi_pass` / `server_url` / `auth_token` / `sync_etag` / `timezone_min`; `open()` calls `EspDefaultNvsPartition::take()` to initialize the NVS flash automatically. The `AlarmStore`/`TodoStore` in `alarms.rs`/`todos.rs` each hold additional namespaces of the same NVS partition and store whole JSON blobs rather than per-field keys.
- `board.rs::battery_millivolts()` uses the ESP-IDF 5.x ADC oneshot API: `AdcDriver::new(peripherals.adc1)` holds the ADC unit persistently; each voltage read takes `GPIO4` (ADC1 CH3 on the ESP32-S3) via `Peripherals::steal()` and temporarily constructs `AdcChannelDriver::new(&self.adc, gpio4, &BATTERY_ADC_CHANNEL_CONFIG)`. Because every field of `Note4Board` is `'static`, putting the channel directly into the board would trigger a borrow conflict, so the strategy is to "rebuild the channel on each read"; the return value is ESP-IDF mV × 2 (onboard 1:2 divider) and is clamped to `u16::MAX`. `BATTERY_ADC_CHANNEL_CONFIG` enables `Calibration::Curve` (eFuse three-point fitting, matching the official demo's `adc_cali_curve_fitting`) and averages `BATTERY_ADC_SAMPLES = 10` readings to suppress jitter.
- The `main.rs` main loop calls `report_power_state()` every `STATUS_REPORT_INTERVAL_POLLS` (50 polls ≈ 1 s), printing `Power state: power_present=… charging=… full=… vbat_mV=… (..%)`; it re-reads the PCF8563 every `CLOCK_POLL_INTERVAL_POLLS` (60 polls ≈ 1.2 s) and marks `CLOCK_RECT` as dirty to trigger a partial refresh only when the second/minute/hour change, avoiding a redraw on every poll.
- Charging state is determined by the `ChargeStatus` state machine in `board.rs` (ported from the official demo's `charge_status.cc`, a simplified debounced version); each tick ≈ 1 s, and stabilization requires 2 consecutive ticks: `power_present` (any status line active), `charging` (`CHRG_L = GPIO2` low and not full), `full` (`STDBY_H = GPIO1` high). `report_power_state` calls `charging_state()` to advance the state machine; the rendering path only reads `charge_snapshot()` so different call frequencies do not break debouncing. Confirmed on real hardware: when fully charged and plugged in, the state is `power_present=true charging=false full=true` (the charging IC has stopped charging; `charging=false` is the real state, not a bug).
- `sdkconfig.defaults` explicitly enables `CONFIG_NVS_ENABLED=y` and `CONFIG_ADC_ONESHOT_ENABLED=y`; ADC eFuse curve calibration is enabled via `Calibration::Curve` and needs no extra config.
- The battery percentage `battery_percent_from_mv` uses the official demo's quadratic polynomial `(-mv² + 9016·mv - 19189000)/10000` (0% ≈ 3444 mV, 100% ≈ 4200 mV), which fits the LiPo discharge curve better than the old 3300–4200 linear mapping (the 4.0 V plateau no longer reads artificially high).

### PCF8563 RTC and I2C0

- Bus: `I2C0`, `SDA = GPIO47`, `SCL = GPIO48`, 400 kHz master mode. `board.rs` calls `I2cDriver::new` only after `_avdd_power` (`GPIO42`, used by the factory as the power source for audio + I2C pull-ups) is initialized high; otherwise the floating SDA/SCL lines NACK.
- Device address: `0x51` (7-bit); `rtc::Pcf8563::probe()` performs a one-byte read of 0x00 as a connectivity test during board initialization; failure makes `Note4Board::take()` return an error.
- Register layout: BCD; second/minute/hour/day/weekday/month/year are at `0x02..=0x08`; bit7 of `0x00` is `voltage_low` — VL=1 means the RTC backup battery lost power or this is the first power-on, so the time can no longer be trusted.
- `main.rs` boot sequence: `read_time()` → print; when `voltage_low`, rewrite via `from_unix(BUILD_EPOCH_SECS)` (`BUILD_EPOCH_SECS` is injected into `cargo:rustc-env` by `build.rs` using `SystemTime::now()`, refreshed at build time).
- Alarms and square wave: after startup, `Pcf8563::clear_alarm()` in `board.rs` clears the alarm registers `0x09..=0x0C` and the AIE bit in `0x01` so a leftover alarm cannot interrupt subsequent deep-sleep wake-ups; `alarms::program_hardware_alarm()` then immediately rewrites the nearest stored alarm into the same register group (the chip has only one hardware alarm slot; in multi-alarm scenarios the firmware itself picks the nearest one).

## 11. Common Failures

### Firmware flashes successfully but keeps resetting

`sdkconfig.defaults` must contain:

```text
CONFIG_ESPTOOLPY_FLASHMODE_DIO=y
```

Do not change it to QIO.

### The log says refresh completed, but the screen does not move at all

- Confirm that the official `zectrix_epd` component from this repository is used, not a simplified SSD1683 command sequence.
- Confirm that the waveform header exists and is compiled into `libzectrix_epd.a` (the build log should show `__idf_zectrix_epd.dir/zectrix_epd.cc.obj`).
- Confirm that the `GPIOGPIO6` power is managed by the driver (`zectrix_epd_power_on/off`).

### `cargo` cannot find the `zectrix_epd` module

Usually the `esp-idf-sys` cache predates the extra component configuration:

```bash
cd rust-firmware
cargo clean -p esp-idf-sys
cd ..
./scripts/build-rust.sh --release
```

### `.cargo-lock` or the target directory is locked

Close any running Cargo, rustc, IDE check tasks, or stale build processes, then retry.

### Wi-Fi connection fails (`reason=201` / `NO_AP_FOUND`), but a manual scan sees the target AP

`EspWifi::connect()` internally triggers a directed scan with SSID filtering that is case-sensitive byte by byte; a manual `scan()`
has no filter and returns all APs. When the two results differ, first confirm that the SSID stored in NVS matches the router's
actual broadcast exactly in case (`gen-nvs-wifi.py` provisioning SSIDs typed by hand often differ in case from what the AP
broadcasts, e.g., `XiaoMi_ED4E` vs the router's actual `Xiaomi_ED4E`) — the SSID printed by scan is the authoritative source.

### Cannot open the serial port

Confirm the user is a member of the `uucp` group (run `sudo usermod -aG uucp $USER` and log in again), close other monitors / serial tools, re-plug the USB cable, and check the device node with `ls /dev/ttyACM*`. If necessary, hold ENTER/BOOT and trigger a reset to enter download mode.

### Powers off shortly after power-on

`GPIO17` is the main power soft latch. It must be configured as a high output early in boot. An RTC GPIO hold must also be designed before entering deep sleep, otherwise the device may truly power off.

### The screen does not clear immediately

This is normal. An e-paper display holds its image when powered off; only a valid refresh waveform changes the content.

## 12. Development Roadmap: Done vs Remaining

Hardware smoke-test baseline, NVS, Wi-Fi/NTP, battery ADC/charging state, PCF8563 RTC, low power, audio (ES8311), NFC
(GT23SC6699), watchdog, unified canvas/font layer, calendar/alarm/todo apps, USB/BLE control protocol, HTTPS two-way sync —
all implemented; see `rust-firmware/AGENTS.md` for module responsibilities.

Remaining:

1. File system, content caching strategy.
2. OTA, rollback.
3. Real-device verification: the full alarm ringing → dismiss flow, BLE pairing end-to-end (especially since it must be redone
   after switching to the Tauri/Vue version of `inkwash-desktop` — the old verification was done under an earlier Desktop
   implementation), and the actual Wi-Fi reconnect-without-restart experience.
   See the "Not Yet Done / Not Yet Verified" section of the root README and `docs/project-status.md` for details.

For each new peripheral, run standalone tests first, then integrate it into the main application. Display, power, and sleep changes carry the highest risk; always keep a recoverable serial path and the factory backup.

## 13. Pre-Commit Checks

```bash
cargo +esp fmt --manifest-path rust-firmware/Cargo.toml -- --check
./scripts/build-rust.sh --release
```

Check on real hardware at least once: cold boot, power hold, initial full refresh, one press of each of the three keys, USB reconnection, and the serial log.

Do not commit `sdkconfig`, build directories, factory backups, or logs containing device credentials. The current `.gitignore` already excludes `build/`, `managed_components/`, `dependencies.lock`, `sdkconfig`, `sdkconfig.old`, `backups/*.bin`, `rust-firmware/target/`, and `rust-firmware/.embuild/`.

## 14. Restoring Factory Firmware

Only use the full backup **of that specific device**:

```bash
esptool.py -p /dev/ttyACM0 -b 921600 write_flash 0x0 backups/note4-factory-20260815-213553.bin
```

After restoring, re-read the Flash or compute the backup file hash to confirm the correct image was used. Never write another device's or a NOTE4C's backup into this device.

## 15. References

- ZECTRIX open-source resources: <https://www.zectrix.com/open-source.html>
- NOTE4 hardware specifications: <https://wiki.zectrix.com/zh/hardware/note/spec>
- Firmware resources: <https://wiki.zectrix.com/zh/software/firmware>
- Community open-source firmware: <https://wiki.zectrix.com/zh/software/Community-OpenSource-Firmware>
- Official NOTE4 EPD Demo: <https://github.com/itopinion/zectrix-note4-epd-demo>
- Slate reference firmware: <https://github.com/qiujun8023/slate>
- Rust on ESP Book: <https://docs.esp-rs.org/book/>
- espflash: <https://github.com/esp-rs/espflash>
