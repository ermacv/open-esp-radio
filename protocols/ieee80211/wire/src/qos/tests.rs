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
