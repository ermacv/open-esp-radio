use super::*;
use oer_hil_protocol::{network::RxConsumerLedgerEvidence, network::RxReorderDeliveryEvidence};

fn exact() -> RxDeliveryEvidence {
    let stage = RxSequenceStageEvidence {
        data_units: 3,
        first: Some(0),
        highest: Some(2),
        control_markers: 1,
        ..Default::default()
    };
    RxDeliveryEvidence {
        post_reorder: stage,
        network_enqueued: stage,
        udp_consumer: stage,
        consumer_ledger: RxConsumerLedgerEvidence {
            matched: 3,
            ..Default::default()
        },
        reorder: RxReorderDeliveryEvidence {
            released: 3,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn classifies_each_delivery_edge_independently() {
    assert!(assess(3, exact()).exact());

    let mut post = exact();
    post.post_reorder.data_units = 2;
    post.network_enqueued = post.post_reorder;
    post.udp_consumer = post.post_reorder;
    post.consumer_ledger.matched = 2;
    assert_eq!(assess(3, post).frontier(), "at-or-before-post-reorder");

    let mut enqueue = exact();
    enqueue.network_queue_full = 1;
    enqueue.network_enqueued.data_units = 2;
    enqueue.udp_consumer = enqueue.network_enqueued;
    enqueue.consumer_ledger.matched = 2;
    assert_eq!(assess(3, enqueue).frontier(), "network-enqueue");

    let mut consumer = exact();
    consumer.consumer_ledger.enqueued_not_consumed = 1;
    consumer.udp_consumer.data_units = 2;
    consumer.consumer_ledger.matched = 2;
    assert_eq!(assess(3, consumer).frontier(), "network-to-udp-consumer");
}

#[test]
fn forward_mac_sequence_localizes_udp_reordering_before_the_target_mac() {
    let mut evidence = exact();
    evidence.post_reorder.gap_events = 1;
    evidence.post_reorder.forward_missing = 1;
    evidence.post_reorder.late_recovered = 1;
    evidence.post_reorder.first_anomaly = Some(2);
    evidence.network_enqueued = evidence.post_reorder;
    evidence.udp_consumer = evidence.post_reorder;
    evidence.mac_order.backward_mac_forward = 1;

    assert_eq!(
        assess(3, evidence).frontier(),
        "before-802.11-sequence-assignment"
    );
}

#[test]
fn pool_exhaustion_and_link_down_fail_admission_even_with_matching_streams() {
    // A dropped terminal/control marker can leave data cardinalities unchanged.
    // Explicit failure counters must still reject an exact-delivery claim.
    let mut pool = exact();
    pool.network_pool_exhausted = 2;
    let mut link = exact();
    link.network_link_down = 1;
    for evidence in [pool, link] {
        let assessment = assess(3, evidence);
        assert!(!assessment.exact());
        assert_eq!(assessment.frontier(), "network-enqueue");
    }
}
