# ZECTRIX NOTE4 Firmware Development Guide

本文档记录接手本仓库所需的硬件信息、开发环境、构建与烧录流程、墨水屏驱动方案、以及容易重复踩到的问题。

> 仓库只在 ZECTRIX NOTE4 **黑白屏版**上验证过。NOTE4C 与 NOTE4 的显示硬件、固件、波形都不兼容，**不可混刷**。

## 1. 项目边界与当前状态

目标设备：**ZECTRIX NOTE4 黑白屏版**（即 `itopinion/zectrix-note4-epd-demo` 对应硬件）。

### 实机已验证（必须保留这些行为）

- ESP32-S3 revision v0.2，通过 USB Serial/JTAG 连接。
- 16 MB Flash，启动镜像必须使用 **DIO** 模式。
- Rust 固件冷启动后保持整机供电（GPIO17 软锁存）、绿色 LED 心跳。
- ENTER / UP / DOWN 三按键（短按 + 1 s 长按）已实机验证。
- 按键消抖：20 ms 采样、4 次确认（`rust-firmware/src/button.rs`）。
- 官方 SSD2683 EPD 驱动可完成 400×300 黑白全刷与局刷，仅刷新数字区域在实机验证；长按清残影也已验证。
- 原厂 16 MiB Flash 已完整备份到 `backups/note4-factory-20260815-213553.bin`（SHA-256 `dbe8b1504710d6b76dee0136505bc952013023db29fdcd1a3d3bfb4c6d9d182a`），可用于恢复本机。

### 当前固件是开发起点，不是完整产品

未完成模块：Wi-Fi、音频（ES8311）、RTC（PCF8563）、NFC（GT23SC6699）、电池管理与 ADC、文件系统、休眠（deep sleep + RTC GPIO hold）、OTA。

## 2. 安全事项（不可妥协）

1. **确认设备是 NOTE4 黑白屏版。** NOTE4C 的显示硬件和固件不同，不能混刷。
2. **第一次修改分区或烧录前，先读取完整 16 MiB Flash 并保存 SHA-256。** 备份应复制到至少一处物理隔离位置。
3. **原厂备份包含设备唯一数据、凭据与校准信息**，不应公开提交。`backups/*.bin` 已被 `.gitignore` 排除。
4. **必须使用 DIO 启动模式。** NOTE4 实机的工厂启动日志和本项目验证结果都是 `mode:DIO`；QIO 镜像会在应用启动前反复触发看门狗复位。
5. **墨水屏必须使用匹配面板的波形和电源时序。** 不要用通用 SSD1683 示例替换官方驱动后直接反复刷新。
6. **GPIO17（PWR_ON）在启动早期必须拉高**，否则松开电源键会导致整机断电；进入 deep sleep 时也要设计 RTC GPIO hold。
7. 烧录前关闭占用串口的 monitor、串口终端和其他 IDE。

## 3. 已验证的硬件

完整 GPIO 表见 [note4-hardware.md](note4-hardware.md)。当前示例直接使用以下信号：

| GPIO | 用途 | 备注 |
| --- | --- | --- |
| 0 | ENTER / BOOT | 低电平按下，RTC-capable wake |
| 3 | 绿色 LED | 低电平点亮 |
| 6 | EPD_PWR_EN | 由官方驱动管理 |
| 8 | EPD_BUSY | 低电平忙 |
| 9 | EPD_NRES | 由官方驱动管理 |
| 10 | EPD_NDC | 由官方驱动管理 |
| 11 | EPD_NCS | SPI3 |
| 12 | EPD_SCK | SPI3 |
| 13 | EPD_SDA / MOSI | SPI3 |
| 17 | PWR_ON 主电源锁存 | 启动早期拉高 |
| 18 | DOWN / KEY_DET | 低电平按下 |
| 39 | UP | 低电平按下，非 RTC 唤醒 |
| 42 | PA_PWR_EN（AVDD）| 当前示例保持关闭 |

GPIO 26-37 被 Octal PSRAM 占用，不能作为普通 GPIO 使用。

## 4. 软件架构

应用主体使用 Rust，底层建立在 ESP-IDF（5.5.5）上：

