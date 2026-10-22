/// NAPOT `tdata2` watching exactly `bottom..bottom + bytes`.
/// The chip contract supplies a power-of-two size of at least 16 bytes.
pub(super) const fn napot_guard(bottom: usize, bytes: usize) -> usize {
    assert!(bottom & (bytes - 1) == 0, "unaligned interrupt stack guard");
    bottom | (bytes / 2 - 1)
}

#[cfg(test)]
mod tests {
    use super::napot_guard;

    #[test]
    fn covers_exactly_the_guard_at_the_stack_bottom() {
        for (bottom, bytes) in [
            (0x2000_0000, 4096),
            (0x2000_8000, 1024),
            (0x4080_0000, 4096),
        ] {
            let encoded = napot_guard(bottom, bytes);
            // NAPOT ignores the trailing ones and the bit above them.
            let mask = (1usize << (encoded.trailing_ones() + 1)) - 1;
            let start = encoded & !mask;
            assert_eq!(start..start + mask + 1, bottom..bottom + bytes);
            for address in [bottom - 1, bottom, bottom + bytes - 1, bottom + bytes] {
                assert_eq!(
                    address & !mask == start,
                    (bottom..bottom + bytes).contains(&address)
                );
            }
        }
    }

    #[test]
    #[should_panic(expected = "unaligned interrupt stack guard")]
    fn refuses_an_unaligned_bottom() {
        napot_guard(0x2000_0440, 4096);
    }
}
