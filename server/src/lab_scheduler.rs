//! `LabScheduler` — manages parallel experiment slots and instrument locking.
//!
//! Up to `AXIOMLAB_EXPERIMENT_SLOTS` experiments (default 1, max 4) run
//! concurrently.  Each slot tracks which instruments it has reserved so that
//! two concurrent experiments never compete for the same physical device.
//!
//! # Backward compatibility
//! When `slot_count == 1` the scheduler behaves identically to the original
//! single-threaded loop: one experiment runs at a time with no contention checks.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// Manages a pool of concurrent experiment execution slots.
#[derive(Clone)]
pub struct LabScheduler {
    /// Maximum number of experiments that may run simultaneously.
    pub slot_count: usize,
    /// slot index → experiment id for currently-running experiments.
    active: Arc<Mutex<HashMap<usize, String>>>,
    /// Instrument names locked by at least one running experiment.
    locks: Arc<Mutex<HashSet<String>>>,
}

impl LabScheduler {
    /// Create a scheduler with an explicit slot count (clamped to [1, 4]).
    pub fn new(slot_count: usize) -> Self {
        let count = slot_count.clamp(1, 4);
        Self {
            slot_count: count,
            active: Arc::new(Mutex::new(HashMap::new())),
            locks:  Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Create a scheduler from the `AXIOMLAB_EXPERIMENT_SLOTS` env var.
    ///
    /// Defaults to 1 (sequential) when the variable is absent or invalid.
    pub fn from_env() -> Self {
        let count = std::env::var("AXIOMLAB_EXPERIMENT_SLOTS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1);
        Self::new(count)
    }

    /// Number of slots not currently occupied.
    pub fn available_slots(&self) -> usize {
        self.slot_count
            .saturating_sub(self.active.lock().unwrap().len())
    }

    /// Whether all slots are in use.
    pub fn is_full(&self) -> bool {
        self.available_slots() == 0
    }

    /// Try to acquire a free slot for `experiment_id`, locking `instruments`.
    ///
    /// Returns the slot index on success.  Returns `None` when:
    /// - all `slot_count` slots are occupied, or
    /// - any instrument in `instruments` is already locked by another slot.
    pub fn try_acquire(&self, experiment_id: &str, instruments: &[&str]) -> Option<usize> {
        let mut active = self.active.lock().unwrap();
        let mut locks  = self.locks.lock().unwrap();

        if active.len() >= self.slot_count {
            return None;
        }
        if instruments.iter().any(|i| locks.contains(*i)) {
            return None;
        }

        let slot = (0..self.slot_count).find(|s| !active.contains_key(s))?;
        active.insert(slot, experiment_id.to_owned());
        for inst in instruments {
            locks.insert(inst.to_string());
        }
        Some(slot)
    }

    /// Release a slot and its associated instrument locks.
    pub fn release(&self, slot: usize, instruments: &[&str]) {
        let mut active = self.active.lock().unwrap();
        let mut locks  = self.locks.lock().unwrap();
        active.remove(&slot);
        for inst in instruments {
            locks.remove(*inst);
        }
    }

    /// Snapshot of active experiment ids (for observability).
    pub fn active_experiments(&self) -> Vec<String> {
        self.active.lock().unwrap().values().cloned().collect()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_slot_basic() {
        let s = LabScheduler::new(1);
        assert_eq!(s.available_slots(), 1);

        let slot = s.try_acquire("exp-1", &[]).unwrap();
        assert_eq!(slot, 0);
        assert_eq!(s.available_slots(), 0);
        assert!(s.is_full());

        // Cannot acquire a second slot when full.
        assert!(s.try_acquire("exp-2", &[]).is_none());

        s.release(slot, &[]);
        assert_eq!(s.available_slots(), 1);
    }

    #[test]
    fn two_slots_concurrent() {
        let s = LabScheduler::new(2);
        let a = s.try_acquire("exp-a", &[]).unwrap();
        let b = s.try_acquire("exp-b", &[]).unwrap();
        assert_ne!(a, b);
        assert!(s.is_full());

        s.release(a, &[]);
        assert_eq!(s.available_slots(), 1);
        s.release(b, &[]);
        assert_eq!(s.available_slots(), 2);
    }

    #[test]
    fn instrument_contention_blocks_slot() {
        let s = LabScheduler::new(2);
        // Acquire slot 0 with ph_meter locked.
        s.try_acquire("exp-1", &["ph_meter"]).unwrap();

        // Slot 1 is free but ph_meter is locked → acquisition fails.
        assert!(s.try_acquire("exp-2", &["ph_meter"]).is_none());

        // Different instrument → succeeds.
        assert!(s.try_acquire("exp-2", &["spectrophotometer"]).is_some());
    }

    #[test]
    fn release_frees_instrument_locks() {
        let s = LabScheduler::new(1);
        let slot = s.try_acquire("exp-1", &["ph_meter", "incubator"]).unwrap();
        s.release(slot, &["ph_meter", "incubator"]);

        // Both instruments are free again.
        let slot2 = s.try_acquire("exp-2", &["ph_meter"]).unwrap();
        assert_eq!(slot2, 0);
    }

    #[test]
    fn slot_count_clamped_to_4() {
        let s = LabScheduler::new(10);
        assert_eq!(s.slot_count, 4);
    }

    #[test]
    fn active_experiments_snapshot() {
        let s = LabScheduler::new(2);
        s.try_acquire("exp-alpha", &[]).unwrap();
        s.try_acquire("exp-beta", &[]).unwrap();
        let mut ids = s.active_experiments();
        ids.sort();
        assert_eq!(ids, vec!["exp-alpha", "exp-beta"]);
    }
}
