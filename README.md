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
- 按键计数在 NVS 命名空间 `inkpaper` 中以 `u32` 持久化（`storage.rs`），启动时 `load()`、按键静默 1 s 后 `save()`，重启后保留。
- 串口每 1 s 打印一次 `Power state: charging=… charge_done=… vbat_mV=…`；电池电压走 ESP-IDF 5.x oneshot ADC（`AdcDriver::new(adc1)` + `GPIO4` = ADC1 CH3），单位是经 1:2 分压校正后的 mV。
- PCF8563 RTC 挂在 I2C0（`GPIO47`/`GPIO48`，AVDD `GPIO42` 上电拉高），启动时 `read_time()`，若 `voltage_low` 置位则用 `build.rs` 记录的构建 epoch 写入芯片并显示；时间显示在屏幕左上角（`YYYY-MM-DD HH:MM:SS`，RTC 状态文本）。
- GPIO17 RTC hold 深度睡眠：长按 DOWN 3 s 进入深睡，ENTER（GPIO0 低电平）唤醒；唤醒后 RTC 若健康则**跳过** Wi-Fi/NTP 重连，直接进入按键循环（见 `docs/wifi-connect-issue.md` 之外的 perf 提交）。
- Wi-Fi STA 连接（`src/wifi.rs`，esp-idf-svc `EspWifi`）+ SNTP 时间同步，结果写回 PCF8563；仅在首次开机 / 上电复位 / RTC `voltage_low` 时才连网，避免每次深睡唤醒都联网。
- **设备端 Wi-Fi 配网向导**（`src/provision.rs`）：主界面长按 UP 3 s 进入，扫描并从列表中选 AP（不用手打 SSID，从根源上避免大小写打错导致连不上——参见 `docs/wifi-connect-issue.md`），UP/DOWN 转字符轮盘输入密码，提交前先实际 `connect()` 验证成功才写入 NVS。仍保留 `scripts/gen-nvs-wifi.py` 作为脚本化供网的备选。
- 统一的画布/字体层：`src/canvas.rs`（1bpp 帧缓冲 + 像素/矩形/文字绘制原语，独立于 EPD 驱动）+ `src/font.rs`（5×7 位图字模）；`display.rs` 只负责 EPD 句柄与屏幕布局。
- ES8311 音频编解码器（`src/audio.rs`）：I2C0 控制寄存器初始化（16 kHz 单声道，MCLK=256x=4.096 MHz）+ I2S0 TX 播放（GPIO14/15/38/45），扬声器 PA 使能走 GPIO46。当前只做了硬件冒烟（开机放一段测试音），未接入真正的内容/TTS 播放。
- GT23SC6699 NFC（`src/nfc.rs`）：I2C0 读 UID block（addr 0x55）+ 读 field-detect（GPIO7，低有效），供电走 GPIO21。同样只做了硬件冒烟（开机读一次 UID），完整 NDEF 读写未移植。
- I2C0 现在由 RTC / 音频编解码器 / NFC 三者通过 `board::SharedI2c`（`Rc<RefCell<I2cDriver>>`）共享同一条总线实例。
### 尚未完成
> 备注：以下功能当前**尚未**移植到本仓库：

文件系统、OTA；内容协议未定，由用户自行设计后端与固件拉取逻辑（设备侧目前的设想是"只拉一张服务端渲好的 1bpp 位图，按 ETag 增量更新，固件不理解内容类型"，具体协议待定）。

完整的环境、安全事项、构建、烧录、调试与故障排查见 **[docs/development-guide.md](docs/development-guide.md)**。

## 仓库结构

