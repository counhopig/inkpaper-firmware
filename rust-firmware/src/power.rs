use anyhow::Result;
use esp_idf_svc::sys::{
    esp_deep_sleep_start, esp_sleep_enable_ext0_wakeup, esp_sleep_enable_gpio_switch,
    esp_sleep_get_wakeup_cause, gpio_hold_dis, gpio_hold_en,
};

const GPIO_NUM_17: i32 = 17;
const GPIO_NUM_0: i32 = 0;

/// Releases the GPIO17 RTC hold left over from a previous deep-sleep session.
/// Must be called before any `PinDriver::output(gpio17)` is constructed; the
/// hold bypasses normal output control, so the pin stays frozen high until
/// cleared.
pub fn release_power_latch_hold() -> Result<()> {
    unsafe { gpio_hold_dis(GPIO_NUM_17) };
    Ok(())
}

/// Logs the cause reported by the ROM bootloader (or `Unknown` if the chip
/// was power-cycled instead of woken from deep sleep).
pub fn log_wakeup_cause() {
    let cause = unsafe { esp_sleep_get_wakeup_cause() };
    log::info!("Wakeup cause raw = 0x{:x}", cause);
}

/// Configures the wake source (ENTER button low on GPIO0) and enters deep
/// sleep. GPIO17 is held high via the RTC slow IO block so the main power
/// latch survives the sleep. The function does not return.
pub fn enter_deep_sleep_with_button_wake() -> ! {
    // ext0 = single GPIO, level-triggered. GPIO0 ENTER is RTC-capable.
    let ret = unsafe { esp_sleep_enable_ext0_wakeup(GPIO_NUM_0, 0) };
    if ret != 0 {
        log::error!(
            "esp_sleep_enable_ext0_wakeup(GPIO0, low) failed: 0x{:x}",
            ret
        );
    }
    // Keep GPIO switches off so the held output (GPIO17) is not yanked back
    // to its sleep default while we are asleep.
    unsafe { esp_sleep_enable_gpio_switch(false) };
    // Hold GPIO17 high so the main power latch does not release.
    unsafe { gpio_hold_en(GPIO_NUM_17) };
    log::info!("Entering deep sleep; wake on ENTER (GPIO0 low)");
    unsafe { esp_deep_sleep_start() };
}