//! Todo-list store, backed by one NVS blob - same shape as `alarms.rs` but
//! with no RTC coupling.

use anyhow::{anyhow, Result};
use esp_idf_svc::nvs::{EspDefaultNvs, EspDefaultNvsPartition};

/// `Importance`/`TodoDue`/`Todo` live in `inkwash-logic` (re-exported here)
/// so `sync_validate`'s host tests share the exact same wire shape instead
/// of a hand-copied one that could drift - see "Remaining engineering work"
/// #1 in `docs/remaining-work.md`.
#[allow(unused_imports)] // TodoDue: part of Todo's public shape; no on-device code names it directly (due dates are server-authored, never constructed on-device).
pub use inkwash_logic::todo::{Importance, Todo, TodoDue};

const NAMESPACE: &str = "inkwash_todo";
const KEY_TODOS: &str = "todos";
/// Locally-changed `local_id`s pending upload (two-way sync dirty set).
const KEY_DIRTY: &str = "dirty";
const BLOB_BUF_LEN: usize = 2048;

pub struct TodoStore {
    nvs: EspDefaultNvs,
}

impl TodoStore {
    /// `partition` must be a clone of the one shared `EspDefaultNvsPartition`
    /// handle `main.rs` takes once - see the doc comment on
    /// `storage::PersistedCounters::open` for why a second independent
    /// `EspDefaultNvsPartition::take()` here would fail at boot.
    pub fn open(partition: EspDefaultNvsPartition) -> Result<Self> {
        let nvs = EspDefaultNvs::new(partition, NAMESPACE, true)
            .map_err(|e| anyhow!("failed to open NVS namespace '{NAMESPACE}': {e}"))?;
        Ok(Self { nvs })
    }

    /// Empty list if nothing has been saved yet.
    pub fn load(&self) -> Result<Vec<Todo>> {
        let mut buf = [0u8; BLOB_BUF_LEN];
        let bytes = self
            .nvs
            .get_blob(KEY_TODOS, &mut buf)
            .map_err(|e| anyhow!("NVS get_blob({KEY_TODOS}) failed: {e}"))?;
        match bytes {
            Some(bytes) => {
                serde_json::from_slice(bytes).map_err(|e| anyhow!("todos JSON decode failed: {e}"))
            }
            None => Ok(Vec::new()),
        }
    }

    pub fn save(&self, todos: &[Todo]) -> Result<()> {
        let bytes =
            serde_json::to_vec(todos).map_err(|e| anyhow!("todos JSON encode failed: {e}"))?;
        if bytes.len() > BLOB_BUF_LEN {
            return Err(anyhow!(
                "todos blob too large: {} bytes (max {BLOB_BUF_LEN})",
                bytes.len()
            ));
        }
        self.nvs
            .set_blob(KEY_TODOS, &bytes)
            .map_err(|e| anyhow!("NVS set_blob({KEY_TODOS}) failed: {e}"))
    }

    // --- Two-way sync dirty tracking -------------------------------------
    //
    // The device uploads only `local_id`s that changed *locally* since the
    // last successful sync, so a `done`/`importance` edit made on the
    // Server/Desktop side is not clobbered by the device's stale copy on
    // the next sync. The set is cleared only after a successful sync.

    /// Marks `id` as locally changed (done flag and/or importance) and
    /// pending upload.
    pub fn mark_dirty(&self, id: u8) -> Result<()> {
        let mut dirty = self.dirty_ids()?;
        if !dirty.contains(&id) {
            dirty.push(id);
        }
        let bytes =
            serde_json::to_vec(&dirty).map_err(|e| anyhow!("dirty JSON encode failed: {e}"))?;
        self.nvs
            .set_blob(KEY_DIRTY, &bytes)
            .map_err(|e| anyhow!("NVS set_blob({KEY_DIRTY}) failed: {e}"))
    }

    /// `local_id`s changed locally since the last successful sync.
    pub fn dirty_ids(&self) -> Result<Vec<u8>> {
        let mut buf = [0u8; BLOB_BUF_LEN];
        let bytes = self
            .nvs
            .get_blob(KEY_DIRTY, &mut buf)
            .map_err(|e| anyhow!("NVS get_blob({KEY_DIRTY}) failed: {e}"))?;
        match bytes {
            Some(bytes) => {
                serde_json::from_slice(bytes).map_err(|e| anyhow!("dirty JSON decode failed: {e}"))
            }
            None => Ok(Vec::new()),
        }
    }

    /// Drops the dirty set after a successful sync.
    pub fn clear_dirty(&self) -> Result<()> {
        self.nvs
            .remove(KEY_DIRTY)
            .map(|_| ())
            .map_err(|e| anyhow!("NVS remove({KEY_DIRTY}) failed: {e}"))
    }
}
