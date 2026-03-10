//! Verified resource allocator for lab consumables.
//!
//! Tracks reagent volumes, tip-rack slots, and well-plate positions.
//! Under Verus, an SMT proof guarantees that the allocator never
//! over-commits a resource.

use crate::verus_shim::*;

/// A fixed-capacity resource pool (e.g., a well plate with N wells).
pub struct ResourcePool {
    capacity: u64,
    allocated: u64,
    _ghost_sum: Ghost<u64>,
}

impl ResourcePool {
    pub fn new(capacity: u64) -> Self {
        Self {
            capacity,
            allocated: 0,
            _ghost_sum: Ghost::new(0),
        }
    }

    /// Allocate `amount` units from the pool.
    ///
    /// Precondition: `allocated + amount <= capacity`.
    pub fn allocate(&mut self, amount: u64) -> Result<u64, &'static str> {
        requires!(self.allocated + amount <= self.capacity);

        self.allocated += amount;
        self._ghost_sum = Ghost::new(self.allocated);

        invariant!(self.allocated <= self.capacity);
        ensures!(|_r: &Result<u64, &'static str>| self.allocated <= self.capacity);

        Ok(self.allocated)
    }

    /// Return `amount` units back to the pool.
    ///
    /// Precondition: `amount <= allocated`.
    pub fn deallocate(&mut self, amount: u64) -> Result<u64, &'static str> {
        requires!(amount <= self.allocated);

        self.allocated -= amount;
        self._ghost_sum = Ghost::new(self.allocated);

        invariant!(self.allocated <= self.capacity);
        Ok(self.allocated)
    }

    pub fn remaining(&self) -> u64 {
        self.capacity - self.allocated
    }

    pub fn allocated(&self) -> u64 {
        self.allocated
    }

    pub fn capacity(&self) -> u64 {
        self.capacity
    }
}

/// A well plate with typed positions (row, col).
pub struct WellPlate {
    rows: u32,
    cols: u32,
    occupied: Vec<bool>,
}

impl WellPlate {
    pub fn new(rows: u32, cols: u32) -> Self {
        let total = (rows * cols) as usize;
        Self {
            rows,
            cols,
            occupied: vec![false; total],
        }
    }

    fn index(&self, row: u32, col: u32) -> Result<usize, &'static str> {
        requires!(row < self.rows && col < self.cols);
        Ok((row * self.cols + col) as usize)
    }

    /// Claim a well, returning an error if out-of-range or already occupied.
    pub fn claim(&mut self, row: u32, col: u32) -> Result<(), &'static str> {
        let idx = self.index(row, col)?;
        if self.occupied[idx] {
            return Err("well already occupied");
        }
        self.occupied[idx] = true;
        Ok(())
    }

    /// Release a well.
    pub fn release(&mut self, row: u32, col: u32) -> Result<(), &'static str> {
        let idx = self.index(row, col)?;
        if !self.occupied[idx] {
            return Err("well not occupied");
        }
        self.occupied[idx] = false;
        Ok(())
    }

    pub fn free_wells(&self) -> usize {
        self.occupied.iter().filter(|&&o| !o).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ResourcePool ──
    #[test]
    fn pool_allocate_and_deallocate() {
        let mut pool = ResourcePool::new(100);
        pool.allocate(40).unwrap();
        assert_eq!(pool.remaining(), 60);
        pool.deallocate(10).unwrap();
        assert_eq!(pool.remaining(), 70);
    }

    #[test]
    fn pool_reject_overcommit() {
        let mut pool = ResourcePool::new(100);
        pool.allocate(90).unwrap();
        assert!(pool.allocate(20).is_err()); // 90+20 > 100
    }

    #[test]
    fn pool_reject_over_dealloc() {
        let mut pool = ResourcePool::new(100);
        pool.allocate(10).unwrap();
        assert!(pool.deallocate(20).is_err());
    }

    // ── WellPlate ──
    #[test]
    fn well_claim_release() {
        let mut plate = WellPlate::new(8, 12); // standard 96-well
        assert_eq!(plate.free_wells(), 96);
        plate.claim(0, 0).unwrap();
        assert_eq!(plate.free_wells(), 95);
        plate.release(0, 0).unwrap();
        assert_eq!(plate.free_wells(), 96);
    }

    #[test]
    fn well_double_claim_rejected() {
        let mut plate = WellPlate::new(8, 12);
        plate.claim(3, 5).unwrap();
        assert!(plate.claim(3, 5).is_err());
    }

    #[test]
    fn well_out_of_range() {
        let mut plate = WellPlate::new(8, 12);
        assert!(plate.claim(10, 0).is_err());
    }
}
