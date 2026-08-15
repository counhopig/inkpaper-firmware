# NOTE4 Hardware Notes

这些硬件信息来自 ZECTRIX NOTE4 规格页和 Slate 固件文档的交叉整理，作为本项目的板级基线。

## Core

| Item | Value |
| --- | --- |
| MCU | ESP32-S3-WROOM-1 N16R8 |
| Flash | 16 MB；固件启动模式使用 DIO |
| PSRAM | 8 MB Octal |
| Display | 4.2 inch black-white EPD |
| Resolution | 400 x 300 |
| Audio | ES8311 codec, speaker, MEMS mic |
| Other | PCF8563 RTC, GT23SC6699 NFC |
| USB | USB-C CDC/JTAG |

## GPIO Map

| GPIO | Signal | Notes |
| --- | --- | --- |
| 0 | KEY_ENTER / BOOT | Active low, RTC-capable wake |
| 1 | STDBY_H | Charge IC full status |
| 2 | CHRG_L | Charge IC charging status, active low |
| 3 | LED_G | Green LED, active low |
| 4 | ADC_BAT | VBAT 1:2 divider |
| 5 | RTC_INT | PCF8563 interrupt |
| 6 | EPD_PWR_EN | EPD power rail |
| 7 | NFC_FD | NFC field detect |
| 8 | EPD_BUSY | Active low: low means busy |
| 9 | EPD_NRES | EPD reset |
| 10 | EPD_NDC | EPD data/command |
| 11 | EPD_NCS | EPD chip select |
| 12 | EPD_SCK | SPI clock |
| 13 | EPD_SDA | SPI MOSI |
| 14 | I2S_MCLK | ES8311 MCLK |
| 15 | I2S_SCLK | ES8311 BCLK |
| 16 | I2S_ASDOUT | Mic data in |
| 17 | PWR_ON | Main power latch, high keeps power |
| 18 | KEY_DET / PGDN | Down key and power-on feedback |
| 19 | USB_DN | USB D- |
| 20 | USB_DP | USB D+ |
| 21 | NFC_PWR | NFC power |
| 38 | I2S_LRCK | ES8311 LRCK |
| 39 | KEY_PGUP | Up key, not RTC-capable wake |
| 42 | PA_PWR_EN | Audio + I2C rail |
| 43 | TXD0 | UART TX |
| 44 | RXD0 | UART RX |
| 45 | I2S_DSDIN | Speaker data out |
| 46 | PA_CTRL | Speaker PA enable |
| 47 | I2C_SDA | I2C data |
| 48 | I2C_SCL | I2C clock |

GPIO 26-37 are occupied by Octal PSRAM and must not be used as ordinary GPIO.

## Power Rails

| Rail | Control | Notes |
| --- | --- | --- |
| Main power | GPIO17 | Must be held high before the user releases the power key |
| EPD 3V3 | GPIO6 | Can be off while EPD keeps its visible image |
| AVDD 3V3 | GPIO42 | Audio power and I2C pull-ups |

Deep sleep needs special care: GPIO17 must be held high through RTC GPIO hold, otherwise the main power latch releases and the device powers off.

## EPD Format

- Logical frame: 400 x 300.
- Packed 1bpp frame size: 15000 bytes.
- Bit order: MSB-first.
- Convention used by Slate: bit 1 = white, bit 0 = black.
- BUSY polarity: active low.

The controller is SSD2683. This repository uses the official ZECTRIX EPD component and its calibrated waveform data; keep that implementation as the known-good baseline when changing display code.
