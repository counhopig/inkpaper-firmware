# ZECTRIX NOTE4 Firmware Development Guide

本文档面向接手本仓库的固件开发者，记录已经验证过的硬件信息、开发环境、构建与烧录流程、墨水屏驱动方案，以及容易重复踩到的问题。

## 1. 项目边界与当前状态

目标设备是 **ZECTRIX NOTE4 黑白屏版**，不是 NOTE4C 四色屏版。

已经在实机上验证：

- ESP32-S3 revision v0.2，可通过 USB Serial/JTAG 连接。
- 16 MB Flash，启动镜像必须使用 DIO 模式。
- Rust 固件可以稳定启动并保持整机供电。
- 绿色 LED、ENTER、UP、DOWN 三个按键工作正常。
- 官方 SSD2683 EPD 驱动可以完成 400 x 300 黑白全屏刷新。
- 当前示例显示 `Hello world`，每次按键后更新对应计数。
- 原厂 16 MB Flash 已做完整备份，可用于恢复本机。

当前固件是开发起点，不是完整产品。Wi-Fi、音频、RTC、NFC、电池管理、文件系统、休眠和 OTA 尚未完成。

## 2. 重要安全事项

1. 确认设备是 NOTE4 黑白屏版。NOTE4C 的显示硬件和固件不同，不能混刷。
2. 第一次修改分区或烧录前，必须读取完整 16 MiB Flash 并保存 SHA-256。
3. 原厂备份可能包含设备唯一数据、凭据和校准信息，不应公开提交到 Git。
4. 不要使用 QIO 启动模式。虽然模组规格支持 QIO，NOTE4 实机的工厂启动日志和本项目验证结果均为 DIO；QIO 镜像会在应用启动前反复复位。
5. 墨水屏必须使用匹配面板的波形和电源时序。不要用通用 SSD1683 示例替换官方驱动后直接反复刷新。
6. 烧录前关闭占用串口的 monitor、串口终端和其他 IDE。

## 3. 已验证的硬件

| 项目 | 值 |
| --- | --- |
| MCU | ESP32-S3, revision v0.2 |
| 模组 | ESP32-S3-WROOM-1 N16R8 |
| Flash | 16 MB |
| PSRAM | 8 MB Octal |
| 屏幕 | 4.2 英寸黑白 EPD，400 x 300 |
| EPD 控制器 | SSD2683 |
| 晶振 | 40 MHz |
| USB | ESP USB Serial/JTAG |

完整 GPIO 表见 [note4-hardware.md](note4-hardware.md)。当前示例直接使用以下信号：

| GPIO | 用途 | 说明 |
| --- | --- | --- |
| 0 | ENTER / BOOT | 低电平按下 |
| 3 | 绿色 LED | 低电平点亮 |
| 6 | EPD 电源 | 由官方驱动管理 |
| 8 | EPD BUSY | 低电平忙 |
| 9 | EPD RESET | 由官方驱动管理 |
| 10 | EPD DC | 由官方驱动管理 |
| 11 | EPD CS | SPI3 |
| 12 | EPD SCLK | SPI3 |
| 13 | EPD MOSI | SPI3 |
| 17 | 主电源锁存 | 启动后必须尽早拉高 |
| 18 | DOWN | 低电平按下 |
| 39 | UP | 低电平按下 |
| 42 | 音频/I2C 电源 | 当前示例保持关闭 |

GPIO 26 至 37 被 Octal PSRAM 占用，不能作为普通 GPIO 使用。

## 4. 软件架构

应用主体使用 Rust，底层建立在 ESP-IDF 上：

```text
Rust application (main.rs)
  |
  +-- Board ownership and buttons (board.rs / esp-idf-hal)
  |
  +-- 1bpp framebuffer and text renderer (display.rs)
          |
          +-- generated C bindings (esp-idf-sys bindgen)
                  |
                  +-- official zectrix_epd C++ component
                          |
                          +-- ESP-IDF GPIO/SPI drivers + SSD2683 waveform
```

关键文件：

