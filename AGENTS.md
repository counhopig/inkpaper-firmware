# PROJECT KNOWLEDGE BASE

**Generated:** 2026-08-16 · **Updated:** 2026-08-17 · **Commit:** f41555b · **Branch:** main

## OVERVIEW
ZECTRIX NOTE4 黑白屏版（ESP32-S3-WROOM-1 N16R8，4.2" 400×300 SSD2683 EPD）自研固件仓库。Rust 单 crate（`rust-firmware/`），ESP-IDF 5.5.5 + `esp` Xtensa 工具链，实现日历/离线闹钟/待办 + HTTPS 同步 + USB/BLE 配置通道。三仓库系统之一（`../inkpaper-desktop` PC 工具、`../inkpaper-server` 后端，均为独立仓库）。设计原则：设备不创作内容——配置通道只下发 Wi-Fi 凭据/服务器地址+token，内容以结构化 JSON 拉取，闹钟离线可响。

## STRUCTURE
```
inkpaper/
├── docs/            # 全部文档：开发指南(必读)、硬件规格、两份跨仓库协议契约、路线图/调查记录
├── rust-firmware/   # 唯一产品代码：inkpaper-note4 crate（21 个平铺 src 模块 ~4.3k LOC + C++ EPD 组件）
├── scripts/         # 构建/烧录/配网脚本（.sh=Linux，.ps1=Windows 孪生，+1 Python 配网）
├── vendor/          # vendored esp-idf-hal 0.46.2 + sdmmc patch（第三方只读，见 UNIQUE STYLES）
└── backups/         # 原厂 16MB flash 备份（gitignored，设备唯一且含凭据——永不提交）
```
无根 Cargo.toml、无 workspace、无 CI、无 LICENSE。`.omo/`、`.claude/` 是工具目录，非项目内容。

## WHERE TO LOOK
| Task | Location | Notes |
|------|----------|-------|
| 改任何固件行为 | `rust-firmware/src/` | 模块职责表见 `rust-firmware/AGENTS.md` |
| 环境/构建/烧录/故障排查 | `docs/development-guide.md` | 必读；含"安全事项（不可妥协）"一节 |
| GPIO/电源轨/EPD 数据格式 | `docs/note4-hardware.md` | 板级硬件权威来源 |
| USB/BLE 命令协议 | `docs/control-protocol.md` | 与 inkpaper-desktop 的契约 |
| HTTP 同步协议 | `docs/sync-api.md` | 与 inkpaper-server 的契约 |
| Wi-Fi 连接历史与 scan 元凶调查 | `rust-firmware/src/wifi.rs`（`WifiManager` 文档注释） | 二次连接崩溃的完整推理链与最终根因（已解决），代码级摘要见 `rust-firmware/AGENTS.md` |
| 实机验证状态/跨仓库进度 | `docs/project-status.md` | 含"尚未实机验证"清单 |

## CODE MAP
无 codegraph 工具；rust-analyzer 自 2026-08-18 起可用（`esp-ra` 工具链 + IDF env 注入，见 NOTES）——以下为静态分析，引用中心性未测量。

| Symbol | Type | Location | Role |
|--------|------|----------|------|
| `main()` | fn | `rust-firmware/src/main.rs:85` | 唯一入口：boot + 20ms 轮询主循环，wire 全部 20 个 `mod` 模块 |
| `Note4Board::take()` | fn | `src/board.rs` | 硬件集中装配：RTC/EPD/按键/LED/音频/NFC/ADC，共享 I2C0 |
| `WifiManager` | struct | `src/wifi.rs` | Wi-Fi 单例——全仓库最关键约束的载体（见 ANTI-PATTERNS #3） |
| `control::dispatch` | fn | `src/control.rs` | USB/BLE 共用命令分发，仅主循环上下文调用 |
| `sync::sync_now` | fn | `src/sync.rs` | HTTPS 同步（ETag/304）+ Wi-Fi 重启规避逻辑 |
| `AlarmStore` / `TodoStore` | struct | `src/alarms.rs` / `src/todos.rs` | NVS 持久化；闹钟挑最近一个写 PCF8563 唯一硬件寄存器 |
| 两个共享单例 | — | `main.rs:97-105, 214` | 一个 NVS partition handle + 一个 WifiManager，每进程各建一次 |

## CONVENTIONS
- 文档用中文；commit 为 conventional 格式且用英文描述（`feat:`/`fix:`/`docs:` 等，小写开头）。
- **无测试、无 CI**：`harness = false`，提交前检查 = fmt + clippy 零警告 + release 构建 + 实机人工验证（development-guide §13）。
- 工具链固定 `esp` 频道（`rust-toolchain.toml`）；格式化必须 `cargo +esp fmt`，禁用 stable/nightly。
- 体积优先：release `opt-level="s"`、dev `"z"`；`build-std=["std","panic_abort"]`。
- `cargo run` = 烧录+监视（runner=espflash）；裸 `cargo build` 在未 source ESP-IDF 环境的 shell 里必失败——一律走 `scripts/build-rust.sh`。

