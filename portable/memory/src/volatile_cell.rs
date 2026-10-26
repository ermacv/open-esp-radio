//! Volatile words in memory shared with hardware.

use core::cell::UnsafeCell;

/// A word in memory the hardware may change: volatile access only.
///
/// It is not `Sync`; its zero bytes are a valid value.
#[repr(transparent)]
#[derive(bytemuck::Zeroable)]
pub struct VolatileCell<T>(UnsafeCell<T>);

#[allow(unsafe_code, reason = "the volatile accesses of a cell's own value")]
impl<T: Copy> VolatileCell<T> {
    /// A cell holding `value`.
    pub const fn new(value: T) -> Self {
        Self(UnsafeCell::new(value))
    }

    /// One volatile read.
    #[inline(always)]
    pub fn get(&self) -> T {
        // SAFETY: the cell is not `Sync`, so no other thread accesses it;
        // the pointer is valid and aligned for `T` for as long as `self`.
        unsafe { self.0.get().read_volatile() }
    }

    /// One volatile write.
    #[inline(always)]
    pub fn set(&self, value: T) {
        // SAFETY: as in `get`; `UnsafeCell` permits the write through `&self`
        // and no reference into the cell's value exists.
        unsafe { self.0.get().write_volatile(value) }
    }
}

#[cfg(test)]
mod tests {
    use super::VolatileCell;

    #[test]
    fn a_volatile_cell_holds_the_value_last_written() {
        let cell = VolatileCell::new(0_u32);
        cell.set(0xdead_beef);
        assert_eq!(cell.get(), 0xdead_beef);
        let zero: VolatileCell<u32> = bytemuck::Zeroable::zeroed();
        assert_eq!(zero.get(), 0);
    }
}
