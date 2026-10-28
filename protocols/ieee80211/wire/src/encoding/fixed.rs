/// Write a fixed-width field into a fixed-width object. Monomorphization
/// proves the entire source fits at OFFSET, so the iterator consumes every
/// byte without a panicking slice operation or a fallible runtime admission.
pub(crate) fn write_array<const OFFSET: usize, const N: usize, const M: usize>(
    output: &mut [u8; N],
    input: &[u8; M],
) {
    const {
        if !(OFFSET <= N && M <= N - OFFSET) {
            panic!("fixed field exceeds its wire object");
        }
    }
    for (destination, source) in output.iter_mut().skip(OFFSET).zip(input) {
        *destination = *source;
    }
}
