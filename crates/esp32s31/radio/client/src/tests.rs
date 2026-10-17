use super::*;

#[test]
fn the_static_cells_are_claimed_once() {
    assert!(claim().is_some());
    assert!(claim().is_none());
}

#[test]
fn epochs_are_minted_in_order() {
    let mint = MemberMint::new();
    let first = mint.next_epoch(None);
    let second = mint.next_epoch(Some(first));
    assert_eq!((first.get(), second.get()), (0, 1));
    assert_ne!(first, second);
}
