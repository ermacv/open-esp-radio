use super::{WmmAccessCategory, WmmUserPriority};

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
