//! Volatile access through valid Rust borrows.

/// Read `value` with a volatile load, which the compiler neither folds into
/// an earlier read nor removes: for memory that a DMA engine may write
/// behind the borrow, such as a received frame whose bytes are checked for a
/// foreign write.
pub fn read_volatile<T: Copy>(value: &T) -> T {
    #[allow(unsafe_code, reason = "a volatile load of a shared borrow")]
    // SAFETY: `value` is a shared borrow, so the place is valid, aligned and
    // initialized for the read; a volatile read only reads it.
    unsafe {
        core::ptr::read_volatile(value)
    }
}

/// Store `value` through an exclusive borrow with a volatile write.
/// The compiler keeps the store even when no ordinary read observes it.
pub fn write_volatile<T: Copy>(destination: &mut T, value: T) {
    #[allow(unsafe_code, reason = "a volatile store through an exclusive borrow")]
    // SAFETY: the exclusive borrow is valid and aligned for T, and Copy
    // has no destructor that replacing the previous value could skip.
    unsafe {
        core::ptr::write_volatile(destination, value);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_volatile_read_reads_the_value() {
        let bytes = [1_u8, 2, 3];
        assert_eq!(super::read_volatile(&bytes[1]), 2);
    }

    #[test]
    fn a_volatile_write_replaces_only_the_borrowed_value() {
        let mut words = [1_u32, 2, 3];
        super::write_volatile(&mut words[1], 0x31a5_c33c);
        assert_eq!(words, [1, 0x31a5_c33c, 3]);
    }
}
