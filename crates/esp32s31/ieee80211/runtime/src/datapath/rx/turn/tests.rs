use super::*;

#[test]
fn scheduler_credit_caps_the_role_limit_and_never_creates_zero_budget() {
    assert_eq!(
        protocol_frame_limit(
            DatapathRxServiceContext {
                maximum_protocol_frames: None,
            },
            32,
        ),
        32
    );
    assert_eq!(
        protocol_frame_limit(
            DatapathRxServiceContext {
                maximum_protocol_frames: Some(4),
            },
            32,
        ),
        4
    );
    assert_eq!(
        protocol_frame_limit(
            DatapathRxServiceContext {
                maximum_protocol_frames: Some(0),
            },
            32,
        ),
        1
    );
    assert_eq!(
        protocol_frame_limit(
            DatapathRxServiceContext {
                maximum_protocol_frames: None,
            },
            0,
        ),
        1
    );
}

#[test]
fn empty_protocol_observations_do_not_spend_frame_credit() {
    let mut turn = FusedRxTurn::new(2);

    turn.observe_protocol(0, false);
    turn.observe_protocol(0, false);

    assert!(turn.has_protocol_budget());
    assert_eq!(turn.remaining_protocol_frames(), 2);
}

#[test]
fn only_post_dma_consumption_releases_an_observed_capacity_block() {
    let mut blocked = FusedRxTurn::new(4);
    blocked.observe_protocol(1, false);
    blocked.observe_dma(DatapathRxProgress::StageCapacityBlocked);
    assert_eq!(
        blocked.finish(false),
        DatapathRxProgress::StageCapacityBlocked
    );

    let mut released = FusedRxTurn::new(4);
    released.observe_dma(DatapathRxProgress::StageCapacityBlocked);
    released.observe_protocol(1, false);
    assert_eq!(released.finish(false), DatapathRxProgress::BudgetExhausted);
}

#[test]
fn protocol_backlog_preserves_a_runnable_continuation() {
    let mut turn = FusedRxTurn::new(4);
    turn.observe_protocol(4, true);
    turn.observe_dma(DatapathRxProgress::Drained);

    assert_eq!(turn.finish(false), DatapathRxProgress::BudgetExhausted);
}

#[test]
#[should_panic(expected = "one fused RX turn cannot service two DMA frontiers")]
fn a_fused_turn_rejects_a_second_dma_phase() {
    let mut turn = FusedRxTurn::new(4);
    turn.observe_dma(DatapathRxProgress::Drained);
    turn.observe_dma(DatapathRxProgress::Drained);
}
