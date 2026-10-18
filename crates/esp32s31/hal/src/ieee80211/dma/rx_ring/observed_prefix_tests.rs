use super::{RxObservedMask, observed_prefix_len};
use crate::ieee80211::dma::ring_index::RingIndex;

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

#[test]
fn the_alias_check_covers_the_reserved_descriptor() {
    // Ring descriptors at 0x2f0f_ffe8 and 0x2f0f_fff4; R at 0x2f10_0000,
    // whose low 20 bits read as an exhausted NEXT.
    assert!(super::descriptor_aliases_an_exhausted_list::<2>(
        0x2f10_0000 - 2 * crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES
    ));
    assert!(!super::descriptor_aliases_an_exhausted_list::<2>(
        0x2f10_0000 - 3 * crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES
    ));
}
