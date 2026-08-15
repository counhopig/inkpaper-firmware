use anyhow::{Context, Result};
use esp_idf_svc::hal::adc::attenuation::DB_12;
use esp_idf_svc::hal::adc::oneshot::{config::AdcChannelConfig, AdcChannelDriver, AdcDriver};
use esp_idf_svc::hal::gpio::{Input, Output, PinDriver, Pull};
use esp_idf_svc::hal::i2c::{I2cConfig, I2cDriver};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::units::Hertz;

use crate::button::Button;
use crate::display::EpdDisplay;
use crate::power;
use crate::rtc::{Pcf8563, PCF8563_ADDR};
pub type BoardAdc = AdcDriver<'static, esp_idf_svc::hal::adc::ADCU1>;

const BATTERY_ADC_CHANNEL_CONFIG: AdcChannelConfig = AdcChannelConfig {
    attenuation: DB_12,
    ..AdcChannelConfig::new()
};

const I2C_FREQUENCY: Hertz = Hertz(400_000);

pub struct Note4Board {
    _power_latch: PinDriver<'static, Output>,
    led: PinDriver<'static, Output>,
    _avdd_power: PinDriver<'static, Output>,
    pub key_enter: Button,
    pub key_up: Button,
    pub key_down: Button,
    charging: PinDriver<'static, Input>,
    charge_done: PinDriver<'static, Input>,
    adc: BoardAdc,
    pub display: EpdDisplay,
    pub rtc: Pcf8563<'static>,
}

impl Note4Board {
    pub fn take() -> Result<Self> {
        let peripherals = Peripherals::take()?;
        let pins = peripherals.pins;

        // After a deep-sleep wakeup GPIO17 may still be held high by the
        // RTC slow IO block from the previous session. Releasing the hold
        // before constructing the PinDriver prevents the two from fighting.
        power::release_power_latch_hold()?;

        let mut power_latch = PinDriver::output(pins.gpio17)?;
        power_latch.set_high()?;

        let mut led = PinDriver::output(pins.gpio3)?;
        led.set_high()?;

        let mut avdd_power = PinDriver::output(pins.gpio42)?;
        avdd_power.set_high()?;

        let key_enter = Button::new(pins.gpio0.into(), Pull::Up)?;
        let key_up = Button::new(pins.gpio39.into(), Pull::Up)?;
        let key_down = Button::new(pins.gpio18.into(), Pull::Up)?;
        let charging = PinDriver::input(pins.gpio2, Pull::Up)?;
        let charge_done = PinDriver::input(pins.gpio1, Pull::Floating)?;
        let display = EpdDisplay::new()?;

        let adc: BoardAdc = AdcDriver::new(peripherals.adc1)?;
        // GPIO4 is reserved as the analog battery pin and intentionally
        // untouched here so the ADC driver can attach it on demand inside
        // `battery_millivolts`. The peripheral is fetched by stealing from
        // the (consumed) Peripherals handle, which is safe exactly once
        // after `Peripherals::take()` in `Note4Board::take`.

        let i2c_config = I2cConfig::new().baudrate(I2C_FREQUENCY);
        let i2c = I2cDriver::new(
            peripherals.i2c0,
            pins.gpio47,
            pins.gpio48,
            &i2c_config,
        )
        .context("failed to install I2C0 driver on GPIO47/48")?;
        let mut rtc = Pcf8563::new(i2c, PCF8563_ADDR);
        rtc.probe().context("PCF8563 not responding on I2C bus")?;
        if let Err(err) = rtc.clear_alarm() {
            log::warn!("PCF8563 clear_alarm failed: {err}");
        }

        Ok(Self {
            _power_latch: power_latch,
            led,
            _avdd_power: avdd_power,
            key_enter,
            key_up,
            key_down,
            charging,
            charge_done,
            adc,
            display,
            rtc,
        })
    }

    pub fn set_led(&mut self, on: bool) -> Result<()> {
        if on {
            self.led.set_low()?;
        } else {
            self.led.set_high()?;
        }
        Ok(())
    }

    pub fn charging_state(&self) -> (bool, bool) {
        (self.charging.is_low(), self.charge_done.is_high())
    }

    /// Reads battery voltage via GPIO4 (ADC1 channel 3, on-board 1:2 divider)
    /// in mV. Returns the ESP-IDF mV reading for the ADC pin doubled, since
    /// the divider halves VBAT before the pin sees it. The DB_12 attenuation
    /// tops out around ~3.1 V on ESP32-S3, so the doubled result is clamped
    /// to `u16::MAX`.
    pub fn battery_millivolts(&mut self) -> Result<u16> {
        let peripherals = unsafe { Peripherals::steal() };
        let mut channel = AdcChannelDriver::new(
            &self.adc,
            peripherals.pins.gpio4,
            &BATTERY_ADC_CHANNEL_CONFIG,
        )?;
        let adc_mv = self.adc.read(&mut channel)?;
        let vbat_mv = (adc_mv as u32) * 2;
        Ok(vbat_mv.min(u16::MAX as u32) as u16)
    }
}
