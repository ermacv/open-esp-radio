use super::{Tid, WmmAccessCategory, WmmUserPriority};

#[test]
fn typed_user_priorities_map_to_the_four_standard_access_categories() {
    let expected = [
        WmmAccessCategory::BestEffort,
        WmmAccessCategory::Background,
        WmmAccessCategory::Background,
        WmmAccessCategory::BestEffort,
        WmmAccessCategory::Video,
        WmmAccessCategory::Video,
        WmmAccessCategory::Voice,
        WmmAccessCategory::Voice,
    ];
    for (value, category) in expected.into_iter().enumerate() {
        let priority = WmmUserPriority::new(value as u8).unwrap();
        assert_eq!(priority.value(), value as u8);
        assert_eq!(priority.access_category(), category);
    }
    assert_eq!(WmmUserPriority::new(8), None);
}

/// Each priority is its three-bit value; an eighth is refused.
#[test]
fn a_user_priority_is_its_three_bit_value() {
    for (value, priority) in WmmUserPriority::ALL.into_iter().enumerate() {
        assert_eq!(usize::from(priority.wire().get()), value);
        assert_eq!(usize::from(priority.value()), value);
        assert_eq!(WmmUserPriority::new(priority.value()), Some(priority));
    }
    assert_eq!(WmmUserPriority::new(8), None);
}

#[test]
fn a_tid_is_four_bits_and_selects_its_own_cell() {
    assert_eq!(Tid::new(16), None);
    assert_eq!(Tid::constant::<15>().get(), 15);
    for value in 0..16_u8 {
        let tid = Tid::new(value).unwrap();
        assert_eq!(tid.get(), value);
        let mut cells = [0_u8; 16];
        *tid.select_mut(&mut cells) = 1;
        assert_eq!(cells[usize::from(value)], 1);
        assert_eq!(cells.iter().map(|cell| u32::from(*cell)).sum::<u32>(), 1);
        assert_eq!(*tid.select(&cells), 1);
        assert_eq!(tid.user_priority(), WmmUserPriority::new(value));
    }
    for priority in WmmUserPriority::ALL {
        assert_eq!(priority.tid().get(), priority.value());
        assert_eq!(priority.tid().user_priority(), Some(priority));
    }
}
