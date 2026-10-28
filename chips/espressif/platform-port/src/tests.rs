use super::Comparator;

#[test]
fn every_comparator_index_names_its_target_and_no_other_index_does() {
    assert_eq!(Comparator::from_index(0), Some(Comparator::Zero));
    assert_eq!(Comparator::from_index(1), Some(Comparator::One));
    assert_eq!(Comparator::from_index(2), Some(Comparator::Two));
    for index in 3..=u8::MAX {
        assert_eq!(Comparator::from_index(index), None);
    }
}

#[cfg(chip_geometry)]
#[test]
fn the_geometry_checks_every_raw_hart_and_names_the_boot_hart() {
    use super::{BOOT_HART, HARTS, Hart, InvalidHart};
    for (index, hart) in Hart::ALL.into_iter().enumerate() {
        assert_eq!(hart.get(), index);
        assert_eq!(Hart::new(index as u32), Ok(hart));
    }
    assert_eq!(
        Hart::new(HARTS as u32),
        Err(InvalidHart { raw: HARTS as u32 })
    );
    assert!(Hart::ALL.contains(&BOOT_HART));
}
