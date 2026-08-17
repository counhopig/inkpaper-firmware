use anyhow::Result;
use esp_idf_svc::hal::gpio::{AnyIOPin, Input, PinDriver, Pull};

pub const POLL_INTERVAL_MS: u32 = 20;
const DEBOUNCE_SAMPLES: u32 = 4;
const LONG_PRESS_POLLS: u32 = 50;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ButtonEvent {
    Pressed,
    Released,
    LongPressed,
}

pub struct Button {
    pin: PinDriver<'static, Input>,
    debounced: bool,
    candidate: bool,
    samples: u32,
    held_polls: u32,
    long_pressed: bool,
}

impl Button {
    pub fn new(pin: AnyIOPin<'static>, pull: Pull) -> Result<Self> {
        let pin = PinDriver::input(pin, pull)?;
        let initial = pin.is_low();
        Ok(Self {
            pin,
            debounced: initial,
            candidate: initial,
            samples: DEBOUNCE_SAMPLES,
            held_polls: 0,
            long_pressed: false,
        })
    }

    pub fn poll(&mut self) -> Option<ButtonEvent> {
        let raw = self.pin.is_low();
        let mut event = None;
        if raw == self.candidate {
            self.samples += 1;
            if self.samples >= DEBOUNCE_SAMPLES {
                self.samples = DEBOUNCE_SAMPLES;
                if self.debounced != self.candidate {
                    self.debounced = self.candidate;
                    if self.debounced {
                        self.held_polls = 0;
                        self.long_pressed = false;
                    } else if self.long_pressed {
                        event = Some(ButtonEvent::Released);
                    } else {
                        // Emit a short press on release. Otherwise every
                        // long press would trigger the short action first.
                        event = Some(ButtonEvent::Pressed);
                    }
                } else if self.debounced {
                    self.held_polls += 1;
                    if self.held_polls == LONG_PRESS_POLLS {
                        self.long_pressed = true;
                        event = Some(ButtonEvent::LongPressed);
                    }
                }
            }
        } else {
            self.candidate = raw;
            self.samples = 0;
        }
        event
    }
}
