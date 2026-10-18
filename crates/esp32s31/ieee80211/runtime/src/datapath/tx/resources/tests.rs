use super::AggregateTxArenaPair;

#[test]
fn arena_pair_swap_preserves_both_unique_owners() {
    let mut pair = AggregateTxArenaPair::new(1_u8, Some(2_u8));

    assert!(pair.swap_active_standby());
    assert_eq!(*pair.active(), 2);
    assert_eq!(pair.standby(), Some(&1));

    let (active, standby) = pair.into_parts();
    assert_eq!(active, 2);
    assert_eq!(standby, Some(1));
}

#[test]
fn single_arena_cannot_cross_a_missing_standby_edge() {
    let mut pair = AggregateTxArenaPair::new(1_u8, None);

    assert!(!pair.swap_active_standby());
    assert_eq!(*pair.active(), 1);
    assert!(!pair.has_standby());
}
