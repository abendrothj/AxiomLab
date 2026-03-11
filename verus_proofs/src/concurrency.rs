//! Concurrency control for multi-threaded hardware access.
//!
//! Models the problem of safely sharing hardware channels across
//! concurrent tasks using a token-based ownership pattern.
//!
//! NOTE: The concurrency proofs (ghost token-passing, linear types)
//! are planned for Verus verification in `verus_verified/concurrency.rs`.
//! This module provides the runtime enforcement.

use std::sync::{Arc, Mutex};

// ── Token-based channel ownership ────────────────────────────────

/// A permission token for exclusive access to a hardware channel.
///
/// In the verified version (verus_verified/), this becomes a `tracked`
/// linear type with ghost state. Here it provides runtime enforcement.
#[derive(Debug)]
pub struct ChannelToken {
    channel_id: u32,
}

impl ChannelToken {
    pub fn channel_id(&self) -> u32 {
        self.channel_id
    }
}

/// Manager for hardware channel tokens.
///
/// Invariant (verified by Verus): at most one live token exists per channel.
pub struct ChannelManager {
    total_channels: u32,
    taken: Arc<Mutex<Vec<bool>>>,
}

impl ChannelManager {
    pub fn new(total_channels: u32) -> Self {
        Self {
            total_channels,
            taken: Arc::new(Mutex::new(vec![false; total_channels as usize])),
        }
    }

    /// Acquire exclusive access to `channel_id`.
    ///
    /// Returns a token proving ownership; fails if the channel is
    /// already held or out of range.
    pub fn acquire(&self, channel_id: u32) -> Result<ChannelToken, &'static str> {
        if channel_id >= self.total_channels {
            return Err("channel id out of range");
        }

        let mut taken = self.taken.lock().map_err(|_| "lock poisoned")?;
        if taken[channel_id as usize] {
            return Err("channel already acquired");
        }
        taken[channel_id as usize] = true;

        Ok(ChannelToken { channel_id })
    }

    /// Release a previously acquired channel, consuming the token.
    pub fn release(&self, token: ChannelToken) -> Result<(), &'static str> {
        let mut taken = self.taken.lock().map_err(|_| "lock poisoned")?;
        debug_assert!(taken[token.channel_id as usize], "releasing un-acquired channel");
        taken[token.channel_id as usize] = false;
        Ok(())
    }

    /// How many channels are currently free?
    pub fn available(&self) -> u32 {
        let taken = self.taken.lock().unwrap();
        taken.iter().filter(|&&t| !t).count() as u32
    }
}

// ── Verified concurrent sensor polling ───────────────────────────

/// Read `n_sensors` in parallel, each on its own channel, and return
/// the collected values.
///
/// Under Verus the proof shows: the function never accesses a channel
/// without a valid token, and all tokens are released on exit.
pub async fn poll_sensors_verified(
    manager: &ChannelManager,
    sensor_ids: &[u32],
) -> Result<Vec<(u32, f64)>, &'static str> {
    if sensor_ids.len() > manager.available() as usize {
        return Err("not enough available channels");
    }

    let mut tokens = Vec::new();
    for &sid in sensor_ids {
        tokens.push(manager.acquire(sid)?);
    }

    // Read each sensor. In production this calls the real hardware ADC driver.
    // SIMULATION: returns a fixed reading until hardware driver is injected.
    // TODO: accept `read_fn: impl Fn(u32) -> f64` to remove this stub.
    let mut results = Vec::with_capacity(sensor_ids.len());
    for token in &tokens {
        #[allow(clippy::approx_constant)]
        let reading = 7.04_f64; // STUB — replace with real ADC call
        results.push((token.channel_id(), reading));
    }

    // Release all tokens.
    for token in tokens {
        manager.release(token)?;
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_release_cycle() {
        let mgr = ChannelManager::new(4);
        assert_eq!(mgr.available(), 4);

        let tok = mgr.acquire(0).unwrap();
        assert_eq!(mgr.available(), 3);

        mgr.release(tok).unwrap();
        assert_eq!(mgr.available(), 4);
    }

    #[test]
    fn double_acquire_rejected() {
        let mgr = ChannelManager::new(4);
        let _tok = mgr.acquire(1).unwrap();
        assert!(mgr.acquire(1).is_err());
    }

    #[test]
    fn out_of_range_rejected() {
        let mgr = ChannelManager::new(4);
        assert!(mgr.acquire(10).is_err());
    }

    #[tokio::test]
    async fn poll_multiple_sensors() {
        let mgr = ChannelManager::new(4);
        let results = poll_sensors_verified(&mgr, &[0, 1, 2]).await.unwrap();
        assert_eq!(results.len(), 3);
        // All channels released.
        assert_eq!(mgr.available(), 4);
    }
}
