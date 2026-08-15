# NOTE4 Bring-up Checklist

> NOTE4 must boot in DIO flash mode. The factory boot log reports `mode:DIO`;
> forcing a QIO boot header causes a watchdog reset before the bootloader starts.

## Before Flashing

- Confirm the device is NOTE4 black-white, not NOTE4C four-color.
- Use a USB-C cable that supports data transfer.
- Install ESP-IDF 5.5.x.
- Back up the complete 16 MiB factory Flash.

```powershell
.\scripts\backup-flash.ps1 -Port COMx
```

Keep the backup in more than one place before experimenting with display drivers or partition layouts.

## First Rust Flash

```powershell
.\scripts\build-rust.ps1 -Release
espflash flash --port COMx --chip esp32s3 --flash-size 16mb --flash-mode dio --flash-freq 80mhz --partition-table .\rust-firmware\partitions.csv D:\espbuild\xtensa-esp32s3-espidf\release\inkpaper-note4
espflash monitor --port COMx
```

Expected behavior:

- The device stays powered after boot.
- Green LED blinks once per second.
- Serial log prints button events.
- The EPD refreshes and displays `Hello world` with three button counters.

## EPD Test

The Rust firmware uses the official ZECTRIX SSD2683 component and waveform table. Expected behavior:

- A visible full-refresh sequence occurs after boot.
- Each button press increments its counter and performs another full refresh.
- The screen keeps the last image after EPD power is switched off.

See [development-guide.md](development-guide.md) before changing the display driver or waveform.

## Recovery

If your custom firmware fails to boot but USB serial still appears, erase or reflash from ESP-IDF.

If you need to restore the factory image:

```powershell
esptool.py -p COMx -b 921600 write_flash 0x0 .\backups\note4-factory-YYYYMMDD-HHMMSS.bin
```

Do not restore a backup from another model or another device unless you understand the credentials and calibration data consequences.
