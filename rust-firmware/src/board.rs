use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Context, Result};
use esp_idf_svc::hal::adc::attenuation::DB_12;
use esp_idf_svc::hal::adc::oneshot::{config::AdcChannelConfig, AdcChannelDriver, AdcDriver};
use esp_idf_svc::hal::gpio::{Input, Output, PinDriver, Pull};
use esp_idf_svc::hal::i2c::{I2cConfig, I2cDriver};
use esp_idf_svc::hal::i2s::{I2sDriver, I2sTx};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::units::Hertz;

use crate::audio::{self, Es8311};
use crate::button::Button;
use crate::display::EpdDisplay;
use crate::nfc::{self, NfcTag};
use crate::power;
use crate::rtc::{Pcf8563, PCF8563_ADDR};
pub type BoardAdc = AdcDriver<'static, esp_idf_svc::hal::adc::ADCU1>;

/// I2C0 is shared between the PCF8563 RTC, the ES8311 audio codec, and the
/// GT23SC6699 NFC tag, so all three hold a clone of the same driver
/// instance rather than each owning their own (only one `I2cDriver` may be
/// installed per port).
pub type SharedI2c = Rc<RefCell<I2cDriver<'static>>>;

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
    pub rtc: Pcf8563,
    /// `None` when the ES8311 failed to initialize; the rest of the board
    /// (display/buttons/RTC/Wi-Fi) still works without it.
    pub audio: Option<Es8311>,
    /// `None` when the GT23SC6699 failed to initialize; see `audio` above.
    pub nfc: Option<NfcTag>,
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

        // Starts high (matches the previous always-on behavior) until the
        // first `update_charging_led` call in the main loop takes over.
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
        let i2c = I2cDriver::new(peripherals.i2c0, pins.gpio47, pins.gpio48, &i2c_config)
            .context("failed to install I2C0 driver on GPIO47/48")?;
        let i2c_bus: SharedI2c = Rc::new(RefCell::new(i2c));
        let mut rtc = Pcf8563::new(i2c_bus.clone(), PCF8563_ADDR);
        rtc.probe().context("PCF8563 not responding on I2C bus")?;
        if let Err(err) = rtc.clear_alarm() {
            log::warn!("PCF8563 clear_alarm failed: {err}");
        }

        // ES8311 audio codec: I2S0 TX on GPIO14/15/38/45 (MCLK/BCLK/WS/DOUT),
        // speaker PA enabled on GPIO46, control registers over the I2C0 bus
        // shared with the RTC above. Soft-fails (logs and leaves `audio` as
        // `None`) rather than aborting board bring-up, since this hardware
        // path is unverified and the rest of the device should stay usable
        // even if the codec doesn't come up.
        let pa_enable = PinDriver::output(pins.gpio46)?;
        let audio = match I2sDriver::<I2sTx>::new_std_tx(
            peripherals.i2s0,
            &audio::i2s_std_config(),
            pins.gpio15,       // BCLK
            pins.gpio45,       // DOUT
            Some(pins.gpio14), // MCLK
            pins.gpio38,       // WS/LRCK
        ) {
            Ok(i2s) => match Es8311::new(i2c_bus.clone(), audio::ES8311_ADDR, i2s, pa_enable) {
                Ok(codec) => Some(codec),
                Err(err) => {
                    log::warn!("ES8311 init failed: {err}");
                    None
                }
            },
            Err(err) => {
                log::warn!("I2S0 TX channel setup failed: {err}");
                None
            }
        };

        // GT23SC6699 NFC tag: power on GPIO21, field-detect on GPIO7,
        // control over the same shared I2C0 bus. Soft-fails like `audio`
        // above.
        let nfc_power = PinDriver::output(pins.gpio21)?;
        let nfc_fd = PinDriver::input(pins.gpio7, Pull::Up)?;
        let nfc = match NfcTag::new(i2c_bus.clone(), nfc::GT23SC6699_ADDR, nfc_power, nfc_fd) {
            Ok(tag) => Some(tag),
            Err(err) => {
                log::warn!("NFC init failed: {err}");
                None
            }
        };

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
            audio,
            nfc,
        })
    }

    pub fn charging_state(&self) -> (bool, bool) {
        (self.charging.is_low(), self.charge_done.is_high())
    }

    /// Drives the status LED (GPIO3) from the charge-management IC's own
    /// pins: lit while actively charging, off once charge is complete or
    /// when not on power at all. Previously the LED was set high once at
    /// boot and never touched again (a leftover from before the UI
    /// rewrite removed the old heartbeat-blink behavior); this gives it a
    /// purpose again without adding a timer/blink pattern - a single
    /// `PinDriver::set_high`/`set_low` call per `report_power_state` poll
    /// is enough for a binary "charging or not" signal.
    pub fn update_charging_led(&mut self) -> Result<()> {
        let (charging, _charge_done) = self.charging_state();
        if charging {
            self.led.set_high()?;
        } else {
            self.led.set_low()?;
        }
        Ok(())
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

/// Rough single-cell LiPo state-of-charge estimate from voltage, linear
/// between the common 3300mV "empty" cutoff and 4200mV "full charge". Real
/// LiPo discharge curves aren't linear, but there's no fuel-gauge IC on
/// this board - this is a coarse glance indicator for the Home screen, not
/// a precise one.
pub fn battery_percent_from_mv(mv: u16) -> u8 {
    const EMPTY_MV: u32 = 3300;
    const FULL_MV: u32 = 4200;
    let mv = mv as u32;
    if mv <= EMPTY_MV {
        0
    } else if mv >= FULL_MV {
        100
    } else {
        (((mv - EMPTY_MV) * 100) / (FULL_MV - EMPTY_MV)) as u8
    }
}
