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

use std::io::Read;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::control;

/// Prefix that marks an incoming line as a command, not a log message.
/// Must exactly match what the PC tool sends.
const COMMAND_PREFIX: &str = ">>IP ";

/// Prefix for outgoing replies, so the PC tool can distinguish control
/// responses from ordinary log output.
const REPLY_PREFIX: &str = "<<IP ";

/// Bounded channel size for queued commands. 16 commands should be plenty -
/// this is not meant to absorb burst traffic, just a small queue in case the
/// main loop is busy for a poll cycle or two while commands arrive.
const CHANNEL_CAPACITY: usize = 16;

/// Reader state: holds the receiving end of the command channel.
pub struct UsbConsole {
    rx: mpsc::Receiver<String>,
}

impl UsbConsole {
    /// Spawns a dedicated reader thread and returns a handle to poll commands
    /// from the main loop. The reader thread runs for the lifetime of the
    /// program (until the process exits).
    pub fn start() -> Self {
        let (tx, rx) = mpsc::sync_channel(CHANNEL_CAPACITY);

        // Pinned to the same core as the main task (Core0) rather than
        // left to the scheduler's default (either core). ESP32-S3's flash
        // cache has to be disabled on *both* cores during any flash/NVS
        // write; if this reader thread landed on the other core and
        // happened to be mid instruction-fetch from flash-mapped code at
        // that exact moment, it crashed with
        // `Guru Meditation Error: Cache error` at `_UserExceptionVector` -
        // reproduced on real hardware while chasing a seemingly unrelated
        // Wi-Fi bug (see `wifi::WifiManager`'s doc comment for that saga;
        // this thread being on the wrong core is suspected to be the
        // actual root cause behind at least some of those crashes, not
        // Wi-Fi reconnection itself). Reset to the default (unpinned)
        // config immediately after spawning so this doesn't affect any
        // other `thread::spawn` call elsewhere in the program.
        let pinned = esp_idf_svc::hal::task::thread::ThreadSpawnConfiguration {
            pin_to_core: Some(esp_idf_svc::hal::cpu::Core::Core0),
            ..Default::default()
        };
        if let Err(err) = pinned.set() {
            log::warn!("Failed to pin USB console reader thread to Core0: {err}");
        }

        thread::spawn(move || {
            let stdin = std::io::stdin();
            let mut handle = stdin.lock();
            // Accumulated as raw bytes, not `String`, and only decoded once a
            // full line is seen: this port's console fd is non-blocking (see
            // the `WouldBlock` arm below), so a single `read()` can return in
            // the middle of a multi-byte UTF-8 codepoint (e.g. non-ASCII todo
            // text from a future PC tool) - decoding byte-by-byte would panic
            // or mangle it.
            let mut line_buf: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 64];
            loop {
                match handle.read(&mut chunk) {
                    Ok(0) => {
                        // EOF on a live console shouldn't happen; back off
                        // instead of spinning.
                        thread::sleep(Duration::from_millis(50));
                    }
                    Ok(n) => {
                        for &b in &chunk[..n] {
                            if b == b'\n' {
                                let line = String::from_utf8_lossy(&line_buf)
                                    .trim_end_matches('\r')
                                    .to_string();
                                line_buf.clear();
                                if let Some(cmd) = line.strip_prefix(COMMAND_PREFIX) {
                                    // Ignore send errors (would only happen if
                                    // the channel is disconnected, which
                                    // shouldn't occur during normal operation).
                                    let _ = tx.send(cmd.to_string());
                                }
                                // Lines without our prefix are just log output
                                // or other noise sharing this port - ignore.
                            } else {
                                line_buf.push(b);
                            }
                        }
                    }
                    // This console fd is non-blocking (`CONFIG_ESP_CONSOLE_
                    // USB_SERIAL_JTAG`'s VFS driver, not a plain blocking
                    // POSIX tty): "no data right now" surfaces as EAGAIN, not
                    // as `read()` blocking until a byte arrives. Treating
                    // this as a loggable error (the original implementation
                    // did) busy-spins the thread and floods the log at
                    // whatever rate the poll loop can achieve - a real bug
                    // caught only by testing on actual hardware, not by a
                    // clean `cargo build`. Back off quietly instead.
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(err) => {
                        log::warn!("USB console reader I/O error: {err}");
                        thread::sleep(Duration::from_millis(100));
                    }
                }
            }
        });

        if let Err(err) = esp_idf_svc::hal::task::thread::ThreadSpawnConfiguration::default().set()
        {
            log::warn!("Failed to reset thread spawn configuration after pinning: {err}");
        }

        Self { rx }
    }

    /// Non-blocking poll for the next command. Returns `Some(Command)` if a
    /// complete command line was received and parsed successfully, or `None`
    /// if there are no queued commands, or if a line was queued but failed to
    /// parse (in which case a warning is logged).
    pub fn poll_command(&self) -> Option<control::Command> {
        match self.rx.try_recv() {
            Ok(line) => match control::parse_command(&line) {
                Ok(cmd) => Some(cmd),
                Err(err) => {
                    log::warn!("USB console: failed to parse command '{}': {err}", line);
                    None
                }
            },
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                log::warn!("USB console: command channel disconnected");
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
