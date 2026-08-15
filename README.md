# Inkpaper NOTE4 Firmware

自研固件起点，目标硬件：**ZECTRIX NOTE4 黑白屏版**（ESP32-S3-WROOM-1 N16R8，4.2 寸 400×300 SSD2683 EPD）。

> 本仓库**只面向 NOTE4 黑白屏**。NOTE4 与 NOTE4C 的屏幕硬件和固件不同，**不可混刷**。

## 当前状态

`rust-firmware/` 是当前可在实机运行的 Rust 主固件：

- GPIO17 主电源软锁存，启动后立即拉高，按下电源键后不会断电。
- GPIO3 绿色 LED 心跳（~0.5 s 翻转）。
- GPIO0/39/18 三按键（ENTER/UP/DOWN），20 ms 采样、4 次确认消抖，支持短按和 1 s 长按事件。
- 官方 ZECTRIX EPD 组件（C++）通过 `package.metadata.esp-idf-sys.extra_components` 接入，FFI 模块 `zectrix_epd` 已生成。
- 启动后全刷显示 `Hello world` + 三个按键计数；按键短按触发**官方局刷 API** 仅刷新数字区域；长按任意键触发一次全刷清残影。
- 16 MB Flash + ~12 MB 预留存储分区已固化在 `rust-firmware/partitions.csv`。
- 完整的 16 MiB 原厂 Flash 已备份（`backups/note4-factory-20260815-213553.bin`，SHA-256 `dbe8b1…d182a`）。

### 尚未完成

Wi-Fi 配网、ES8311 音频、RTC（PCF8563）、NFC（GT23SC6699）、电池 ADC 与充电管理、文件系统、休眠和 OTA；内容协议未定（自研 HTTP / Slate 兼容 / 完全离线）。

完整的环境、安全事项、构建、烧录、调试与故障排查见 **[docs/development-guide.md](docs/development-guide.md)**。

## 仓库结构

```
inkpaper/
├── docs/
│   ├── development-guide.md  完整开发指南（必读）
│   └── note4-hardware.md     板级 GPIO / 电源轨 / EPD 格式
├── rust-firmware/
│   ├── .cargo/config.toml           输出固定到 D:\espbuild
│   ├── Cargo.toml                   依赖 + extra_components (EPD FFI)
│   ├── Cargo.lock
│   ├── build.rs                     embuild::espidf::sysenv::output
│   ├── partitions.csv               nvs / phy_init / factory / storage
│   ├── rust-toolchain.toml          channel = "esp"
│   ├── sdkconfig.defaults           DIO / 80 MHz / OCT PSRAM / USB Serial/JTAG
│   ├── src/
│   │   ├── main.rs                  入口 + 按键事件 + 局刷/全刷循环
│   │   ├── board.rs                 电源锁存 / LED / 按键 / 充电 GPIO
│   │   ├── button.rs                消抖 + 短按/1s 长按
│   │   └── display.rs               1bpp 画布 + EPD Rust 封装 + 5x7 字模
│   └── components/zectrix_epd/
│       ├── CMakeLists.txt
│       ├── zectrix_epd.cc
│       ├── include/zectrix_epd.h
│       └── private_include/ssd2683_waveform.h
├── scripts/
│   ├── build-rust.ps1               激活 ESP-IDF 5.5.5 并构建
│   └── backup-flash.ps1             完整 16 MiB Flash 备份
├── vendor/
│   ├── README.md                    esp-idf-hal 0.46.2 patch 说明
│   └── esp-idf-hal/                 本地 patch 仓库（含 sdmmc 字段）
└── backups/
    └── note4-factory-20260815-213553.bin     (gitignored)
```

## 开发环境

已验证组合：

| 组件 | 版本 / 路径 |
| --- | --- |
| 操作系统 | Windows 11 x86-64 |
| ESP-IDF | 5.5.5（默认 `C:\Espressif\frameworks\esp-idf-v5.5.5-2`）|
| Python | 3.11（由 ESP-IDF 管理） |
| Rust 工具链 | 频道 `esp`（Espressif Xtensa，rust-toolchain.toml） |
| Xtensa GCC | `xtensa-esp-elf` 14.2.0（含 `xtensa-esp32s3-elf-nm/readelf/objdump`） |
| 烧录工具 | `cargo-espflash`（在 `~/.cargo/bin`） |
| 构建工具 | Visual Studio C++ Build Tools + Windows 11 SDK |

如系统上的 ESP-IDF 路径或版本不同，需要同步修改 `scripts/build-rust.ps1` 和 `rust-firmware/.cargo/config.toml`。

## 快速开始

第一次刷自己的固件前，先完整备份原厂 16 MiB Flash：

```powershell
.\scripts\backup-flash.ps1 -Port COMx
Get-FileHash .\backups\*.bin -Algorithm SHA256
```

构建并烧录：

```powershell
# 构建 release 镜像
.\scripts\build-rust.ps1 -Release

# 烧录（已映射到生成路径）
espflash flash `
  --port COMx `
  --chip esp32s3 `
  --flash-size 16mb `
  --flash-mode dio `
  --flash-freq 80mhz `
  --partition-table .\rust-firmware\partitions.csv `
  D:\espbuild\xtensa-esp32s3-espidf\release\inkpaper-note4

# 查看日志
espflash monitor --port COMx
```

成功标志：电源保持按通、LED 闪烁、串口日志输出按键事件、EPD 完成全刷后显示 `Hello world` + 三个按键计数。

### 故障信号

- **烧录成功但反复复位** → 确认 `sdkconfig.defaults` 仍为 `CONFIG_ESPTOOLPY_FLASHMODE_DIO=y`（NOTE4 实机不支持 QIO）。
- **日志说刷新完成但屏幕完全不动** → 确认有编译进静态库的 `libzectrix_epd.a`；`vendor/esp-idf-hal/` 切到正确版本后重跑 `cargo clean -p esp-idf-sys`。
- **上电后立刻关机** → 确认 `GPIO17` 在启动早期被拉高（GPIO 锁存）。
- **无法识别 `espflash`/`esptool.py`** → ESP-IDF 环境未激活。先在 PowerShell 中 `. "C:\Espressif\frameworks\esp-idf-v5.5.5-2\export.ps1"`。

## 版本

| 组件 | 版本 |
| --- | --- |
| `inkpaper-note4` (crate) | `0.1.0` |
| `esp-idf-sys` | 0.37.2 |
| `esp-idf-svc` | 0.52.1 |
| `esp-idf-hal` | 0.46.2（vendor + sdmmc patch）|
| `embuild` (build) | 0.33.3 |

## 开发路线（参考 `docs/development-guide.md#12`）

1. 保留当前示例作为硬件冒烟基线。
2. 加入统一画布、字体与布局层（当前 `display.rs` 是 5×7 像素字模硬编码 + 逐像素绘图）。
3. NVS 设置、Wi-Fi 配网、时间同步。
4. 电池 ADC、充电状态、PCF8563 RTC 与低功耗策略（注意 GPIO17 在深睡时需要 RTC GPIO hold）。
5. 音频（I2S + ES8311）、NFC（GT23SC6699）。
6. 内容协议、内容缓存、文件系统。
7. OTA、回滚、看门狗与故障恢复。

Slate 是一个参考实现，但本仓库会保持为你自己的 NOTE4 固件起点。