```
inkpaper/
├── docs/
│   ├── development-guide.md  完整开发指南（必读）
│   ├── note4-hardware.md     板级 GPIO / 电源轨 / EPD 格式
│   └── wifi-connect-issue.md Wi-Fi STA 连接排查记录（已解决：SSID 大小写不匹配）
├── rust-firmware/
│   ├── .cargo/config.toml           构建目标 / IDF 路径 / libclang
│   ├── Cargo.toml                   依赖 + extra_components (EPD FFI)
│   ├── Cargo.lock
│   ├── build.rs                     embuild::espidf::sysenv::output
│   ├── partitions.csv               nvs / phy_init / factory / storage
│   ├── rust-toolchain.toml          channel = "esp"
│   ├── sdkconfig.defaults           DIO / 80 MHz / OCT PSRAM / USB Serial/JTAG
│   ├── src/
│   │   ├── main.rs                  入口 + 按键事件 + 局刷/全刷循环 + Wi-Fi/深睡编排
│   │   ├── audio.rs                 ES8311 编解码器（I2C 初始化 + I2S0 TX 播放）
│   │   ├── board.rs                 电源锁存 / LED / 按键 / 充电 GPIO / oneshot ADC / SharedI2c
│   │   ├── button.rs                消抖 + 短按/1s 长按
│   │   ├── canvas.rs                1bpp 帧缓冲 + 像素/矩形/文字绘制原语
│   │   ├── font.rs                  5x7 位图字模
│   │   ├── display.rs               EPD Rust 封装 + 屏幕布局（依赖 canvas/font）
│   │   ├── nfc.rs                   GT23SC6699 NFC（UID 读取 + field-detect）
│   │   ├── power.rs                 深度睡眠 / GPIO17 RTC hold / 唤醒原因
│   │   ├── provision.rs             设备端 Wi-Fi 配网向导（长按 UP 3s 进入）
│   │   ├── rtc.rs                   PCF8563 驱动
│   │   ├── storage.rs               NVS 持久化按键计数 + Wi-Fi 凭据
│   │   └── wifi.rs                  Wi-Fi STA 连接 + 扫描 + NTP 同步
│   └── components/zectrix_epd/
│       ├── CMakeLists.txt
│       ├── zectrix_epd.cc
│       ├── include/zectrix_epd.h
│       └── private_include/ssd2683_waveform.h
├── scripts/
│   ├── build-rust.sh                激活 ESP-IDF 5.5.5 并构建（Linux）
│   ├── build-rust.ps1               激活 ESP-IDF 5.5.5 并构建（Windows，遗留）
│   ├── backup-flash.ps1             完整 16 MiB Flash 备份（Windows，遗留）
│   └── gen-nvs-wifi.py              脚本化 Wi-Fi 供网（设备端向导的备选）
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
| 操作系统 | Arch Linux x86-64 |
| ESP-IDF | 5.5.5（git clone 到 `~/esp/esp-idf`，`alias get_idf='. $HOME/esp/esp-idf/export.sh'`）|
| Python | 由 ESP-IDF 管理（`~/.espressif/python_env/idf5.5_py3.14_env`） |
| Rust 工具链 | 频道 `esp`（Espressif Xtensa 1.95.0.0，espup 安装于 `~/.rustup/toolchains/esp`） |
| Xtensa GCC | `xtensa-esp-elf` 15.2.0（espup 安装，`~/.rustup/toolchains/esp/xtensa-esp-elf/`）|
| 烧录工具 | `espflash` / `cargo-espflash` 4.5.0（extra 仓库，pacman 安装） |
| USB 串口 | USB Serial/JTAG → `/dev/ttyACM0`（用户需在 `uucp` 组） |

`rust-firmware/.cargo/config.toml` 中的 `LIBCLANG_PATH` 指向本机 espup 的 esp-clang 路径；如工具链版本不同需同步修改。`.cargo/config.toml` 不再指定 `target-dir`，产物在 `rust-firmware/target/` 下。`sdkconfig.defaults` 用 `${CMAKE_CURRENT_SOURCE_DIR}` 变量引用 `partitions.csv`（esp-idf-sys 的 CMake 源码目录在 out 下，深度固定 6 层），因此仓库内不含硬编码的 Windows 路径。

## 快速开始

第一次刷自己的固件前，先完整备份原厂 16 MiB Flash：

```bash
# espflash save-image 完整读取 16 MiB
espflash save-image \
  --port /dev/ttyACM0 --chip esp32s3 \
  --flash-size 16mb --flash-mode dio --flash-freq 80mhz \
  backups/note4-factory-$(date +%Y%m%d-%H%M%S).bin
sha256sum backups/*.bin > backups/SHA256SUMS
```

构建并烧录：

```bash
# 构建 release 镜像
./scripts/build-rust.sh --release

# 烧录
espflash flash \
  --port /dev/ttyACM0 \
  --chip esp32s3 \
  --flash-size 16mb \
  --flash-mode dio \
  --flash-freq 80mhz \
  --partition-table rust-firmware/partitions.csv \
  rust-firmware/target/xtensa-esp32s3-espidf/release/inkpaper-note4

# 查看日志
espflash monitor --port /dev/ttyACM0
```

成功标志：电源保持按通、LED 闪烁、串口日志输出按键事件、EPD 完成全刷后显示 `Hello world` + 三个按键计数。若 NVS 里已有 Wi-Fi 凭据（脚本供网或设备端向导写入的），日志还会显示连上 AP、NTP 同步、屏幕左上角时钟更新；首次开机长按 UP 3 s 可进入配网向导。

### 故障信号

- **烧录成功但反复复位** → 确认 `sdkconfig.defaults` 仍为 `CONFIG_ESPTOOLPY_FLASHMODE_DIO=y`（NOTE4 实机不支持 QIO）。
- **日志说刷新完成但屏幕完全不动** → 确认有编译进静态库的 `libzectrix_epd.a`；`vendor/esp-idf-hal/` 切到正确版本后重跑 `cargo clean -p esp-idf-sys`。
- **上电后立刻关机** → 确认 `GPIO17` 在启动早期被拉高（GPIO 锁存）。
- **无法识别 `espflash`/`esptool.py`** → ESP-IDF 环境未激活。先执行 `get_idf`（或 `. ~/esp/esp-idf/export.sh`）。

## 版本

| 组件 | 版本 |
| --- | --- |
| `inkpaper-note4` (crate) | `0.1.0` |
| `esp-idf-sys` | 0.37.2 |
| `esp-idf-svc` | 0.52.1 |
| `esp-idf-hal` | 0.46.2（vendor + sdmmc patch）|
| `embuild` (build) | 0.33.3 |

## 开发路线（参考 `docs/development-guide.md#12`）

1. ~~保留当前示例作为硬件冒烟基线。~~ 完成。
2. ~~加入统一画布、字体与布局层。~~ 完成（`canvas.rs` + `font.rs`；`display.rs` 只剩 EPD 句柄与布局）。
3. ~~NVS 设置、Wi-Fi 配网、时间同步。~~ 完成（`wifi.rs` + `provision.rs` 设备端向导 + SNTP）。
4. ~~电池 ADC、充电状态、PCF8563 RTC 与低功耗策略。~~ 完成（深睡时 RTC GPIO17 hold；深睡唤醒且 RTC 健康时跳过 Wi-Fi/NTP 重连）。
5. ~~音频（I2S + ES8311）、NFC（GT23SC6699）。~~ 硬件冒烟完成（`audio.rs` / `nfc.rs`，均已用实机验证）；真正的内容播放/NDEF 读写留给上层。
6. 内容协议、内容缓存、文件系统 —— 由用户自行设计与实现（后端 + 固件拉取逻辑），本仓库暂不深入。
7. OTA、回滚、看门狗与故障恢复。

Slate 是一个参考实现，但本仓库会保持为你自己的 NOTE4 固件起点。
