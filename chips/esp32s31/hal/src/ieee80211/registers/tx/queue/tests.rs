use crate::ieee80211::registers::ordinary_bank;

#[test]
fn logical_queues_reverse_the_four_physical_banks() {
    let banks = [0, 1, 2, 3].map(|queue| ordinary_bank(queue).map(|bank| bank.get()));
    assert_eq!(banks, [Some(3), Some(2), Some(1), Some(0)]);
    assert_eq!(ordinary_bank(4), None);
}
