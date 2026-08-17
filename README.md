# Inkpaper NOTE4 Firmware

自研固件起点，目标硬件：**ZECTRIX NOTE4 黑白屏版**（ESP32-S3-WROOM-1 N16R8，4.2 寸 400×300 SSD2683 EPD）。

> 本仓库**只面向 NOTE4 黑白屏**。NOTE4 与 NOTE4C 的屏幕硬件和固件不同，**不可混刷**。

## 系统架构

这个固件是三个仓库里的一个：

```
inkpaper-desktop (PC 工具)          inkpaper-server (后端)
       |  USB 串口 / BLE                  |  HTTPS GET（轮询）
       |  （只下发配置：                    |  （内容：alarms[]、todos[]）
       |   Wi-Fi 凭据、服务器地址+token）    |
       v                                  v
              inkpaper（本固件仓库）
```

设计原则：**设备不负责内容创作**。PC 工具只通过 USB/BLE 下发配置（Wi-Fi 凭据、服务器地址与 token）；实际内容（闹钟、待办）存在服务器上，设备以**结构化 JSON**（不是服务端渲好的位图）方式拉取——这样闹钟才能在完全没有网络的情况下按时响铃，因为固件本身就得知道具体的闹钟时间，而不是只会显示服务端给的一张图。

## 当前状态

`rust-firmware/` 是当前可在实机运行的 Rust 主固件，已经是一台真正能用的日历/闹钟/待办设备，不再是按键计数器 demo：

- GPIO17 主电源软锁存、GPIO0/39/18 三按键（短按+1s 长按）、官方 SSD2683 EPD 驱动（全刷/局刷）、16 MB Flash 分区、按键消抖等基础能力（详见下方硬件小节）与此前一致；状态 LED 心跳已在 UI 重写时移除。
- **主界面**：时钟 + 下一个闹钟时间 + 待办数量摘要；长按 UP/DOWN 打开导航抽屉（HOME / CALENDAR / ALARMS / TODOS / SETTINGS），SETTINGS 下是 SYNC NOW / BLE PAIRING / SLEEP。设备端不再提供 Wi-Fi/服务器配置界面——这两项现在只能通过 USB/BLE 命令下发（见下方 USB 控制协议）。
- **离线闹钟**（`src/alarms.rs` + `src/rtc.rs` 的 PCF8563 硬件闹钟寄存器 + `src/power.rs` 的 GPIO5 深睡唤醒 + `src/audio.rs` 的 ES8311 出声）：闹钟数据存在本地 NVS，响铃完全不依赖网络。芯片只有一路硬件闹钟寄存器，固件会自动把所有已存闹钟里最近的一个写进去。
- **待办列表**（`src/todos.rs`）：本地 NVS 存储，菜单里可勾选完成/取消。新增待办文本已不在设备端输入，由 Desktop/Server 下发。
- **HTTPS 同步客户端**（`src/sync.rs`）：双向 POST——设备上传本地 alarms/todos 的 `enabled`/`done` 标记，服务器合并后返回权威的完整列表，写入本地 store 并重新武装硬件闹钟。ETag 仍会被读取缓存，但当前 POST 流程不发 `If-None-Match`（每次都拿完整响应）；旧的 GET + 条件请求仍保留给历史固件兼容用。契约见 [`docs/sync-api.md`](docs/sync-api.md)。
- **USB 控制协议**（`src/control.rs` + `src/usb_console.rs`）：复用现有 USB-Serial-JTAG 控制台端口，用 `>>IP `/`<<IP ` 前缀区分命令/回复和普通日志，六个命令：`set_wifi` / `set_server` / `sync_now` / `get_status` / `clear_alarms` / `set_timezone`。`usb_console.rs` 已从独立读取线程改为主循环内联轮询（`UsbConsole::poll_command`），不再有单独的 Core0 读取任务。契约见 [`docs/control-protocol.md`](docs/control-protocol.md)。
- **BLE 控制通道**（`src/ble_control.rs`，`esp32-nimble`）：按需开启（进入"BLE PAIRING"菜单才启动，退出即销毁，NimBLE 常驻要占约 150KB RAM），走同一套命令协议。
- 统一画布/字体层：`src/canvas.rs`（1bpp 帧缓冲，含 `stroke_rect` 描边）+ `src/font8x16.rs`（比例宽字体，来自官方 demo 移植）；`src/ui.rs` 收拢了 3 按键交互的通用组件（列表选择、翻页、导航抽屉），供 `screens.rs`（日历/闹钟/待办/设置）使用。设备端已不做文本输入——用户输入的字符串一律来自 Desktop/Server。
- Wi-Fi STA（`src/wifi.rs`）+ SNTP，仅在 RTC 时间不可信时于开机时自动连一次；`storage.rs` 用 NVS 持久化 Wi-Fi 凭据、服务器配置、同步 ETag、本地时区偏移。设备端已不提供 Wi-Fi 配网向导，凭据只能通过 USB/BLE 的 `set_wifi`/`set_server` 命令下发。
- PCF8563 RTC、电量 ADC、深度睡眠（GPIO17 RTC hold）、ES8311 音频、GT23SC6699 NFC、I2C0 共享总线、任务看门狗——均沿用此前已验证的实现。

