use anyhow::Result;
use esp_idf_svc::sys::{
    esp_deep_sleep_start, esp_sleep_enable_ext1_wakeup, esp_sleep_enable_gpio_switch,
    esp_sleep_enable_timer_wakeup, esp_sleep_ext1_wakeup_mode_t_ESP_EXT1_WAKEUP_ANY_LOW,
    esp_sleep_get_ext1_wakeup_status, esp_sleep_get_wakeup_cause,
    esp_sleep_source_t_ESP_SLEEP_WAKEUP_EXT1, esp_sleep_source_t_ESP_SLEEP_WAKEUP_UNDEFINED,
    gpio_hold_dis, gpio_hold_en,
};

const GPIO_NUM_17: i32 = 17;
const GPIO_NUM_0: i32 = 0;
/// PCF8563 `RTC_INT`, open-drain active low; asserted when the RTC alarm
/// fires (see `rtc::Pcf8563::set_alarm`). RTC-capable on ESP32-S3
/// (GPIOs 0-21), unlike GPIO39 (UP), which is why the alarm line was wired
/// here instead.
const GPIO_NUM_5: i32 = 5;

/// Who woke the chip up, disambiguated from `esp_sleep_get_wakeup_cause`'s
/// EXT1 case via `esp_sleep_get_ext1_wakeup_status`'s per-pin bitmask -
/// both ENTER and the RTC alarm share the same ext1 wake source, so the
/// cause alone doesn't tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeCause {
    Enter,
    RtcAlarm,
    Other,
}

/// Releases the GPIO17 RTC hold left over from a previous deep-sleep session.
/// Must be called before any `PinDriver::output(gpio17)` is constructed; the
/// hold bypasses normal output control, so the pin stays frozen high until
/// cleared.
pub fn release_power_latch_hold() -> Result<()> {
    unsafe { gpio_hold_dis(GPIO_NUM_17) };
    Ok(())
}

/// Logs the cause reported by the ROM bootloader and reports whether this
/// boot is a wake from deep sleep (as opposed to a power-on reset, a fresh
/// flash, or a reset button press).
pub fn log_wakeup_cause() -> bool {
    let cause = unsafe { esp_sleep_get_wakeup_cause() };
    log::info!("Wakeup cause raw = 0x{:x}", cause);
    cause != esp_sleep_source_t_ESP_SLEEP_WAKEUP_UNDEFINED
}

/// Configures wake sources - ENTER (GPIO0) or the RTC alarm line (GPIO5),
/// either driving low - plus an optional periodic timer wake for
/// background content resync, then enters deep sleep. GPIO17 is held high
/// via the RTC slow IO block so the main power latch survives the sleep.
/// The function does not return.
pub fn enter_deep_sleep_with_wakeups(resync_interval: Option<std::time::Duration>) -> ! {
    // ext1 = multi-GPIO, any-selected-pin-low wakes the chip. Both GPIO0
    // and GPIO5 are RTC-capable on ESP32-S3 (GPIOs 0-21).
    let mask: u64 = (1u64 << GPIO_NUM_0) | (1u64 << GPIO_NUM_5);
    let ret = unsafe {
        esp_sleep_enable_ext1_wakeup(mask, esp_sleep_ext1_wakeup_mode_t_ESP_EXT1_WAKEUP_ANY_LOW)
    };
    if ret != 0 {
        log::error!(
            "esp_sleep_enable_ext1_wakeup(GPIO0|GPIO5, low) failed: 0x{:x}",
            ret
        );
    }
    if let Some(interval) = resync_interval {
        let ret = unsafe { esp_sleep_enable_timer_wakeup(interval.as_micros() as u64) };
        if ret != 0 {
            log::error!("esp_sleep_enable_timer_wakeup failed: 0x{:x}", ret);
        }
    }
    // Keep GPIO switches off so the held output (GPIO17) is not yanked back
    // to its sleep default while we are asleep.
    unsafe { esp_sleep_enable_gpio_switch(false) };
    // Hold GPIO17 high so the main power latch does not release.
    unsafe { gpio_hold_en(GPIO_NUM_17) };
    log::info!("Entering deep sleep; wake on ENTER/RTC alarm (GPIO0|GPIO5 low)");
    unsafe { esp_deep_sleep_start() };
}

/// Disambiguates which ext1 pin caused the wake. Must be called before any
/// GPIO reconfiguration that might change pin levels.
pub fn wake_cause() -> WakeCause {
    let cause = unsafe { esp_sleep_get_wakeup_cause() };
    if cause != esp_sleep_source_t_ESP_SLEEP_WAKEUP_EXT1 {
        return WakeCause::Other;
    }
    let status = unsafe { esp_sleep_get_ext1_wakeup_status() };
    if status & (1u64 << GPIO_NUM_5) != 0 {
        WakeCause::RtcAlarm
    } else if status & (1u64 << GPIO_NUM_0) != 0 {
        WakeCause::Enter
    } else {
        WakeCause::Other
    }
}
