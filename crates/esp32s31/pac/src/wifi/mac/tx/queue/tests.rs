use super::*;

#[test]
fn logical_queues_reverse_the_four_physical_banks() {
    assert_eq!(physical_bank(0), 3);
    assert_eq!(physical_bank(1), 2);
    assert_eq!(physical_bank(2), 1);
    assert_eq!(physical_bank(3), 0);
}
