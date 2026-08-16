//! Todo-list store, backed by one NVS blob - same shape as `alarms.rs` but
//! with no RTC coupling.

use anyhow::{anyhow, Result};
use esp_idf_svc::nvs::{EspDefaultNvs, EspDefaultNvsPartition};
use serde::{Deserialize, Serialize};

const NAMESPACE: &str = "inkpaper_todo";
const KEY_TODOS: &str = "todos";
const BLOB_BUF_LEN: usize = 2048;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Todo {
    pub id: u8,
    pub text: String,
    pub done: bool,
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
}

/// Next unused id, so callers adding a todo don't have to track a counter
/// themselves - just `id: next_id(&todos)`.
pub fn next_id(todos: &[Todo]) -> u8 {
    todos.iter().map(|t| t.id).max().map_or(0, |m| m + 1)
}
