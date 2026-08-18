# rust-firmware — inkpaper-note4 crate

## OVERVIEW
全部产品代码所在的单 crate：ESP32-S3 固件，21 个平铺 src 模块（无子目录，含 `main.rs`）+ 1 个 C++ EPD FFI 组件。入口 `src/main.rs:85`（`[[bin]] inkpaper-note4`）。

## MODULE MAP (src/)
| 分组 | 模块 | 职责 |
|------|------|------|
| 编排 | `main.rs` | boot 序列 + 20ms 轮询主循环；两个单例（NVS handle L97-105、WifiManager L214）在此创建 |
| 硬件 | `board.rs` | 集中装配点 `Note4Board::take()`；GPIO/ADC/共享 I2C0（`Rc<RefCell<I2cDriver>>` 唯一实例） |
| 硬件 | `power.rs` | 深睡/唤醒原因/GPIO17 RTC hold；`enter_deep_sleep_with_wakeups` 是唯一安全重启路径 |
| 硬件 | `rtc.rs` | PCF8563 驱动 + 唯一硬件闹钟寄存器（L165-170） |
| 硬件 | `button.rs` / `watchdog.rs` / `audio.rs` / `nfc.rs` | 消抖+短按/1s 长按；任务 WDT；ES8311；GT23SC6699 |
| 渲染 | `canvas.rs` / `font8x16.rs` / `display.rs` | 1bpp 帧缓冲 + stroke_rect 描边；比例宽字模；EPD FFI 封装 + 主界面布局 |
| UI | `ui.rs` / `screens.rs` | 3 按键通用组件（列表选择、翻页、导航抽屉）；导航抽屉/日历/闹钟/待办/设置菜单（长按 UP/DOWN 进入，无设备端文本输入） |
| 服务 | `alarms.rs` / `todos.rs` / `storage.rs` | NVS store；闹钟负责挑最近一个武装 PCF8563 |
| 协议 | `control.rs` / `usb_console.rs` / `ble_control.rs` / `sync.rs` / `wifi.rs` | `>>IP `/`<<IP ` 命令协议（USB/BLE 共用 `control::dispatch`）；HTTPS 同步；WifiManager |

## FFI COMPONENT: components/zectrix_epd/
官方 SSD2683 C++ 驱动（784 LOC `.cc`），经 Cargo.toml `extra_components` + bindgen 生成 `zectrix_epd` 模块。`private_include/ssd2683_waveform.h` 是校准波形数据，**禁止删除**。EPD 电源/时序只能走 `zectrix_epd_power_on/off` FFI（`display.rs:92-136`）。

## CONVENTIONS (crate-level)
- 错误处理统一 `anyhow::Result`；外设初始化一律收进 `Note4Board::take()`，不在各模块自行 `Peripherals::steal()`。
- `main.rs` 的 mod 声明按字母序。
- 字模数据表用 `#[rustfmt::skip]`（`font8x16.rs`）。
- 构建注入：`build.rs` 发 `BUILD_EPOCH_SECS`（RTC 兜底）；rustflags 含 `--cfg espidf_time64`。

