//! Inbox item data shape, moved out of `rust-firmware/src/inbox.rs` (which
//! re-exports it) so `sync_validate` can share one definition instead of
//! duplicating the wire shape.

use serde::{Deserialize, Serialize};

/// Inbox notification kind, wire-compatible with the server's `InboxKind`
/// (`"alert"`/`"event"`/`"info"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboxKind {
    Alert,
    Event,
    Info,
}

/// Inbox notification priority, wire-compatible with the server's `Priority`
/// (`"normal"`/`"high"`). `High` alerts trigger an urgent full-screen
/// reminder with an insistent tone; the sync client long-polls for them so
/// they surface in real time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    #[default]
    Normal,
    High,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InboxItem {
    /// Device-visible stable id (server `seq`, monotonic).
    pub id: u64,
    pub kind: InboxKind,
    #[serde(default)]
    pub priority: Priority,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub when: Option<i64>,
    #[serde(default)]
    pub read: bool,
}