| 路径 | 作用 |
| --- | --- |
| `rust-firmware/src/main.rs` | 启动、按键边沿检测、计数和刷新循环 |
| `rust-firmware/src/board.rs` | 电源锁存、LED、按键和充电状态 GPIO |
| `rust-firmware/src/display.rs` | 1bpp 画布、字体及官方 EPD API 的 Rust 封装 |
| `rust-firmware/components/zectrix_epd/` | 官方 NOTE4 EPD C++ 驱动和波形表 |
| `rust-firmware/Cargo.toml` | Rust 依赖及 ESP-IDF extra component/bindgen 配置 |
| `rust-firmware/sdkconfig.defaults` | ESP32-S3、DIO、PSRAM、串口等构建配置 |
| `rust-firmware/partitions.csv` | 16 MB Flash 分区表 |
| `scripts/build-rust.ps1` | 激活本机 ESP-IDF 并构建 Rust 固件 |

### 官方 EPD 驱动来源

驱动来自 ZECTRIX NOTE4 官方示例仓库：

- <https://github.com/itopinion/zectrix-note4-epd-demo>
- 本项目使用其中的 `components/zectrix_epd`。

不要删除 `private_include/ssd2683_waveform.h`。它包含完整波形数据，是此前“程序报告刷新成功但屏幕仍显示旧画面”问题的关键修复。更新上游组件时，应保留其目录结构并重新执行完整构建。

`Cargo.toml` 中的 `package.metadata.esp-idf-sys.extra_components` 同时完成两件事：让 CMake 编译官方组件，并从 `zectrix_epd.h` 生成独立的 Rust FFI 模块。

## 5. Windows 开发环境

已验证组合：

- Windows 11 x86-64
- ESP-IDF 5.5.5
- Python 3.11（由 ESP-IDF 管理）
- Rust stable，用于主机工具
- Espressif `esp` Xtensa Rust 工具链，用于 ESP32-S3
- `espflash` 4.5.0
- Visual Studio C++ Build Tools 与 Windows 11 SDK

推荐通过 Espressif ESP-IDF Tools Installer 安装 ESP-IDF、USB 驱动、PowerShell 环境和 Rust Xtensa 支持。确认环境：

```powershell
idf.py --version
rustup toolchain list
rustc +esp --version
espflash --version
```

预期能看到 ESP-IDF 5.5.x、名为 `esp` 的工具链和可运行的 `espflash`。若缺少烧录工具：

```powershell
cargo install espflash
```

本机脚本默认 ESP-IDF 位于：

```text
C:\Espressif\frameworks\esp-idf-v5.5.5-2
```

如果安装位置或版本不同，需要修改 `scripts/build-rust.ps1` 和 `rust-firmware/.cargo/config.toml` 中的路径及版本。

## 6. 首次连接与完整备份

插入支持数据传输的 USB-C 线，然后查看芯片：

```powershell
espflash board-info
```

如有多个串口，明确指定：

```powershell
espflash board-info --port COM5
```

在仓库根目录完整备份 16 MiB Flash：

```powershell
.\scripts\backup-flash.ps1 -Port COM5
Get-FileHash .\backups\*.bin -Algorithm SHA256
```

备份文件应为 `16777216` 字节。将文件和 SHA-256 记录复制到另一个安全位置。`backups/*.bin` 已被 `.gitignore` 排除。

## 7. 构建 Rust 固件

从普通 PowerShell 在仓库根目录运行：

```powershell
.\scripts\build-rust.ps1 -Release
```

脚本会激活 ESP-IDF，然后在 `rust-firmware` 中执行 `cargo build --release`。

Windows 下 ESP-IDF/CMake 的路径长度容易超限，因此 `.cargo/config.toml` 将输出目录固定为：

```text
D:\espbuild
```

生成的应用 ELF 位于：

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
espflash flash `
  --port COM5 `
  --chip esp32s3 `
  --flash-size 16mb `
  --flash-mode dio `
  --flash-freq 80mhz `
  --partition-table .\rust-firmware\partitions.csv `
  D:\espbuild\xtensa-esp32s3-espidf\release\inkpaper-note4
```

然后监视日志：

```powershell
espflash monitor --port COM5
```

退出 monitor 使用 `Ctrl+C`。烧录失败时，先退出 monitor，再重新运行 flash 命令。

### 成功标准

1. 设备启动后不会因松开按键而断电。
2. 绿色 LED 周期闪烁。
3. 墨水屏经过明显的刷新过程后显示 `Hello world`。
4. 屏幕显示 ENTER、UP、DOWN 三个计数。
5. 每按一次对应按键，串口日志打印按键名称，屏幕计数增加一次。

当前示例每次按键都执行全刷，适合验证但不适合最终交互体验。产品代码应加入消抖、刷新合并和局刷策略。

## 9. 显示数据约定

