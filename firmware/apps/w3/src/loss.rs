//! Passive observations at the bridge boundaries. Means are computed on the host.
/// Render one typed traffic row without a target formatter.
pub fn traffic_note(
    stage: oer_diagnostics::Name<32>,
    counts: oer_ieee80211_wire::data::traffic::TrafficCounts,
) -> oer_diagnostics::Note<'static> {
    oer_diagnostics::Note::w3_rx_traffic(
        stage,
        counts.echo_request,
        counts.echo_reply,
        counts.other_icmp,
        counts.other_ipv4,
        counts.arp,
        counts.ipv6,
        counts.other,
        counts.unidentified,
    )
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Counters {
    pub queue_samples: u32,
    pub queue_sum: u64,
    pub queue_max: u32,
    pub publisher_blocked: u32,
    pub published: u32,
    pub echo_requests: u32,
    pub echo_replies: u32,
}
impl Counters {
    pub const fn new() -> Self {
        Self {
            queue_samples: 0,
            queue_sum: 0,
            queue_max: 0,
            publisher_blocked: 0,
            published: 0,
            echo_requests: 0,
            echo_replies: 0,
        }
    }
    pub fn sample_queue(&mut self, queued: u32) {
        self.queue_samples = self.queue_samples.saturating_add(1);
        self.queue_sum = self.queue_sum.saturating_add(u64::from(queued));
        self.queue_max = self.queue_max.max(queued);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_receive_note_preserves_every_traffic_class() {
        let note = traffic_note(
            oer_diagnostics::name!("dma"; 32),
            oer_ieee80211_wire::data::traffic::TrafficCounts {
                echo_request: 1,
                echo_reply: 2,
                other_icmp: 3,
                other_ipv4: 4,
                arp: 5,
                ipv6: 6,
                other: 7,
                unidentified: 8,
            },
        );
        assert_eq!(note.words(), &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(note.names(), &["dma"]);
    }
    #[test]
    fn samples_include_empty_turns_and_saturate() {
        let mut counts = Counters::new();
        for queued in [0, 8, 4] {
            counts.sample_queue(queued);
        }
        assert_eq!(
            (counts.queue_samples, counts.queue_sum, counts.queue_max),
            (3, 12, 8)
        );
        counts.queue_samples = u32::MAX;
        counts.queue_sum = u64::MAX;
        counts.sample_queue(8);
        assert_eq!(
            (counts.queue_samples, counts.queue_sum),
            (u32::MAX, u64::MAX)
        );
    }
}
