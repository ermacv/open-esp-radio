use oer_hil_protocol::{network::RxDeliveryEvidence, network::RxSequenceStageEvidence};

/// Where a session's RX delivery first diverged from the host's offer.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub struct RxDeliveryAssessment {
    pub host_to_post_reorder: bool,
    pub before_mac_sequence_ordering: bool,
    pub post_reorder_to_enqueue: bool,
    pub enqueue_to_consumer: bool,
}

impl RxDeliveryAssessment {
    pub const fn exact(self) -> bool {
        !self.host_to_post_reorder && !self.post_reorder_to_enqueue && !self.enqueue_to_consumer
    }

    pub fn frontier(self) -> &'static str {
        match (
            self.host_to_post_reorder,
            self.before_mac_sequence_ordering,
            self.post_reorder_to_enqueue,
            self.enqueue_to_consumer,
        ) {
            (false, false, false, false) => "exact",
            (true, true, false, false) => "before-802.11-sequence-assignment",
            (true, false, false, false) => "at-or-before-post-reorder",
            (false, false, true, false) => "network-enqueue",
            (false, false, false, true) => "network-to-udp-consumer",
            _ => "multiple-frontiers",
        }
    }
}

pub fn assess(host_units: u64, evidence: RxDeliveryEvidence) -> RxDeliveryAssessment {
    let host_to_post_reorder = !stage_matches_host(evidence.post_reorder, host_units);
    let post = evidence.post_reorder;
    let mac = evidence.mac_order;
    let before_mac_sequence_ordering = host_to_post_reorder
        && stage_has_exact_cardinality(post, host_units)
        && post.forward_missing == post.late_recovered
        && post.late_recovered != 0
        && mac.backward_mac_forward == post.late_recovered
        && mac.backward_mac_backward == 0
        && mac.backward_mac_same == 0
        && mac.backward_mac_other_tid == 0
        && mac.backward_mac_unavailable == 0;
    let post_reorder_to_enqueue = evidence.network_queue_full != 0
        || evidence.network_invalid_length != 0
        || evidence.network_pool_exhausted != 0
        || evidence.network_link_down != 0
        || !same_sequence_stream(evidence.post_reorder, evidence.network_enqueued);
    let ledger = evidence.consumer_ledger;
    let enqueue_to_consumer = ledger.overflow != 0
        || ledger.enqueued_not_consumed != 0
        || ledger.skipped_before_observed != 0
        || ledger.unexpected_consumer != 0
        || u64::from(ledger.matched) != u64::from(evidence.network_enqueued.data_units)
        || !same_sequence_stream(evidence.network_enqueued, evidence.udp_consumer);
    RxDeliveryAssessment {
        host_to_post_reorder,
        before_mac_sequence_ordering,
        post_reorder_to_enqueue,
        enqueue_to_consumer,
    }
}

fn stage_has_exact_cardinality(stage: RxSequenceStageEvidence, host_units: u64) -> bool {
    let expected_highest = host_units
        .checked_sub(1)
        .and_then(|value| u32::try_from(value).ok());
    u64::from(stage.data_units) == host_units
        && stage.first == (host_units != 0).then_some(0)
        && stage.highest == expected_highest
        && stage.duplicates == 0
        && stage.backward_unclassified == 0
        && stage.data_after_terminal == 0
}

fn stage_matches_host(stage: RxSequenceStageEvidence, host_units: u64) -> bool {
    let expected_highest = host_units
        .checked_sub(1)
        .and_then(|value| u32::try_from(value).ok());
    u64::from(stage.data_units) == host_units
        && stage.first == (host_units != 0).then_some(0)
        && stage.highest == expected_highest
        && stage.gap_events == 0
        && stage.forward_missing == 0
        && stage.late_recovered == 0
        && stage.duplicates == 0
        && stage.backward_unclassified == 0
        && stage.data_after_terminal == 0
}

fn same_sequence_stream(left: RxSequenceStageEvidence, right: RxSequenceStageEvidence) -> bool {
    left == right
}

#[cfg(test)]
mod tests;
