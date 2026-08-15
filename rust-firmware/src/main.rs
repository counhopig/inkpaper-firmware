mod board;
mod display;

use std::thread;
use std::time::Duration;

use anyhow::Result;
use board::Note4Board;
use display::ButtonCounts;

fn main() -> Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    log::info!("Inkpaper NOTE4 Rust bring-up starting");
    let mut board = Note4Board::take()?;
    log::info!("Power latch is high; rendering Hello world");

    let mut counts = ButtonCounts {
        enter: 0,
        up: 0,
        down: 0,
    };
    board.display.render(&counts);
    board.display.refresh_full()?;
    log::info!("Initial display refresh completed");

    let mut led_on = false;
    let mut previous = board.state();
    log::info!(
        "Power state: charging={} charge_done={}",
        previous.charging,
        previous.charge_done
    );
    loop {
        led_on = !led_on;
        board.set_led(led_on)?;

        let state = board.state();
        let mut changed = false;
        if state.enter && !previous.enter {
            counts.enter += 1;
            changed = true;
            log::info!("ENTER pressed count={}", counts.enter);
        }
        if state.up && !previous.up {
            counts.up += 1;
            changed = true;
            log::info!("UP pressed count={}", counts.up);
        }
        if state.down && !previous.down {
            counts.down += 1;
            changed = true;
            log::info!("DOWN pressed count={}", counts.down);
        }
        previous = state;

        if changed {
            board.display.render(&counts);
            board.display.refresh_full()?;
            log::info!("Button display refresh completed");
            previous = board.state();
        }

        thread::sleep(Duration::from_millis(100));
    }
}
