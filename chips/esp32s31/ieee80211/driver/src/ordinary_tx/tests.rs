use super::*;

#[test]
fn tx_plan_contains_protocol_policy_but_no_hardware_queue_encoding() {
    let plan = MacTxPlan {
        access_category: WmmAccessCategory::Video,
        initial_rate: 7_u8,
        publication_limit: 4,
        publication_timeout: oer_time::Duration::from_micros(250_000),
    };
    assert_eq!(plan.access_category, WmmAccessCategory::Video);
    assert_eq!(plan.initial_rate, 7);
    assert_eq!(plan.publication_limit, 4);
}