## ANTI-PATTERNS (THIS PROJECT)
红线（违反=变砖或必崩；代码锚点见 `rust-firmware/AGENTS.md`）：
1. **NOTE4 与 NOTE4C 不可混刷**——硬件/波形/固件不兼容；恢复只用本机自己的备份。
2. **Flash 永远 DIO，禁止 QIO**——QIO 镜像在 app 启动前看门狗复位循环。
3. **Wi-Fi 同 boot 可多次连接**——曾以为"一次开机只允许成功连 Wi-Fi 一次"（二次连接必崩），2026-08-17 查明元凶是每次连接前的阻塞 `esp_wifi_scan_start()`（凭据来自 desktop `SetWifi`，扫描纯属多余），移除后同 boot 连续多次 sync 实测全部成功。**永不调 `esp_wifi_stop()`**（stop→start 即崩溃触发点，`start()` 每进程一次）；如需重启只能走深睡+~100ms 定时唤醒路径，**严禁 `esp_restart()`**。详见 `rust-firmware/src/wifi.rs` 文档注释。
4. **GPIO17 电源锁存**：boot 早期拉高 + 深睡期间 RTC GPIO hold——否则物理断电。
5. `ssd2683_waveform.h` 与官方 zectrix_epd 组件**禁止删除/替换**；不用通用 SSD1683 序列。
6. 永不提交：`sdkconfig`、`backups/*.bin`、`target/`、含设备凭据的日志。

## UNIQUE STYLES
- `vendor/esp-idf-hal` = crates.io 0.46.2 + `sdmmc_host_t` 字段 patch（适配 IDF 5.5.5），经 `[patch.crates-io]` 接入。**只读**；sync upstream 不得丢 patch 字段；上游支持 5.5.5 后整目录连同 patch 段删除（`vendor/README.md`）。
- C++ EPD 驱动经 `extra_components` + bindgen 融入 crate（FFI 模块 `zectrix_epd`）；改后必须 `cargo clean -p esp-idf-sys`。
- `build.rs` 注入 `BUILD_EPOCH_SECS`，作 RTC 掉电后的兜底时间源。
- `partitions.csv` 经 `${CMAKE_CURRENT_SOURCE_DIR}/../../../../../../` 固定 6 层相对深度引用——勿动层级。

## COMMANDS
```bash
./scripts/build-rust.sh --release        # 构建（脚本自 source ESP-IDF 环境；不加参数=debug）
cargo +esp fmt --manifest-path rust-firmware/Cargo.toml -- --check   # 提交前检查
./scripts/build-rust.sh && cd rust-firmware && cargo clippy   # clippy 零警告（需 IDF env，可借 build-rust.sh 的 export）
espflash flash --port /dev/tty.usbmodem1101 --chip esp32s3 --flash-size 16mb \
  --flash-mode dio --flash-freq 80mhz --partition-table rust-firmware/partitions.csv \
  rust-firmware/target/xtensa-esp32s3-espidf/release/inkpaper-note4  # 烧录（macOS 端口名；Linux 为 /dev/ttyACM0）
espflash monitor --port /dev/tty.usbmodem1101     # 串口日志（唯一的"测试"手段）
```

## NOTES
- `rust-firmware/.cargo/config.toml` 含机器相关路径（`IDF_PATH=~/esp/esp-idf`、`LIBCLANG_PATH` 指向本机 espup esp-clang）——换机或换工具链版本需手改。
- **rust-analyzer 可用性依赖两个机器相关件**（均随本仓库根 `.vscode/settings.json` 生效）：① `esp-ra` 工具链（`rustup toolchain link esp-ra ~/esp/esp-ra`）：`bin/cargo` 是 wrapper（`--version` 报 1.96.0，其余转发 esp cargo），`rustc`/`rustdoc` 为指向 esp 工具链的符号链接——espup 重装后需重建；② `.vscode/settings.json` 注入 `RUSTUP_TOOLCHAIN=esp-ra` + IDF env（IDF_PATH、tools、venv python，路径为 `~/esp/esp-idf` 与 `~/.espressif/python_env/idf5.5_py3.9_env/bin`）。根因：rust-analyzer 0.3.3016 将 esp cargo 的 `1.95.0-nightly` 判为 <1.95.0，走已移除的 `--lockfile-path` 参数导致 `cargo metadata` 降级 `--no-deps`（编辑器假 unresolved import）；wrapper 引导其走 `-Zlockfile-path` 分支（esp cargo 支持）。rustup 上游升级到 rustc ≥1.96.0-nightly 后可移除整个 hack。
- 设备支持自动周期 sync（默认 60 分钟，设置菜单可改 1/5/10/30/60），失败会在下个周期重试；`esp_wifi_stop()` 与 `esp_restart()` 仍属红线，见红线 #3。
- 尚未实机验证：闹钟响铃全流程、BLE 端到端配对——改相关代码后无法靠"编译通过"背书。