```text
Rust application (main.rs)
  |
  +-- Board ownership and buttons (board.rs / esp-idf-hal)
  |
  +-- 1bpp framebuffer and 5x7 glyph renderer (display.rs)
         |
         +-- generated C bindings (esp-idf-sys bindgen)
                 |
                 +-- official zectrix_epd C++ component (vendor)
                         |
                         +-- ESP-IDF GPIO/SPI drivers + SSD2683 waveform
```

| 文件 | 作用 |
| --- | --- |
| `rust-firmware/src/main.rs` | 启动、按键事件处理、计数、局刷/全刷循环 |
| `rust-firmware/src/button.rs` | 按键消抖（20 ms / 4 次确认）、短按与 1 s 长按事件 |
| `rust-firmware/src/board.rs` | 电源锁存、LED、按键和充电状态 GPIO |
| `rust-firmware/src/display.rs` | 1bpp 画布、5×7 字模、局刷区域打包、官方 EPD Rust 封装 |
| `rust-firmware/components/zectrix_epd/` | 官方 NOTE4 EPD C++ 驱动 + SSD2683 波形表 |
| `rust-firmware/Cargo.toml` | Rust 依赖 + esp-idf-sys extra_components 配置（生成 `zectrix_epd` FFI） |
| `rust-firmware/sdkconfig.defaults` | ESP32-S3、DIO、PSRAM、串口等配置 |
| `rust-firmware/partitions.csv` | 16 MB Flash 分区表 |
| `scripts/build-rust.ps1` | 激活本机 ESP-IDF 并构建 Rust 固件 |

### 官方 EPD 驱动来源

- 官方仓库：<https://github.com/itopinion/zectrix-note4-epd-demo>
- 本项目使用其中的 `components/zectrix_epd`。

不要删除 `rust-firmware/components/zectrix_epd/private_include/ssd2683_waveform.h`。它包含完整波形数据，是“程序报告刷新成功但屏幕仍显示旧画面”问题的关键修复。更新上游组件时应保留其目录结构并重新完整构建。

`Cargo.toml` 中：

```toml
[[package.metadata.esp-idf-sys.extra_components]]
component_dirs = ["components/zectrix_epd"]
bindings_header = "components/zectrix_epd/include/zectrix_epd.h"
bindings_module = "zectrix_epd"
```

这一节同时完成两件事：让 CMake 编译官方组件，并从 `zectrix_epd.h` 生成独立的 Rust FFI 模块（本仓库 release 产物里已观察到 `out/bindings.rs` 含 9 个 `zectrix_epd_*` 符号）。

构建后 ELF 中会导出以下 C ABI 符号（已用 `xtensa-esp32s3-elf-nm` 验证）：

```
T zectrix_epd_del
T zectrix_epd_get_default_config
T zectrix_epd_new
T zectrix_epd_power_off
T zectrix_epd_power_on
T zectrix_epd_refresh_full_1bpp
T zectrix_epd_refresh_partial_1bpp
```

## 5. Windows 开发环境

已验证组合：

| 项目 | 版本 |
| --- | --- |
| 操作系统 | Windows 11 x86-64 |
| ESP-IDF | 5.5.5（默认安装路径 `C:\Espressif\frameworks\esp-idf-v5.5.5-2`） |
| Python | 3.11（由 ESP-IDF 管理，环境目录 `C:\Espressif\python_env\idf5.5_py3.11_env\Scripts`） |
| Rust stable | 主机工具链 |
| Rust Xtensa | 频道 `esp`（`rust-toolchain.toml`） |
| `espflash` | 在 `~\.cargo\bin\cargo-espflash.exe` |
| VS C++ Build Tools | 包含 Windows 11 SDK |

确认环境：

```powershell
idf.py --version
rustup toolchain list
rustc +esp --version
cargo-espflash --version
```

预期能看到 ESP-IDF 5.5.x、名为 `esp` 的工具链和可运行的 `cargo-espflash`。若缺少：

```powershell
cargo install cargo-espflash
espup install
```

如果本机 ESP-IDF 路径或工具链版本不同，需要修改 `scripts/build-rust.ps1` 与 `rust-firmware/.cargo/config.toml`。

## 6. 首次连接与完整备份

用支持数据传输的 USB-C 线连接，然后：

```powershell
cargo-espflash board-info --port COMx
```

完整备份：

```powershell
.\scripts\backup-flash.ps1 -Port COMx
Get-FileHash .\backups\*.bin -Algorithm SHA256
```