- 分辨率：400 x 300。
- 1bpp 帧大小：`400 * 300 / 8 = 15000` 字节。
- 行优先，像素位序为 MSB-first。
- `1` 表示白色，`0` 表示黑色。
- 官方驱动公开全刷、局刷和 16 级灰度全刷 API。

典型刷新生命周期：

```text
zectrix_epd_power_on
zectrix_epd_refresh_full_1bpp
zectrix_epd_power_off
```

屏幕断电后仍会保持最后画面，这是电子纸的正常特性。因此“仍看到旧画面”不能证明新固件没有运行，必须结合串口日志和真实刷新闪动判断。

## 10. 常见故障

### 固件烧录成功但不断复位

检查镜像是否使用 DIO。`sdkconfig.defaults` 必须包含：

```text
CONFIG_ESPTOOLPY_FLASHMODE_DIO=y
```

不要改成 QIO。

### 日志说刷新完成，但屏幕完全不动

确认使用的是本仓库的官方 `zectrix_epd` 组件，而不是简化的 SSD1683 命令序列；确认波形头文件存在且被编译；确认 GPIO6 电源由驱动管理。

### `cargo` 找不到 `zectrix_epd` 模块

通常是 `esp-idf-sys` 缓存早于 extra component 配置。执行：

```powershell
cd rust-firmware
cargo clean -p esp-idf-sys
cd ..
.\scripts\build-rust.ps1 -Release
```

### `failed to open D:/espbuild/.../.cargo-lock`

关闭仍在运行的 Cargo、rustc、IDE 检查任务或旧构建进程，然后重试。还要确认当前用户对 `D:\espbuild` 有写权限。

### 无法打开 COM 口

关闭其他 monitor/串口工具，重新插拔 USB，检查设备管理器中的端口号。必要时按住 ENTER/BOOT，再触发复位进入下载模式。

### 上电后很快关机

`GPIO17` 是主电源软锁存。它必须在启动早期被配置为输出高电平。进入 deep sleep 前也要设计 RTC GPIO hold，否则设备可能真正断电。

### 屏幕没有立刻清空

这是正常的。电子纸断电保持画面；只有执行有效刷新波形才会改变内容。

## 11. 恢复原厂固件

只使用该设备自己的完整备份：

```powershell
esptool.py -p COM5 -b 921600 write_flash 0x0 `
  .\backups\note4-factory-YYYYMMDD-HHMMSS.bin
```

恢复后重新读取 Flash 或计算备份文件哈希，确认使用了正确镜像。不要把另一台设备或 NOTE4C 的备份写入本机。

## 12. 推荐开发顺序

1. 把当前 Hello World + 三按键固件保留为硬件冒烟测试。
2. 加入可靠的按键消抖、长按和组合键事件层。
3. 在全刷基线成功后验证官方局刷 API，并限制连续局刷次数。
4. 引入成熟图形库或建立统一画布、字体和布局层。
5. 实现 NVS 设置、Wi-Fi 配网和时间同步。
6. 实现电池 ADC、充电状态、RTC 和低功耗策略。
7. 再开发音频、NFC、内容同步和文件系统。
8. 最后设计 OTA、回滚、看门狗和故障恢复。

每加入一个外设，先做独立测试，再接入主应用。显示、电源和休眠改动的风险最高，应始终保留可恢复的串口路径和原厂备份。

## 13. 提交前检查

```powershell
cargo +esp fmt --manifest-path .\rust-firmware\Cargo.toml -- --check
.\scripts\build-rust.ps1 -Release
```

实机至少检查一次：冷启动、电源保持、初始全刷、三个按键各按一次、USB 重新连接和串口日志。不要提交 `sdkconfig`、构建目录、原厂备份或包含设备凭据的日志。

## 14. 参考资料

- ZECTRIX 开源资料：<https://www.zectrix.com/open-source.html>
- NOTE4 硬件规格：<https://wiki.zectrix.com/zh/hardware/note/spec>
- 固件资料：<https://wiki.zectrix.com/zh/software/firmware>
- 社区开源固件：<https://wiki.zectrix.com/zh/software/Community-OpenSource-Firmware>
- 官方 NOTE4 EPD Demo：<https://github.com/itopinion/zectrix-note4-epd-demo>
- Slate 参考固件：<https://github.com/qiujun8023/slate>
- Rust on ESP Book：<https://docs.esp-rs.org/book/>
- espflash：<https://github.com/esp-rs/espflash>

