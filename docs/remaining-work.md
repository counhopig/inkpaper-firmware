# Firmware Remaining Work

Updated: 2026-08-21  
Flashed revision: `86a7f57` (`main`)

## Verification completed in this pass

- ESP32-S3 release build completed successfully.
- Firmware flashed through `/dev/tty.usbmodem1101` using DIO, 80 MHz and a
  16 MB flash layout.
- Boot ROM confirmed ESP32-S3 revision 0.2, DIO mode, 16 MB flash and 8 MB
  PSRAM; the PSRAM memory test passed.
- The application initialized the power latch, RTC, display and Wi-Fi stack
  without a watchdog reset or panic.
- The RTC retained a valid clock and the display completed full and partial
  refreshes.
- An aligned background network cycle connected, obtained DHCP and validated
  the HTTPS certificate after boot.

## Required physical-device verification

1. **Alarm end-to-end:** test RTC alarm wake from deep sleep, audible ring,
   ENTER dismissal, AF acknowledgement, one-shot removal and re-arming of the
   next recurring alarm.
2. **Alerts outside Home:** verify alarm, urgent Inbox and high-importance Todo
   screens interrupt Calendar, Settings, list/detail and BLE pairing screens,
   then return to a correctly redrawn page.
3. **Connection soak:** repeatedly disconnect/reconnect BLE and unplug/replug
   USB over a long-running session; confirm BLE teardown does not affect later
   Wi-Fi synchronization.
4. **Repeated Wi-Fi cycles:** leave the device running across many urgent and
   full-sync boundaries and confirm there is no second-connect crash or heap
   degradation.
5. **Large data sets:** exercise maximum practical alarm, Todo and Inbox lists;
   check pagination, truncation, NVS capacity and e-paper ghosting.

## Remaining engineering work

1. Add host-runnable tests for alarm ordering/recurrence, scheduler boundary
   transitions, reminder de-duplication, ID allocation and sync merge rules.
2. Add a repeatable hardware smoke-test checklist or serial-log harness for
   boot, synchronization, alarm and BLE/USB recovery.
3. Fix stale ESP-IDF application metadata. The boot log's `App version` and
   `Compile time` can come from the cached `esp-idf-sys` CMake build instead of
   the Rust firmware revision just linked and flashed. Embed the Git revision
   from `build.rs` or force the application descriptor to rebuild.
4. Replace machine-specific ESP-IDF, Python and rust-analyzer paths with a
   documented local bootstrap/configuration mechanism.
5. Design and implement OTA with signed images, health confirmation and a
   rollback partition before enabling remote firmware upgrades.

## Release blockers

- Do not claim the offline-alarm feature verified until item 1 in the physical
  test list passes on the actual NOTE4 hardware.
- Do not ship OTA until rollback and power-loss behavior are demonstrated.
- Continue flashing only ESP32-S3 NOTE4 images in DIO mode; NOTE4C firmware and
  QIO mode remain forbidden.
