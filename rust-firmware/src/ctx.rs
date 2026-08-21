//! DeviceContext: a single bundle of the firmware's long-lived shared state,
//! so screen/sync/control functions pass one `&mut DeviceContext` instead of
//! threading eight individual arguments through every call site. `clock` and
//! `ble_control` are intentionally NOT here: `clock` is a transient value
//! re-read from the RTC, and `ble_control` has a distinct lifetime (owned by
//! the main loop and torn down when leaving the pairing screen), so they stay
//! as explicit parameters where they're needed.
//!
//! The store fields are `&'a` (immutable) because their methods all take
//! `&self` (the underlying NVS handles have internal mutability); only
//! `board`, `wifi_mgr`, and `usb_console` need `&'a mut`. This lets a function read a store
//! and mutate the board in the same scope without fighting the borrow checker.

use crate::alarms::AlarmStore;
use crate::board::Note4Board;
use crate::control::{self, Command, Reply};
use crate::inbox::InboxStore;
use crate::rtc::DateTime;
use crate::storage::PersistedCounters;
use crate::todos::TodoStore;
use crate::usb_console::UsbConsole;
use crate::wifi::WifiManager;

/// All of the firmware's shared, long-lived state, created once in `main()`
/// and threaded through the UI/sync/control layers by reference.
pub struct DeviceContext<'a> {
    pub board: &'a mut Note4Board,
    pub counters: &'a PersistedCounters,
    pub wifi_mgr: &'a mut WifiManager,
    pub alarm_store: &'a AlarmStore,
    pub todo_store: &'a TodoStore,
    pub inbox_store: &'a InboxStore,
    pub usb_console: &'a mut UsbConsole,
}

impl DeviceContext<'_> {
    /// Services one queued USB command from any UI loop. Returns true when a
    /// successful command may have changed visible device state and the
    /// current screen should redraw from its stores/RTC.
    pub fn poll_usb_control(&mut self, now: Option<&DateTime>) -> bool {
        let Some(cmd) = self.usb_console.poll_command() else {
            return false;
        };
        let changes_visible_state = !matches!(cmd, Command::GetStatus);
        let fresh_now = self.board.rtc.read_time().ok();
        let reply = control::dispatch(self, cmd, fresh_now.as_ref().or(now));
        crate::usb_console::write_reply(&reply);
        changes_visible_state && matches!(reply, Reply::Ok)
    }
}
