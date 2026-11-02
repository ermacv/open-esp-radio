//! Constant-time comparison of secret-derived bytes.

/// Whether two byte sequences of equal length are equal, accumulating every
/// difference before the one data-independent decision. Sequences of
/// different lengths compare unequal.
pub(crate) fn constant_time_eq<'a>(
    left: impl ExactSizeIterator<Item = &'a u8>,
    right: impl ExactSizeIterator<Item = &'a u8>,
) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (a, b) in left.zip(right) {
        difference |= a ^ b;
    }
    core::hint::black_box(difference) == 0
}
