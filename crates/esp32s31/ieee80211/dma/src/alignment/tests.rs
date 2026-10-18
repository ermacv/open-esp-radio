use super::*;

#[test]
fn aligned_header_reservation_matches_the_pinned_leaf() {
    assert_eq!(
        plan(0x2000, 24, 24, 100, 0x3fff_8123),
        Some(AlignPlan {
            reserved_start: 0x1fe8,
            aligned_start: 0x1fe8,
            move_len: 124,
            storage_word: 0xd01f_0123,
        })
    );
}

#[test]
fn qos_header_moves_at_most_three_alignment_bytes() {
    assert_eq!(
        plan(0x2000, 26, 26, 100, 0x2000_0007),
        Some(AlignPlan {
            reserved_start: 0x1fe6,
            aligned_start: 0x1fe4,
            move_len: 126,
            storage_word: 0xc01f_8007,
        })
    );
}

#[test]
fn only_the_recovered_finite_layout_is_admitted() {
    assert_eq!(plan(0x2000, 25, 25, 100, 0), None);
    assert_eq!(plan(0x2000, 24, 26, 100, 0), None);
    assert_eq!(plan(20, 24, 24, 100, 0), None);
    assert_eq!(plan(0x5000, 24, 24, 0x3fff, 0), None);
}