## ANTI-PATTERNS (code-enforced)
根 AGENTS.md 的红线在本 crate 的具体执行点：
- **Wi-Fi 连接**（`wifi.rs`）：① **同一 boot 可多次 `connect()`**——曾长期以为每 boot 只能连一次（二次连接 `Guru Meditation`/`Unhandled debug exception` 崩溃），后查明元凶是每次连接前跑的阻塞 `esp_wifi_scan_start()`（凭据来自 desktop `SetWifi`，扫描纯属多余），已移除；实测同 boot 连续 3 次 sync 全部成功。raw FFI `esp_wifi_connect()`/`disconnect()` 保留（esp-idf-svc wrapper 的 status 追踪仍会在复用时报 #503 panic）。② **永不调 `esp_wifi_stop()`**（stop→start 即崩溃触发点，`start()` 每进程一次）。③ 如需重启只用深睡路径（`power::restart_via_deep_sleep`，纯定时唤醒，无 ext1/RTC 闹钟），**不用 `esp_restart()`**。`restart_for_fresh_wifi_session` 现为死代码（`#[allow(dead_code)]` 逃生舱）。Wi-Fi 配置必须 `WIFI_STORAGE_RAM`。设备端已无 Wi-Fi 配网向导，`SetWifi` 只能来自 USB/BLE。
- **自动周期 sync**（`main.rs` `maybe_auto_sync`）：主循环每 ~30s 检查一次，距上次成功 sync 超过 `sync_interval_minutes`（NVS 默认 60，设置菜单 SYNC INTERVAL 可选 1/5/10/30/60）就调 `sync::sync_now`。检查仅在 Home 空闲时进行（菜单页阻塞主循环）；失败记日志、下次再试。手动 sync 与自动 sync 共用同一路径。
- **BLE**（`ble_control.rs`）：NimBLE 回调线程**禁止碰 `Note4Board`**，只 push channel，dispatch 仅在主循环（L80-83）；`deinit_full()` 后每次 `start()` 必须显式 `BLEDevice::init()`（L55-58）。BLE 与 Wi-Fi 共享射频，永不同时开——BLE 只在配对页存活。
- **RTC 闹钟**：只有一个硬件槽，多闹钟时必须总是写时间最近的那个（`alarms::next_due`，`rtc.rs:165-170`）；boot 时 `clear_alarm()` 清残留（`board.rs:92-94`）。
- **上电时序**：GPIO42(AVDD) 拉高必须先于 I2C0 init（`board.rs:69-70, 87`）；唤醒后先 `release_power_latch_hold()` 再在 GPIO17 上建 PinDriver（`power.rs:33-36` → `board.rs:61`）。
- **EPD**：4bpp 全刷后必须先做一次 1bpp 全刷才能恢复局刷；局刷前确认电源经 FFI 管理。
- **闹钟/待办 `id` 在列表内不得冲突**（sync-api 契约）；命令只在 Home 主循环轮询，菜单页不响应。
- **遥控状态变更必须触发主页重绘**（`main.rs:294-328`）：`CLOCK_RECT`（y=36..128）不覆盖 NEXT ALARM / OPEN TODOS 面板（y=139..233，`display.rs:48-77`）。`control::dispatch` 改写 NVS 后 NVS-渲染管道不会自动脏标——任何 USB/BLE 命令只要会改主页显示的数据（`SyncNow`、`clear_alarms`，及未来同类命令），必须在 `main` 轮询里 `dirty.push(FULL_SCREEN_RECT)`。命令在 dispatch 前用 `matches!` 抓 variant（`Command` 不是 `Copy`，dispatch 会 move），仅回复 `Reply::Ok` 时入栈；失败回复不浪费一次全刷。

## COMMANDS
```bash
./scripts/build-rust.sh --release   # 从仓库根跑；crate 内裸 cargo build 需已 source ESP-IDF 环境
cargo +esp fmt -- --check           # crate 目录内
cargo clippy                        # 需先 source ESP-IDF 环境；当前零警告（browse_page 有 #[allow(clippy::too_many_arguments)]）
```

## NOTES
- `.cargo/config.toml` 的 `IDF_PATH`/`LIBCLANG_PATH` 是机器相关的，见根 AGENTS.md NOTES。
- rust-analyzer 的可用性依赖根仓库 `.vscode/settings.json` + `esp-ra` 工具链（根因与重建方法见根 AGENTS.md NOTES）；改代码后可在编辑器内直接看诊断。
- 改 `sdkconfig.defaults` 时对照根红线 #2（DIO）与 partitions.csv 6 层相对路径约定。
- 无测试可跑；验证 = 烧录 + monitor（根 AGENTS.md COMMANDS）。
