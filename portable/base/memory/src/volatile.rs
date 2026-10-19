//! A volatile read of a value behind a shared borrow.

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

#[cfg(test)]
mod tests {
    #[test]
    fn a_volatile_read_reads_the_value() {
        let bytes = [1_u8, 2, 3];
        assert_eq!(super::read_volatile(&bytes[1]), 2);
    }
}
