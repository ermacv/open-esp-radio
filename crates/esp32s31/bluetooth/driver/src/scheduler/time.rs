//! Positional microsecond time in the source-owned Bluetooth scheduler.
//!
//! This module carries only wrapping scheduler positions and their ordering.
//! Projection to or from raw controller time remains owned by the live
//! controller epoch.

#![forbid(unsafe_code)]

/// One positional microsecond instant in the BLE software scheduler.
///
/// External callers cannot manufacture an instant from a detached integer
/// image. Internal protocol roles may share the same retained scheduler epoch
/// without inventing role-specific time domains.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerInstant(u32);

impl SchedulerInstant {
    /// Preserve one complete wrapping microsecond image.
    #[cfg(any(target_arch = "riscv32", test))]
    pub const fn from_image(image: u32) -> Self {
        Self(image)
    }

    /// Return the complete positional microsecond image.
    pub const fn image(self) -> u32 {
        self.0
    }

    /// Advance by one wrapping microsecond delta.
    pub const fn wrapping_add(self, delta: u32) -> Self {
        Self(self.0.wrapping_add(delta))
    }

    /// Select the later position under the reviewed signed wrapping order.
    #[cfg(any(target_arch = "riscv32", test))]
    pub const fn later(self, other: Self) -> Self {
        if self.is_before(other) { other } else { self }
    }

    /// Whether this position precedes another under signed wrapping order.
    #[cfg(any(target_arch = "riscv32", test))]
    pub const fn is_before(self, other: Self) -> bool {
        (self.0.wrapping_sub(other.0) as i32) < 0
    }
}
