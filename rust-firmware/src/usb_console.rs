//! Non-blocking USB serial console command reader for the shared
//! `CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG` port.
//!
//! Commands and log output share the same USB serial port. To avoid colliding
//! with `log::info!`/`log::warn!` output, commands are framed with a sentinel
//! prefix `>>IP ` (note the trailing space). The reader thread continuously
//! reads lines from stdin, filters for those starting with `>>IP `, and pushes
//! them (without the prefix) into a bounded sync_channel. The main loop polls
//! this channel non-blockingly each iteration with `try_recv()` and dispatches
//! any commands.
//!
//! Replies are written back to stdout with the `<<IP ` prefix to distinguish
//! them as control output (not ordinary logs).
//!
//! This design avoids the complexity of raw fcntl/termios manipulation on the
//! USB-Serial-JTAG peripheral and leverages Rust's std threads, which work
//! reliably on this FreeRTOS+std platform.

use crate::control;
use std::io::Read;

/// Prefix that marks an incoming line as a command, not a log message.
/// Must exactly match what the PC tool sends.
const COMMAND_PREFIX: &str = ">>IP ";

/// Prefix for outgoing replies, so the PC tool can distinguish control
/// responses from ordinary log output.
const REPLY_PREFIX: &str = "<<IP ";

/// Reader state. USB Serial/JTAG stdin is non-blocking, so the main loop can
/// drain it directly without a second FreeRTOS task competing with Wi-Fi on
/// Core 0.
pub struct UsbConsole {
    line_buf: Vec<u8>,
}

impl UsbConsole {
    pub fn start() -> Self {
        Self {
            line_buf: Vec::new(),
        }
    }

    /// Non-blocking poll for the next command. Returns `Some(Command)` if a
    /// complete command line was received and parsed successfully, or `None`
    /// if there are no queued commands, or if a line was queued but failed to
    /// parse (in which case a warning is logged).
    pub fn poll_command(&mut self) -> Option<control::Command> {
        let mut chunk = [0u8; 128];
        match std::io::stdin().read(&mut chunk) {
            Ok(n) => {
                for &byte in &chunk[..n] {
                    if byte == b'\n' {
                        let line = String::from_utf8_lossy(&self.line_buf)
                            .trim_end_matches('\r')
                            .to_string();
                        self.line_buf.clear();
                        if let Some(json) = line.strip_prefix(COMMAND_PREFIX) {
                            return match control::parse_command(json) {
                                Ok(cmd) => Some(cmd),
                                Err(err) => {
                                    log::warn!("USB console: failed to parse command: {err}");
                                    None
                                }
                            };
                        }
                    } else {
                        self.line_buf.push(byte);
                    }
                }
                None
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => None,
            Err(err) => {
                log::warn!("USB console reader I/O error: {err}");
                None
            }
        }
    }
}

/// Write a reply back to the PC tool, framed with the `<<IP ` prefix.
/// This should be called from the main loop after dispatching a command.
pub fn write_reply(reply: &control::Reply) {
    let json = control::render_reply(reply);
    println!("{REPLY_PREFIX}{json}");
}
