//! Wall-clock boundary-alignment math shared by `rust-firmware/src/ctx.rs`'s
//! `SyncScheduler` (urgent-poll and full-sync cadences) and `main.rs`'s
//! status/clock poll counters. "Cron-style" alignment means a boundary is
//! due when its index *advances* relative to the last one observed, not
//! when a boot-relative timer elapses - so "every 30s" fires at :00/:30 of
//! each minute and "every 1h" fires at the top of the hour, regardless of
//! what wall-clock time the device happened to boot at.

/// The boundary index that `unix` falls into for a period of `period_secs`
/// seconds - e.g. `boundary_index(unix, 30)` for the urgent-poll cadence, or
/// `boundary_index(unix, interval_minutes * 60)` for the full-sync cadence.
pub fn boundary_index(unix: u64, period_secs: u64) -> u64 {
    unix / period_secs
}

/// Whether the boundary `unix` falls into has advanced past `last_seen` (the
/// index recorded the last time this boundary fired). A caller should
/// record the new `boundary_index(unix, period_secs)` after acting on
/// `true`, mirroring `ctx.rs::poll_scheduled_sync`.
pub fn boundary_advanced(unix: u64, period_secs: u64, last_seen: u64) -> bool {
    boundary_index(unix, period_secs) != last_seen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_index_groups_seconds_within_one_period() {
        assert_eq!(boundary_index(0, 30), 0);
        assert_eq!(boundary_index(29, 30), 0);
        assert_eq!(boundary_index(30, 30), 1);
        assert_eq!(boundary_index(59, 30), 1);
        assert_eq!(boundary_index(60, 30), 2);
    }

    #[test]
    fn boundary_advanced_is_false_within_the_same_period() {
        // 100 falls in period index 3 (covers unix 90..=119).
        let last_seen = boundary_index(100, 30);
        assert!(!boundary_advanced(101, 30, last_seen));
        assert!(!boundary_advanced(119, 30, last_seen));
    }

    #[test]
    fn boundary_advanced_is_true_once_the_index_changes() {
        let last_seen = boundary_index(100, 30);
        assert!(boundary_advanced(130, 30, last_seen));
    }

    #[test]
    fn hourly_boundary_aligns_to_wall_clock_not_boot_time() {
        // Two devices booting at different times of the hour should both
        // fire "every 1h" at the same wall-clock top-of-hour, not offset by
        // their own boot time. 3600 = 1h; unix=3599 and unix=1 are in
        // different hourly boundaries from unix=3600's perspective, but
        // unix=1 and unix=3599 share hour-boundary 0.
        assert_eq!(boundary_index(1, 3600), boundary_index(3599, 3600));
        assert_ne!(boundary_index(3599, 3600), boundary_index(3600, 3600));
    }

    #[test]
    fn a_never_seen_boundary_index_of_zero_is_not_special_cased() {
        // Regression guard: `last_seen = 0` (e.g. a freshly constructed
        // scheduler) must still report "advanced" once the real boundary
        // index is anything other than 0, not be mistaken for "already
        // fired boundary 0".
        assert!(boundary_advanced(3600, 3600, 0));
        assert!(!boundary_advanced(0, 3600, 0));
    }
}