### 已知问题：Wi-Fi 二次连接崩溃（已绕过，非根本修复）

同一次开机周期内第二次连 Wi-Fi 会稳定崩溃（`Guru Meditation Error`），已确认是 ESP-IDF/esp-idf-svc 生态里已知但官方未修复的问题类别（espressif/esp-idf#7579、#11171，esp-rs/esp-idf-svc#503），不是调用方式的问题——连改成直接调原始 `esp_wifi_connect()` FFI、完全绕开 Rust 封装层也会在同一地址崩溃。当前用"检测到已经用过 Wi-Fi 就先干净重启一次（走已验证的深睡+定时唤醒路径，不用 `esp_restart()`，它也会踩同一个坑），重启后再重试"来规避。完整调查记录见 [`docs/calendar-alarm-todo-plan.md`](docs/calendar-alarm-todo-plan.md) 的 "Post-Phase-6" 一节。

### 尚未完成 / 尚未验证

> 备注：以下是当前**尚未**做完或**尚未**在实机上完整验证的部分：

- 文件系统、OTA 仍未实现。
- **真机上完整的"闹钟响铃→ENTER 解除"流程还没有人工确认过**（硬件闹钟寄存器的读写逻辑已验证正确，但没有真正听到/看到响铃解除的全过程）。
- **BLE 配对已经真机验证过一次**：固件 GATT 服务端和 `inkpaper-desktop` 的 `btleplug` 客户端实际连上过，收到过写入+notify 回复（`BLE connected` / `OK`）——但这是在更早的 egui 版 Desktop 下测的。**换成现在的 Tauri/Vue 版 Desktop 之后这条验证需要重新做一遍**（USB 那边同理）。完整调试记录见工作区根目录的 `INKPAPER_ENGINEERING_HISTORY.md` 第 4.3/11 节。
- Wi-Fi 二次连接的"重启后重试"体验需要用真实的 `espflash monitor` 会话确认（连续触发两次 `sync_now`，确认第一次干净重启、第二次真正连上并同步成功）。

完整的环境、安全事项、构建、烧录、调试与故障排查见 **[docs/development-guide.md](docs/development-guide.md)**；日历/闹钟/待办功能的完整开发过程和踩坑记录见 **[docs/calendar-alarm-todo-plan.md](docs/calendar-alarm-todo-plan.md)**；跨三个仓库的整体进度快照见 **[docs/project-status.md](docs/project-status.md)**；更详细的三仓库联合工作纪要（设计决策、真机调试记录、部署方式）见工作区根目录的 **[`../INKPAPER_ENGINEERING_HISTORY.md`](../INKPAPER_ENGINEERING_HISTORY.md)**。

## 仓库结构

```
inkpaper/
├── docs/
│   ├── development-guide.md       完整开发指南（必读）
│   ├── note4-hardware.md          板级 GPIO / 电源轨 / EPD 格式
│   ├── calendar-alarm-todo-plan.md 日历/闹钟/待办功能路线图 + Wi-Fi 崩溃调查记录
│   ├── control-protocol.md        USB/BLE 命令协议规格（给 inkpaper-desktop 对接）
│   ├── sync-api.md                HTTP 同步协议规格（给 inkpaper-server 对接）
│   └── project-status.md          跨三仓库的进度快照
├── rust-firmware/
│   ├── .cargo/config.toml           构建目标 / IDF 路径 / libclang
│   ├── Cargo.toml                   依赖 + extra_components (EPD FFI)
│   ├── Cargo.lock
│   ├── build.rs                     embuild::espidf::sysenv::output
│   ├── partitions.csv               nvs / phy_init / factory / storage
│   ├── rust-toolchain.toml          channel = "esp"
│   ├── sdkconfig.defaults           DIO / 80 MHz / OCT PSRAM / USB Serial/JTAG / NimBLE
│   ├── src/
│   │   ├── main.rs                  入口 + 主循环 + Wi-Fi/深睡/闹钟响铃编排
│   │   ├── alarms.rs                多闹钟 NVS store + 挑最近一个写进 PCF8563 硬件寄存器
│   │   ├── todos.rs                 待办 NVS store
│   │   ├── screens.rs               导航抽屉/日历/闹钟/待办/设置界面
│   │   ├── ui.rs                    3 按键交互通用组件（列表选择、翻页、导航抽屉）
│   │   ├── sync.rs                  HTTPS 同步客户端（含 sync_now 的 Wi-Fi 重连规避逻辑）
│   │   ├── control.rs               USB/BLE 共用的命令/回复协议
│   │   ├── usb_console.rs           USB 串口命令通道（复用日志端口）
│   │   ├── ble_control.rs           BLE GATT 控制通道（按需开关）
│   │   ├── audio.rs                 ES8311 编解码器
│   │   ├── board.rs                 电源锁存 / LED / 按键 / 充电 GPIO / ADC / SharedI2c
│   │   ├── button.rs                消抖 + 短按/1s 长按
│   │   ├── canvas.rs                1bpp 帧缓冲 + 绘制原语（含 stroke_rect 描边）
│   │   ├── font8x16.rs              比例宽字体（官方 demo 移植）
│   │   ├── display.rs               EPD 封装 + 主界面布局
│   │   ├── nfc.rs                   GT23SC6699 NFC
│   │   ├── power.rs                 深度睡眠 / GPIO17 hold / 唤醒原因判定
│   │   ├── rtc.rs                   PCF8563 驱动（含硬件闹钟寄存器）
│   │   ├── storage.rs               NVS 持久化（Wi-Fi/服务器配置/同步 ETag）
│   │   ├── watchdog.rs              任务看门狗
│   │   └── wifi.rs                  WifiManager（单例复用，规避二次连接崩溃）
│   └── components/zectrix_epd/
├── scripts/
├── vendor/
└── backups/
```

