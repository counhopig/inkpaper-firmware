//! Non-blocking USB serial console command reader for the shared
//! `CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG` port.
//!
//! Commands and log output share the same USB serial port. To avoid colliding
//! with `log::info!`/`log::warn!` output, commands are framed with a sentinel
//! prefix `>>IP ` (note the trailing space). Each call to `poll_command()`
//! does a single non-blocking `read()` of whatever bytes are currently
//! available, splits them into `\n`-terminated lines, and parses every line
//! that starts with `>>IP ` into a `Command`. Since one read can contain more
//! than one complete command (e.g. two commands sent back-to-back land in the
//! same read), parsed commands are queued in `pending` and drained one per
//! `poll_command()` call — the whole read buffer is always fully consumed,
//! so no command is ever silently dropped because it shared a read with
//! another one.
//!
//! Replies are written back to stdout with the `<<IP ` prefix to distinguish
//! them as control output (not ordinary logs).
//!
//! This design avoids the complexity of raw fcntl/termios manipulation on the
//! USB-Serial-JTAG peripheral and leverages Rust's std threads, which work
//! reliably on this FreeRTOS+std platform.

use crate::control;
use std::collections::VecDeque;
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
    /// Commands already parsed out of a previous read but not yet returned
    /// to the caller. A single 128-byte read can contain more than one
    /// complete `>>IP ...\n` line; every one of them is parsed here so none
    /// are lost, and `poll_command()` hands them out one at a time.
    pending: VecDeque<control::Command>,
}

impl UsbConsole {
    pub fn start() -> Self {
        Self {
            line_buf: Vec::new(),
            pending: VecDeque::new(),
        }
    }

    /// Non-blocking poll for the next command. Returns `Some(Command)` if a
    /// complete command line was received and parsed successfully, or `None`
    /// if there are no queued commands, or if a line was queued but failed to
    /// parse (in which case a warning is logged).
    pub fn poll_command(&mut self) -> Option<control::Command> {
        if let Some(cmd) = self.pending.pop_front() {
            return Some(cmd);
        }

        let mut chunk = [0u8; 128];
        match std::io::stdin().read(&mut chunk) {
            Ok(n) => {
                // Always consume the whole chunk - a single read can contain
                // more than one complete command line, and stopping early
                // would silently drop whatever came after the first one.
                for &byte in &chunk[..n] {
                    if byte == b'\n' {
                        let line = String::from_utf8_lossy(&self.line_buf)
                            .trim_end_matches('\r')
                            .to_string();
                        self.line_buf.clear();
                        if let Some(json) = line.strip_prefix(COMMAND_PREFIX) {
                            match control::parse_command(json) {
                                Ok(cmd) => self.pending.push_back(cmd),
                                Err(err) => {
                                    log::warn!("USB console: failed to parse command: {err}");
                                }
                            }
                        }
                    } else {
                        self.line_buf.push(byte);
                    }
                }
                self.pending.pop_front()
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
