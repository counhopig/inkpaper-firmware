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
//! `board` and `wifi_mgr` need `&'a mut`. This lets a function read a store
//! and mutate the board in the same scope without fighting the borrow checker.

use crate::alarms::AlarmStore;
use crate::board::Note4Board;
use crate::inbox::InboxStore;
use crate::storage::PersistedCounters;
use crate::todos::TodoStore;
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
}