`inkpaper-desktop`（PC 配置工具，Tauri 2 + Vue 3 + Rust，USB/BLE 双传输）和 `inkpaper-server`（后端，Rust + axum + SQLite）是独立仓库，与本仓库同级：`../inkpaper-desktop`、`../inkpaper-server`。

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

成功标志：电源保持按通、LED 闪烁、屏幕全刷后显示时钟 + 下一闹钟 + 待办数量摘要。若 NVS 里已有 Wi-Fi 凭据，日志还会显示连上 AP、NTP 同步；首次开机长按 UP 3s 可进入配网向导。按 ENTER 打开菜单可以看日历、管理闹钟/待办、配置服务器地址、手动触发同步、开启 BLE 配对。

### 故障信号

- **烧录成功但反复复位** → 确认 `sdkconfig.defaults` 仍为 `CONFIG_ESPTOOLPY_FLASHMODE_DIO=y`（NOTE4 实机不支持 QIO）。
- **日志说刷新完成但屏幕完全不动** → 确认有编译进静态库的 `libzectrix_epd.a`；`vendor/esp-idf-hal/` 切到正确版本后重跑 `cargo clean -p esp-idf-sys`。
- **上电后立刻关机** → 确认 `GPIO17` 在启动早期被拉高（GPIO 锁存）。
- **无法识别 `espflash`/`esptool.py`** → ESP-IDF 环境未激活。先执行 `get_idf`（或 `. ~/esp/esp-idf/export.sh`）。
- **触发 Sync Now 或重新打开 Wi-Fi 向导后设备突然重启一次** → 这是预期行为，见上方"已知问题"一节，不是故障；重启完成后再操作一次即可。

## 版本

| 组件 | 版本 |
| --- | --- |
| `inkpaper-note4` (crate) | `0.1.0` |
| `esp-idf-sys` | 0.37.2 |
| `esp-idf-svc` | 0.52.1 |
| `esp-idf-hal` | 0.46.2（vendor + sdmmc patch）|
| `embuild` (build) | 0.33.3 |
| `esp32-nimble` (BLE) | 0.12 |
| `serde` / `serde_json` | 1.x |

## 开发路线

1. ~~硬件冒烟基线、画布/字体层、Wi-Fi/NVS/配网、电池/RTC/低功耗、音频/NFC、看门狗~~ 完成（详见 `docs/development-guide.md`）。
2. ~~日历 / 离线闹钟 / 待办~~ 完成（`docs/calendar-alarm-todo-plan.md` 全部 6 个 Phase）。
3. ~~USB/BLE 配置协议~~ 完成（`control.rs` / `usb_console.rs` / `ble_control.rs`）。
4. ~~HTTPS 内容同步~~ 完成（`sync.rs`，契约见 `docs/sync-api.md`）。
5. ~~PC 工具（`inkpaper-desktop`）、服务器（`inkpaper-server`）~~ 首版完成，见各自仓库；三个仓库现在都已提交 git 并推送到各自的 `origin/main`。
6. 真机验证：闹钟响铃全流程、BLE 配对端到端、Wi-Fi 重连规避的重启体验——见上方"尚未完成"一节。
7. 文件系统、OTA、回滚仍未做。

Slate（`https://github.com/qiujun8023/slate`）是同款硬件的一个参考实现，本仓库会保持为你自己的 NOTE4 固件起点。
