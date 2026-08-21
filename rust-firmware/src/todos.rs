//! Todo-list store, backed by one NVS blob - same shape as `alarms.rs` but
//! with no RTC coupling.

use anyhow::{anyhow, Result};
use esp_idf_svc::nvs::{EspDefaultNvs, EspDefaultNvsPartition};
use serde::{Deserialize, Serialize};

use crate::alarms::Repeat;

const NAMESPACE: &str = "inkpaper_todo";
const KEY_TODOS: &str = "todos";
/// Locally-changed `local_id`s pending upload (two-way sync dirty set).
const KEY_DIRTY: &str = "dirty";
const BLOB_BUF_LEN: usize = 2048;

/// Todo importance, wire-compatible with the server's `models::Importance`
/// (`"low"`/`"medium"`/`"high"`). `Medium` is the default so records synced
/// from before importance existed keep a sane value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Importance {
    Low,
    #[default]
    Medium,
    High,
}

/// Full due date (year/month/day). The calendar page draws a marker on that
/// date, and a `High` todo due today triggers a one-shot reminder in
/// `main.rs`. The `year` field defaults to 0 so records synced before it
/// existed deserialize safely (year 0 never matches a real date, so such
/// todos simply don't mark the calendar or remind).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoDue {
    #[serde(default)]
    pub year: u16,
    pub month: u8,
    pub day: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Todo {
    pub id: u8,
    pub text: String,
    pub done: bool,
    #[serde(default)]
    pub importance: Importance,
    /// Single due date (used when `repeat` is `None`).
    #[serde(default)]
    pub due_date: Option<TodoDue>,
    /// Recurrence schedule; when set, the todo is due on every date the
    /// schedule covers instead of just `due_date`.
    #[serde(default)]
    pub repeat: Option<Repeat>,
}

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
