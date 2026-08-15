use anyhow::Result;
use esp_idf_svc::hal::gpio::{Input, Output, PinDriver, Pull};
use esp_idf_svc::hal::peripherals::Peripherals;

use crate::button::Button;
use crate::display::EpdDisplay;

pub struct Note4Board {
    _power_latch: PinDriver<'static, Output>,
    led: PinDriver<'static, Output>,
    _avdd_power: PinDriver<'static, Output>,
    pub key_enter: Button,
    pub key_up: Button,
    pub key_down: Button,
    charging: PinDriver<'static, Input>,
    charge_done: PinDriver<'static, Input>,
    pub display: EpdDisplay,
}

impl Note4Board {
    pub fn take() -> Result<Self> {
        let peripherals = Peripherals::take()?;
        let pins = peripherals.pins;

        let mut power_latch = PinDriver::output(pins.gpio17)?;
        power_latch.set_high()?;

        let mut led = PinDriver::output(pins.gpio3)?;
        led.set_high()?;

        let mut avdd_power = PinDriver::output(pins.gpio42)?;
        avdd_power.set_low()?;

        let key_enter = Button::new(pins.gpio0.into(), Pull::Up)?;
        let key_up = Button::new(pins.gpio39.into(), Pull::Up)?;
        let key_down = Button::new(pins.gpio18.into(), Pull::Up)?;
        let charging = PinDriver::input(pins.gpio2, Pull::Up)?;
        let charge_done = PinDriver::input(pins.gpio1, Pull::Floating)?;
        let display = EpdDisplay::new()?;

        Ok(Self {
            _power_latch: power_latch,
            led,
            _avdd_power: avdd_power,
            key_enter,
            key_up,
            key_down,
            charging,
            charge_done,
            display,
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
}
