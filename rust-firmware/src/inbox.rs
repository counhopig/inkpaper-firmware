//! Inbox store, backed by one NVS blob - same shape as `todos.rs` but for
//! notifications delivered from external sources (webhook/CalDAV) via the
//! server. The device only browses, marks read, and uploads read `seq`s back;
//! it never authors inbox content.
//!
//! Two pieces of state are kept separately so a power cut between "user
//! opened a message" and "server acked the read" can't lose the pending read:
//! - the authoritative `InboxItem` list (from the server),
//! - a set of `seq`s locally marked read but not yet acknowledged by the
//!   server. They are only removed once `inbox_read_acked` echoes them back.

use anyhow::{anyhow, Result};
use esp_idf_svc::nvs::{EspDefaultNvs, EspDefaultNvsPartition};
use serde::{Deserialize, Serialize};

const NAMESPACE: &str = "inkpaper_inbox";
const KEY_ITEMS: &str = "items";
const KEY_PENDING: &str = "pending";
const BLOB_BUF_LEN: usize = 4096;

/// Inbox notification kind, wire-compatible with the server's `InboxKind`
/// (`"alert"`/`"event"`/`"info"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxKind {
    Alert,
    Event,
    Info,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InboxItem {
    /// Device-visible stable id (server `seq`, monotonic).
    pub id: u64,
    pub kind: InboxKind,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub when: Option<i64>,
    #[serde(default)]
    pub read: bool,
}

/// Maximum number of items kept locally; anything beyond this is dropped on
/// save so a large server inbox can't overflow NVS.
pub const MAX_ITEMS: usize = 32;

pub struct InboxStore {
    nvs: EspDefaultNvs,
}

impl InboxStore {
    /// `partition` must be a clone of the one shared `EspDefaultNvsPartition`
    /// handle `main.rs` takes once - see the doc comment on
    /// `storage::PersistedCounters::open`.
    pub fn open(partition: EspDefaultNvsPartition) -> Result<Self> {
        let nvs = EspDefaultNvs::new(partition, NAMESPACE, true)
            .map_err(|e| anyhow!("failed to open NVS namespace '{NAMESPACE}': {e}"))?;
        Ok(Self { nvs })
    }

    fn read_blob<T: for<'de> Deserialize<'de>>(&self, key: &str) -> Result<Option<T>> {
        let mut buf = [0u8; BLOB_BUF_LEN];
        let bytes = self
            .nvs
            .get_blob(key, &mut buf)
            .map_err(|e| anyhow!("NVS get_blob({key}) failed: {e}"))?;
        match bytes {
            Some(bytes) => serde_json::from_slice(bytes)
                .map(Some)
                .map_err(|e| anyhow!("{key} JSON decode failed: {e}")),
            None => Ok(None),
        }
    }

    fn write_blob<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let bytes =
            serde_json::to_vec(value).map_err(|e| anyhow!("{key} JSON encode failed: {e}"))?;
        if bytes.len() > BLOB_BUF_LEN {
            return Err(anyhow!(
                "{key} blob too large: {} bytes (max {BLOB_BUF_LEN})",
                bytes.len()
            ));
        }
        self.nvs
            .set_blob(key, &bytes)
            .map_err(|e| anyhow!("NVS set_blob({key}) failed: {e}"))
    }

    /// The authoritative inbox list, empty if never synced.
    pub fn load(&self) -> Result<Vec<InboxItem>> {
        Ok(self.read_blob(KEY_ITEMS)?.unwrap_or_default())
    }

    /// Replaces the inbox with the server's authoritative list. Any locally
    /// pending-read `seq`s that are now `read` (or no longer present) are
    /// dropped; the rest are preserved so they can still be acked later.
    pub fn save(&self, items: &[InboxItem]) -> Result<()> {
        let pending = self.pending_read()?;
        let pending: Vec<u64> = pending
            .into_iter()
            .filter(|seq| items.iter().any(|it| it.id == *seq && !it.read))
            .collect();
        let mut owned: Vec<InboxItem> = items.to_vec();
        owned.truncate(MAX_ITEMS);
        // Preserve the locally-read flag for anything the server still sends
        // back as unread but that we've already opened this session.
        for item in owned.iter_mut() {
            if pending.contains(&item.id) {
                item.read = true;
            }
        }
        self.write_blob(KEY_ITEMS, &owned)?;
        self.write_blob(KEY_PENDING, &pending)
    }

    /// `seq`s locally read but not yet acked by the server.
    pub fn pending_read(&self) -> Result<Vec<u64>> {
        Ok(self.read_blob(KEY_PENDING)?.unwrap_or_default())
    }

    /// Marks `seq` read locally and adds it to the pending-read set (unless
    /// already there). Used when the user opens a message on-device.
    pub fn mark_read(&self, seq: u64) -> Result<()> {
        let mut items = self.load()?;
        let mut pending = self.pending_read()?;
        if let Some(item) = items.iter_mut().find(|it| it.id == seq) {
            item.read = true;
        }
        if !pending.contains(&seq) {
            pending.push(seq);
        }
        self.write_blob(KEY_ITEMS, &items)?;
        self.write_blob(KEY_PENDING, &pending)
    }

    /// Removes `seq`s from the pending-read set once the server acknowledges
    /// them (`inbox_read_acked`).
    pub fn ack_read(&self, acked: &[u64]) -> Result<()> {
        let pending = self.pending_read()?;
        let remaining: Vec<u64> = pending
            .into_iter()
            .filter(|seq| !acked.contains(seq))
            .collect();
        self.write_blob(KEY_PENDING, &remaining)
    }

    /// Count of locally-unread items (for the home badge). `Alert` items that
    /// have been notified also count as unread until the user opens them.
    pub fn unread_count(&self) -> Result<usize> {
        Ok(self.load()?.iter().filter(|it| !it.read).count())
    }
}
