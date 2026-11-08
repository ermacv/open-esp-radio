use super::*;

#[test]
fn an_unknown_cost_is_charged_the_backends_ceiling_of_the_executed_bounds() {
    assert_eq!(AGGREGATE_MEMBER_ATTEMPTS, 32);
    assert_eq!(MAX_SCHEDULE_PUBLICATION_LIMIT, 32);
    assert_eq!(
        S31Ceiling.ceiling(1514),
        msdu_cost_ceiling_micros(1514, 32, 32).unwrap()
    );
    // A longer frame is never charged less, and a frame shorter than its
    // header is charged the most.
    assert!(S31Ceiling.ceiling(1514) > S31Ceiling.ceiling(64));
    assert_eq!(S31Ceiling.ceiling(13), NonZeroU32::MAX);
}
