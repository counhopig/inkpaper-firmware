# Wi-Fi 连接问题调查记录

> 状态：**已解决**。本文档记录 2026-08-16 排查 NOTE4 (ESP32-S3) Wi-Fi STA 连接失败的过程与结论。

## 根因

**NVS 中存的 SSID 大小写与路由器实际广播的 SSID 不一致。**

供网时（`scripts/gen-nvs-wifi.py`）填的是 `XiaoMi_ED4E`（大写 `M`），但路由器实际广播的是
`Xiaomi_ED4E`（小写 `m`）。ESP32 的 SSID 匹配是逐字节区分大小写的：

- `EspWifi::scan()`（手动全扫描）**不带 SSID 过滤器**，会返回所有 AP，所以能看到 `Xiaomi_ED4E`。
- `EspWifi::connect()` 内部触发的是**带 SSID 过滤的定向扫描**，只找精确匹配配置里那个字符串的 AP。
  一个字符对不上，扫描结果就是 0 个，随即 `wifi:Send disconnect event, reason=201`（`NO_AP_FOUND`）。

这解释了当时最费解的现象：手动 scan 能看到目标 AP（RSSI 正常），connect 内部扫描却完全空 ——
不是驱动 bug，纯粹是配网时的拼写错误。

修复：用正确大小写重新生成 NVS 凭据。

```bash
./scripts/gen-nvs-wifi.py --ssid Xiaomi_ED4E --password <password> --port /dev/ttyACM0
```

烧录后重启，日志确认：`wifi:connected with Xiaomi_ED4E` → DHCP 拿到 IP → SNTP 同步 →
PCF8563 RTC 写入成功 → 时钟区域局刷。

## 已验证但被排除的假设

| 假设 | 结果 |
| --- | --- |
| `CONFIG_FREERTOS_HZ=1000` 导致闭源 wifi 库行为异常 | ❌ 现场把 `sdkconfig.defaults` 改成 100Hz 重新编译烧录，**失败现象完全一致**（`reason=201`），排除。已改回 1000。 |
| 官方 C demo（`itopinion/zectrix-note4-epd-demo`）对比 | 该 demo **从未调用 `esp_wifi_connect()`**，Wi-Fi RF 自检只做 `esp_wifi_scan_start()` 扫描（见 `components/zectrix_self_test/zectrix_self_test.cc` 的 `RunRf()`），没有可比对的连接代码。对本次排查没有直接帮助，但确认了官方固件也从未在这块硬件上验证过 STA connect 流程。 |
| RF 硬件 / esp-idf-svc 封装 / `WifiDriver` vs `EspWifi` / storage 模式 | 均已验证与本次 bug 无关（见下方复现环境的诊断日志）。 |

## 复现环境

- 芯片：ESP32-S3 rev v0.2，16 MB Flash（DIO）
- ESP-IDF：5.5.5（`~/esp/esp-idf`）
- esp-idf-svc：0.52.1，esp-idf-hal：0.46.2（vendored patch），heapless 0.9
- 相关代码：`rust-firmware/src/wifi.rs`（含诊断用手动 scan 打印，保留作为连接前的健康检查）
- 供网工具：`scripts/gen-nvs-wifi.py --ssid <SSID> --password <password> --port /dev/ttyACM0`
  （**SSID 大小写必须与路由器实际广播的完全一致**——手动 scan 打印出来的 SSID 就是权威来源）

## 后续方向

Wi-Fi STA 连接 + NTP 时间同步已跑通，可以作为「内容获取」链路的基础。下一步按
`README.md` 开发路线：NVS 设置/配网 UI、内容协议、OTA。
