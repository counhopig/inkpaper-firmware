# Inkpaper NOTE4 Firmware

这是一个面向 **ZECTRIX NOTE4 黑白屏** 的自研固件起点。

目标硬件是 NOTE4 / ZecTrix Note4 V1.0：ESP32-S3 N16R8、4.2 英寸 400x300 黑白 EPD、ES8311 音频、MEMS 麦、NFC、三颗按键、锂电池和 Type-C。

> 注意：本项目只面向 NOTE4 黑白屏，不适用于 NOTE4C 四色屏。NOTE4 和 NOTE4C 的屏幕驱动与固件不可混刷。

## 当前状态

当前提供 Rust 主固件。Rust 固件已在实机验证：

- GPIO17 主电源软锁存保持，避免松开电源键后断电。
- GPIO3 绿色 LED 心跳，确认 app 正常运行。
- GPIO0 / GPIO39 / GPIO18 三个按键读取。
- 官方 SSD2683 驱动和完整波形已接入，可执行 400x300 黑白全刷。
- 启动显示 `Hello world`，按键后更新 ENTER / UP / DOWN 计数。
- 16 MB Flash + 约 12 MB 预留存储分区已预置。

完整的环境、备份、构建、烧录、恢复、架构与排错说明见：

**[NOTE4 固件开发指南](docs/development-guide.md)**

## 目录

```text
.
├── docs/
│   ├── bringup-checklist.md
│   ├── development-guide.md
│   └── note4-hardware.md
├── rust-firmware/
│   ├── .cargo/config.toml
│   ├── Cargo.toml
│   ├── partitions.csv
│   ├── sdkconfig.defaults
│   └── src/
└── scripts/
    ├── backup-flash.ps1
    └── build-rust.ps1
```

## 开发环境

推荐使用 ESP-IDF 5.5.x。Rust 固件使用已安装的 `esp` Xtensa 工具链。进入 ESP-IDF PowerShell 后：

```powershell
cd rust-firmware
cargo build
```

Windows 上 `esp-idf-sys` 对构建路径长度有限制，因此 Rust 编译产物固定放在 `D:\espbuild`。
也可以从普通 PowerShell 运行 `scripts\build-rust.ps1`，脚本会自动激活本机 ESP-IDF 5.5.5 环境。

确认已经备份原厂 Flash 后，构建 release 固件：

```powershell
.\scripts\build-rust.ps1 -Release
```

如果是第一次刷自己的固件，请先完整备份 16 MiB 原厂 Flash：

```powershell
.\scripts\backup-flash.ps1 -Port COMx
```

## 开发路线

建议按这个顺序推进：

1. 备份原厂 Flash，确认可恢复。
2. 刷入当前 Rust 固件，确认串口日志、LED、按键和电源保持正常。
3. 保留官方 EPD 驱动作为全刷基线，再验证局刷和刷新合并。
4. 建立应用 UI、字体和输入事件层。
5. 加入 Wi-Fi 配网、NVS 凭据、低功耗 deep sleep。
6. 决定内容协议：自研极简 HTTP、兼容 Slate，或完全离线应用。

Slate 是一个很好的参考项目，但这个仓库会保持为你自己的 NOTE4 固件起点。
