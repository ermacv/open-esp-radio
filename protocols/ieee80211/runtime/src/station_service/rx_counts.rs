//! Passive RX accounting, separate from admission and capacity.
use oer_ieee80211_sta::machine::StaRxError;
use oer_ieee80211_upper_mac::receive::RxDropReason;
use oer_ieee80211_wire::data::traffic::{Traffic, TrafficCounts};

pub const REASONS: [&str; 10] = [
    "header",
    "frame-kind",
    "address",
    "fragmented",
    "protection",
    "integrity",
    "replay",
    "payload",
    "not-connected",
    "security",
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ServiceRxCounts {
    pub input: TrafficCounts,
    pub delivered: TrafficCounts,
    pub reorder_dropped: TrafficCounts,
    pub drained: TrafficCounts,
    pub handled: TrafficCounts,
    pub queue_full: TrafficCounts,
    pub queued: TrafficCounts,
    pub too_long: TrafficCounts,
    pub rejections: [TrafficCounts; 10],
}
impl ServiceRxCounts {
    pub(super) fn rejected(&mut self, traffic: Traffic, reason: StaRxError) {
        let index = match reason {
            StaRxError::Frame(reason) => match reason {
                RxDropReason::Header => 0,
                RxDropReason::FrameKind => 1,
                RxDropReason::Address => 2,
                RxDropReason::Fragmented => 3,
                RxDropReason::Protection => 4,
                RxDropReason::Integrity => 5,
                RxDropReason::Replay => 6,
                RxDropReason::Payload => 7,
                RxDropReason::NotConnected => 8,
            },
            StaRxError::Security(_) => 9,
        };
        if let Some(counts) = self.rejections.get_mut(index) {
            counts.record(traffic);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refusals_keep_their_cause_and_do_not_merge_echo_and_background_traffic() {
        let mut counts = ServiceRxCounts::default();
        counts.rejected(
            Traffic::EchoRequest,
            StaRxError::Frame(RxDropReason::Address),
        );
        counts.rejected(Traffic::Arp, StaRxError::Frame(RxDropReason::Address));
        counts.rejected(
            Traffic::EchoReply,
            StaRxError::Frame(RxDropReason::Integrity),
        );
        assert_eq!(counts.rejections[2].echo_request, 1);
        assert_eq!(counts.rejections[2].arp, 1);
        assert_eq!(counts.rejections[5].echo_reply, 1);
        assert_eq!(counts.queued.total(), 0);
    }
}
