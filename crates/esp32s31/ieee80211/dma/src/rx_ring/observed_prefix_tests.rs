use super::{RxObservedMask, observed_prefix_len};
use crate::ring_index::RingIndex;

#[test]
fn counts_linear_wrapped_full_and_holed_prefixes() {
    let mask = |word| RxObservedMask {
        words: [word, 0, 0],
    };
    assert_eq!(
        observed_prefix_len::<8>(mask(0b0000_0111), RingIndex::checked(0).unwrap()),
        3
    );
    assert_eq!(
        observed_prefix_len::<8>(mask(0b1100_0001), RingIndex::checked(6).unwrap()),
        3
    );
    assert_eq!(
        observed_prefix_len::<8>(mask(0b1100_0101), RingIndex::checked(6).unwrap()),
        3
    );
    assert_eq!(
        observed_prefix_len::<8>(mask(0b1010_0001), RingIndex::checked(6).unwrap()),
        0
    );
    assert_eq!(
        observed_prefix_len::<8>(mask(u32::MAX), RingIndex::checked(5).unwrap()),
        8
    );
    assert_eq!(
        observed_prefix_len::<64>(
            RxObservedMask {
                words: [u32::MAX, u32::MAX, 0],
            },
            RingIndex::checked(63).unwrap(),
        ),
        64
    );
    assert_eq!(
        observed_prefix_len::<96>(
            RxObservedMask {
                words: [u32::MAX; 3],
            },
            RingIndex::checked(80).unwrap(),
        ),
        96
    );
}
