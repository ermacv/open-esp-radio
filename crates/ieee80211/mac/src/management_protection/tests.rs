use super::*;

#[test]
fn robust_categories_follow_the_vendor_table() {
    for category in [
        0, 1, 2, 3, 5, 6, 8, 9, 10, 12, 13, 14, 16, 17, 18, 19, 23, 126,
    ] {
        assert!(is_robust_action_category(category), "category {category}");
    }
    for category in [4, 7, 11, 15, 20, 21, 22, 127] {
        assert!(!is_robust_action_category(category), "category {category}");
    }
}

#[test]
fn deauthentication_disassociation_and_robust_actions_are_robust() {
    assert!(is_robust_management_frame(0xc0, 0));
    assert!(is_robust_management_frame(0xa0, 0));
    assert!(is_robust_management_frame(0xd0, 3));
    assert!(!is_robust_management_frame(0xd0, 4));
    assert!(!is_robust_management_frame(0x80, 0));
    assert!(!is_robust_management_frame(0xb0, 0));
}

#[test]
fn sa_query_bodies_round_trip() {
    let request = SaQuery::Request {
        transaction: [0x12, 0x34],
    };
    assert_eq!(request.encode(), [8, 0, 0x12, 0x34]);
    assert_eq!(SaQuery::parse(&request.encode()), Some(request));
    let response = SaQuery::Response {
        transaction: [0x12, 0x34],
    };
    assert_eq!(SaQuery::parse(&[8, 1, 0x12, 0x34]), Some(response));
    assert_eq!(SaQuery::parse(&[8, 2, 0x12, 0x34]), None);
    assert_eq!(SaQuery::parse(&[3, 0, 0x12, 0x34]), None);
    assert_eq!(SaQuery::parse(&[8, 0, 0x12]), None);
}