预期 `backups\note4-factory-YYYYMMDD-HHMMSS.bin` 大小为 `16777216` 字节。备份与 SHA-256 应复制到至少一处物理隔离位置。

## 7. 构建 Rust 固件

从普通 PowerShell 在仓库根目录运行：

```powershell
.\scripts\build-rust.ps1 -Release
```

脚本会激活 ESP-IDF，然后在 `rust-firmware` 中执行 `cargo build --release`。Windows 下 ESP-IDF / CMake 的路径长度容易超限，因此 `.cargo/config.toml` 将输出目录固定为：

```text
D:\espbuild\xtensa-esp32s3-espidf\release\inkpaper-note4
```

修改 `Cargo.toml` 中的 extra component 配置后，如果新 FFI 模块没有出现，清理 `esp-idf-sys` 再构建：

```powershell
cd rust-firmware
cargo clean -p esp-idf-sys
cd ..
.\scripts\build-rust.ps1 -Release
```

## 8. 烧录与串口监视

从仓库根目录执行：

```powershell
cargo-espflash flash `
  --port COMx `
  --chip esp32s3 `
  --flash-size 16mb `
  --flash-mode dio `
  --flash-freq 80mhz `
  --partition-table .\rust-firmware\partitions.csv `
  D:\espbuild\xtensa-esp32s3-espidf\release\inkpaper-note4

