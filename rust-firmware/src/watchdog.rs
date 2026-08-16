//! Thin wrapper around ESP-IDF's Task Watchdog Timer (TWDT).
//!
//! TWDT is already initialized by `CONFIG_ESP_TASK_WDT_INIT=y` (watching
//! the idle tasks), but our own main task isn't subscribed by default -
//! [`subscribe`] adds it. [`feed`] must then be called periodically from
//! every long-running poll loop in the firmware (the main loop, the Wi-Fi
//! connect wait, the provisioning wizard's screens) or TWDT will fire.
//! `CONFIG_ESP_TASK_WDT_PANIC=y` turns a timeout into a panic, which
//! reboots the device - this is the actual safety net: a genuine hang
//! (stuck peripheral, a future infinite-loop bug) gets the device back
//! instead of leaving it stuck until someone notices and power-cycles it.

use anyhow::{anyhow, Result};
use esp_idf_svc::sys::{esp, esp_task_wdt_add, esp_task_wdt_reset};

/// Subscribes the calling task (expected to be the main task, called once
/// at startup) to the TWDT.
pub fn subscribe() -> Result<()> {
    esp!(unsafe { esp_task_wdt_add(std::ptr::null_mut()) })
        .map_err(|e| anyhow!("esp_task_wdt_add failed: {e}"))
}

/// Resets the calling task's watchdog countdown. Call this at least once
/// per `CONFIG_ESP_TASK_WDT_TIMEOUT_S` from every loop that can run longer
/// than that between iterations.
pub fn feed() {
    // A reset call from an unsubscribed task would be a bug, but not one
    // worth taking the device down over - log and move on.
    if let Err(err) = esp!(unsafe { esp_task_wdt_reset() }) {
        log::warn!("esp_task_wdt_reset failed: {err}");
    }
}
