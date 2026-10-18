//! Caller-owned software packet storage, borrowed by one AP epoch at a time.

use super::queue::ApFrameLeaseArena;
use core::ops::{Deref, DerefMut};

/// CPU-only storage for retained AP packets. The production composition places
/// this outside movable futures. Custom callers may keep it on their stack;
/// no static lifetime, pinning, allocator or hardware address is required.
pub struct AccessPointTxStorage<N> {
    frames: ApFrameLeaseArena<N>,
}

impl<N> AccessPointTxStorage<N> {
    pub const fn new() -> Self {
        Self {
            frames: ApFrameLeaseArena::new(),
        }
    }

    pub(super) fn borrow(&mut self) -> FrameArenaLease<'_, N> {
        debug_assert_eq!(
            self.frames.remaining_capacity(),
            super::AP_SOFTWARE_TX_CAPACITY
        );
        FrameArenaLease {
            storage: Some(self),
        }
    }
}

impl<N> Default for AccessPointTxStorage<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// Queue indices remain in the epoch owner. Dropping that owner releases all
/// retained packets before the caller can lend the storage to another epoch.
pub(super) struct FrameArenaLease<'a, N> {
    storage: Option<&'a mut AccessPointTxStorage<N>>,
}

impl<'a, N> FrameArenaLease<'a, N> {
    pub(super) fn release(mut self) -> &'a mut AccessPointTxStorage<N> {
        let storage = self.storage.take().expect("one storage borrow");
        storage.frames.clear();
        storage
    }
}

impl<N> Deref for FrameArenaLease<'_, N> {
    type Target = ApFrameLeaseArena<N>;
    fn deref(&self) -> &Self::Target {
        &self.storage.as_ref().expect("live storage borrow").frames
    }
}

impl<N> DerefMut for FrameArenaLease<'_, N> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.storage.as_mut().expect("live storage borrow").frames
    }
}

impl<N> Drop for FrameArenaLease<'_, N> {
    fn drop(&mut self) {
        if let Some(storage) = self.storage.as_mut() {
            storage.frames.clear();
        }
    }
}

#[cfg(test)]
mod tests;