cargo-espflash monitor --port COMx
```

退出 monitor 用 `Ctrl+C`。烧录失败时，先退出 monitor 再重新运行 flash 命令。

### 成功标准

1. 设备启动后不会因松开电源键而断电。
2. 绿色 LED 周期闪烁（`button.rs` 之外的 `led_tick` 在主循环控制 ~0.5 s 翻转）。
3. 屏幕经过明显的全刷过程后显示 `Hello world`。
4. 屏幕上显示 ENTER / UP / DOWN 三个计数。
5. 每按一次对应按键，串口日志打印按键名称，屏幕上的计数增加一次（局刷）。

当前示例每次按键都用官方局刷 API 只刷新变化的数字区域，启动和长按清残影时才执行全刷。

## 9. 显示数据约定

| 项目 | 值 |
| --- | --- |
| 分辨率 | 400 × 300 |
| 1bpp 帧大小 | `400 * 300 / 8 = 15000` 字节 |
| 行优先 | MSB-first |
| 颜色 | `1` = 白，`0` = 黑 |
| BUSY | 低电平忙 |
| 控制器 | SSD2683 |
| 官方 API | 全刷、局刷、16 级灰度全刷 |
| 4bpp 全刷后 | 直到再次完成 1bpp 全刷才能继续使用局刷 |

典型全刷生命周期：

```text
zectrix_epd_power_on
zectrix_epd_refresh_full_1bpp
zectrix_epd_power_off
```

电子纸断电后仍保持最后画面，这是正常特性。“仍看到旧画面”不能证明新固件没有运行，必须结合串口日志和真实刷新闪动判断。

## 10. 当前实现要点（源码索引）

- `main.rs` 主循环以 `POLL_INTERVAL_MS = 20`（ms）轮询三按键；按键事件用 `Option<ButtonEvent>` 携带。短按计数；长按标记 `full_refresh`，下一轮统一处理（避免一次按键内既刷局又刷全）。
- `display.rs::render(&ButtonCounts)` 是“每次刷新前重画整张 buffer”的简单模式——避免维护局部脏区的同时也限制了性能边界；后续加入 UI 层时应评估帧缓冲双缓冲与局部脏区合并。
- `display.rs::pack_rect(Rect)` 把矩形区域按 `(row_bytes, height)` 重打包为 MSB-first；只要源 framebuffer 与目标 rect 的 MSB-first 约定一致即可，与 5×7 字模的位置无关。
- `display.rs::glyph` 是 5×7 字模硬编码表，大写 A-Z、0-9 加小写字母映射为大写；其他字符返回全零。
- `board.rs::take()` 集中初始化：电源锁存拉高、LED 拉高（熄灭）、AVDD 拉低（关闭）、按键走 `Pull::Up`、`charging` 走 `Pull::Up`、`charge_done` 浮空输入。

## 11. 常见故障

### 固件烧录成功但不断复位

`sdkconfig.defaults` 必须含：

```text
CONFIG_ESPTOOLPY_FLASHMODE_DIO=y
```

不要改成 QIO。

### 日志说刷新完成，但屏幕完全不动

- 确认使用的是本仓库的官方 `zectrix_epd` 组件，而不是简化的 SSD1683 命令序列。
- 确认波形头文件存在且被编译进 `libzectrix_epd.a`（构建日志里能看到 `__idf_zectrix_epd.dir/zectrix_epd.cc.obj`）。
- 确认 `GPIOGPIO6` 电源由驱动管理（`zectrix_epd_power_on/off`）。

### `cargo` 找不到 `zectrix_epd` 模块

通常是 `esp-idf-sys` 缓存早于 extra component 配置：

```powershell
cd rust-firmware
cargo clean -p esp-idf-sys
cd ..
.\scripts\build-rust.ps1 -Release
```

### `failed to open D:/espbuild/.../.cargo-lock`

关闭仍在运行的 Cargo、rustc、IDE 检查任务或旧构建进程，然后重试。还要确认当前用户对 `D:\espbuild` 有写权限。

### 无法打开 COM 口

关闭其他 monitor / 串口工具，重新插拔 USB，检查设备管理器中的端口号。必要时按住 ENTER/BOOT，再触发复位进入下载模式。

### 上电后很快关机

`GPIO17` 是主电源软锁存。它必须在启动早期被配置为输出高电平。进入 deep sleep 前也要设计 RTC GPIO hold，否则设备可能真正断电。

### 屏幕没有立刻清空

这是正常的。电子纸断电保持画面；只有执行有效刷新波形才会改变内容。

## 12. 推荐开发顺序

1. 把当前 Hello World + 三按键固件保留为硬件冒烟测试基线。
2. 按键消抖 / 长按 / 组合键事件层已基本就绪；接下来评估刷新合并的宽限窗口、连续局刷次数限制。
3. 引入成熟图形库（`embedded-graphics` 已是 `esp-idf-hal` 支持的可选 feature）或建立统一画布、字体和布局层。
4. NVS 设置、Wi-Fi 配网和时间同步。
5. 电池 ADC、充电状态、PCF8563 RTC 与低功耗策略。
6. 音频（I2S + ES8311）、NFC（GT23SC6699）。
7. 内容协议、内容缓存、文件系统。
8. OTA、回滚、看门狗和故障恢复。

每加入一个外设，先做独立测试，再接入主应用。显示、电源和休眠改动的风险最高，应始终保留可恢复的串口路径与原厂备份。

## 13. 提交前检查

```powershell
cargo +esp fmt --manifest-path .\rust-firmware\Cargo.toml -- --check
.\scripts\build-rust.ps1 -Release
```

实机至少检查一次：冷启动、电源保持、初始全刷、三个按键各按一次、USB 重新连接和串口日志。

不要提交 `sdkconfig`、构建目录、原厂备份或包含设备凭据的日志。当前 `.gitignore` 已排除 `build/`、`managed_components/`、`dependencies.lock`、`sdkconfig`、`sdkconfig.old`、`backups/*.bin`、`rust-firmware/target/`、`rust-firmware/.embuild/`。

## 14. 恢复原厂固件

只使用该设备**自己的**完整备份：

```powershell
esptool.py -p COMx -b 921600 write_flash 0x0 .\backups\note4-factory-20260815-213553.bin
```

恢复后重新读取 Flash 或计算备份文件哈希，确认使用了正确镜像。不要把另一台设备或 NOTE4C 的备份写入本机。

## 15. 参考资料

- ZECTRIX 开源资料：<https://www.zectrix.com/open-source.html>
- NOTE4 硬件规格：<https://wiki.zectrix.com/zh/hardware/note/spec>
- 固件资料：<https://wiki.zectrix.com/zh/software/firmware>
- 社区开源固件：<https://wiki.zectrix.com/zh/software/Community-OpenSource-Firmware>
- 官方 NOTE4 EPD Demo：<https://github.com/itopinion/zectrix-note4-epd-demo>
- Slate 参考固件：<https://github.com/qiujun8023/slate>
- Rust on ESP Book：<https://docs.esp-rs.org/book/>
- espflash：<https://github.com/esp-rs/espflash>
