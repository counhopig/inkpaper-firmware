use anyhow::Result;
use esp_idf_svc::hal::gpio::{Input, Output, PinDriver, Pull};
use esp_idf_svc::hal::peripherals::Peripherals;

use crate::display::EpdDisplay;

pub struct Note4Board {
    _power_latch: PinDriver<'static, Output>,
    led: PinDriver<'static, Output>,
    _avdd_power: PinDriver<'static, Output>,
    key_enter: PinDriver<'static, Input>,
    key_up: PinDriver<'static, Input>,
    key_down: PinDriver<'static, Input>,
    charging: PinDriver<'static, Input>,
    charge_done: PinDriver<'static, Input>,
    pub display: EpdDisplay,
}

#[derive(Clone, Copy, Debug)]
pub struct BoardState {
    pub enter: bool,
    pub up: bool,
    pub down: bool,
    pub charging: bool,
    pub charge_done: bool,
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

        let key_enter = PinDriver::input(pins.gpio0, Pull::Up)?;
        let key_up = PinDriver::input(pins.gpio39, Pull::Up)?;
        let key_down = PinDriver::input(pins.gpio18, Pull::Up)?;
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

    pub fn state(&self) -> BoardState {
        BoardState {
            enter: self.key_enter.is_low(),
            up: self.key_up.is_low(),
            down: self.key_down.is_low(),
            charging: self.charging.is_low(),
            charge_done: self.charge_done.is_high(),
        }
    }
}
