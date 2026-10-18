//! Exact, session-scoped UDP RX delivery evidence for qualification images.

use oer_hil_protocol::{
    network::RxConsumerLedgerEvidence, network::RxDeliveryEvidence, network::RxForwardGapEvidence,
    network::RxMacOrderEvidence, network::RxReorderDeliveryEvidence,
    network::RxSequenceStageEvidence,
};
use oer_ieee80211_mac::sequence::SequenceNumber;

const SEEN_WINDOW: usize = 256;
const SEEN_WORDS: usize = SEEN_WINDOW / 64;
pub const FORWARD_GAP_SAMPLE_CAPACITY: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkDropReason {
    QueueFull,
    InvalidLength,
    PoolExhausted,
    LinkDown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SequenceDisposition {
    Control,
    First,
    InOrder,
    Gap,
    LateRecovered,
    Duplicate,
    BackwardUnclassified,
}

#[derive(Default)]
struct SequenceTracker {
    evidence: RxSequenceStageEvidence,
    seen: [u64; SEEN_WORDS],
    terminal_seen: bool,
}

impl SequenceTracker {
    fn observe(&mut self, sequence: i32) -> SequenceDisposition {
        if sequence < 0 {
            self.evidence.control_markers = self.evidence.control_markers.saturating_add(1);
            if self.evidence.data_units != 0 {
                self.terminal_seen = true;
            }
            return SequenceDisposition::Control;
        }
        let sequence = sequence as u32;
        if self.terminal_seen {
            self.evidence.data_after_terminal = self.evidence.data_after_terminal.saturating_add(1);
            self.record_anomaly(sequence);
        }
        self.evidence.data_units = self.evidence.data_units.saturating_add(1);
        let Some(highest) = self.evidence.highest else {
            self.evidence.first = Some(sequence);
            self.evidence.highest = Some(sequence);
            self.seen[0] = 1;
            return SequenceDisposition::First;
        };
        if sequence > highest {
            let advance = sequence - highest;
            self.shift_seen(advance);
            self.seen[0] |= 1;
            self.evidence.highest = Some(sequence);
            if advance == 1 {
                SequenceDisposition::InOrder
            } else {
                self.evidence.gap_events = self.evidence.gap_events.saturating_add(1);
                self.evidence.forward_missing = self
                    .evidence
                    .forward_missing
                    .saturating_add(advance.saturating_sub(1));
                self.record_anomaly(sequence);
                SequenceDisposition::Gap
            }
        } else {
            let distance = highest - sequence;
            if distance >= SEEN_WINDOW as u32 {
                self.evidence.backward_unclassified =
                    self.evidence.backward_unclassified.saturating_add(1);
                self.record_anomaly(sequence);
                SequenceDisposition::BackwardUnclassified
            } else if self.seen(distance as usize) {
                self.evidence.duplicates = self.evidence.duplicates.saturating_add(1);
                self.record_anomaly(sequence);
                SequenceDisposition::Duplicate
            } else {
                self.mark_seen(distance as usize);
                self.evidence.late_recovered = self.evidence.late_recovered.saturating_add(1);
                self.record_anomaly(sequence);
                SequenceDisposition::LateRecovered
            }
        }
    }

    fn record_anomaly(&mut self, sequence: u32) {
        self.evidence.first_anomaly.get_or_insert(sequence);
    }

    fn seen(&self, distance: usize) -> bool {
        self.seen[distance / 64] & (1_u64 << (distance % 64)) != 0
    }

    fn mark_seen(&mut self, distance: usize) {
        self.seen[distance / 64] |= 1_u64 << (distance % 64);
    }

    fn shift_seen(&mut self, advance: u32) {
        if advance >= SEEN_WINDOW as u32 {
            self.seen = [0; SEEN_WORDS];
            return;
        }
        let advance = advance as usize;
        let words = advance / 64;
        let bits = advance % 64;
        let old = self.seen;
        self.seen = [0; SEEN_WORDS];
        for destination in words..SEEN_WORDS {
            let source = destination - words;
            self.seen[destination] |= old[source] << bits;
            if bits != 0 && source > 0 {
                self.seen[destination] |= old[source - 1] >> (64 - bits);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MacOrder {
    Backward,
    Same,
    Forward,
}

#[derive(Default)]
struct MacOrderTracker {
    expected: [Option<SequenceNumber>; 16],
    last: Option<(u8, u16)>,
    last_udp: Option<u32>,
    evidence: RxMacOrderEvidence,
    correlated_gaps: u32,
    gap_samples: [Option<RxForwardGapEvidence>; FORWARD_GAP_SAMPLE_CAPACITY],
}

impl MacOrderTracker {
    fn observe(&mut self, disposition: SequenceDisposition, mac: Option<(u8, u16)>, udp: i32) {
        if disposition == SequenceDisposition::Control {
            return;
        }
        let previous = self.last;
        let previous_udp = self.last_udp;
        self.last_udp = u32::try_from(udp).ok();
        if matches!(disposition, SequenceDisposition::Gap)
            && let (
                Some(previous_udp),
                Some(current_udp),
                Some((previous_tid, previous_mac)),
                Some((tid, current_mac)),
            ) = (previous_udp, self.last_udp, previous, mac)
            && previous_tid == tid
            && tid < 16
            && previous_mac < 4096
            && current_mac < 4096
            && current_udp > previous_udp.saturating_add(1)
        {
            let gap = RxForwardGapEvidence {
                previous_udp,
                current_udp,
                tid,
                previous_mac,
                current_mac,
            };
            self.evidence.first_forward_gap.get_or_insert(gap);
            if let Some(slot) = self.gap_samples.get_mut(self.correlated_gaps as usize) {
                *slot = Some(gap);
            }
            self.correlated_gaps = self.correlated_gaps.saturating_add(1);
        }
        let order = mac.map(|(tid, sequence)| self.observe_mac(tid, sequence));
        self.last = mac;
        if !matches!(
            disposition,
            SequenceDisposition::LateRecovered | SequenceDisposition::BackwardUnclassified
        ) {
            return;
        }
        match (previous, mac, order) {
            (Some((previous_tid, _)), Some((tid, _)), _) if previous_tid != tid => {
                self.evidence.backward_mac_other_tid =
                    self.evidence.backward_mac_other_tid.saturating_add(1);
            }
            (_, Some(_), Some(MacOrder::Backward)) => {
                self.evidence.backward_mac_backward =
                    self.evidence.backward_mac_backward.saturating_add(1);
            }
            (_, Some(_), Some(MacOrder::Same)) => {
                self.evidence.backward_mac_same = self.evidence.backward_mac_same.saturating_add(1);
            }
            (_, Some(_), Some(MacOrder::Forward)) => {
                self.evidence.backward_mac_forward =
                    self.evidence.backward_mac_forward.saturating_add(1);
            }
            _ => {
                self.evidence.backward_mac_unavailable =
                    self.evidence.backward_mac_unavailable.saturating_add(1);
            }
        }
    }

    fn observe_mac(&mut self, tid: u8, raw_sequence: u16) -> MacOrder {
        if self.last == Some((tid, raw_sequence)) {
            return MacOrder::Same;
        }
        let sequence = SequenceNumber::from_low_bits(raw_sequence);
        let Some(expected) = self.expected.get_mut(usize::from(tid)) else {
            return MacOrder::Forward;
        };
        let Some(frontier) = *expected else {
            *expected = Some(sequence.next());
            return MacOrder::Forward;
        };
        if frontier.forward_distance(sequence) < SequenceNumber::HALF_SPACE {
            *expected = Some(sequence.next());
            MacOrder::Forward
        } else {
            MacOrder::Backward
        }
    }
}

struct SequenceLedger<const CAPACITY: usize> {
    entries: [Option<u32>; CAPACITY],
    head: usize,
    length: usize,
    evidence: RxConsumerLedgerEvidence,
}

impl<const CAPACITY: usize> Default for SequenceLedger<CAPACITY> {
    fn default() -> Self {
        Self {
            entries: [None; CAPACITY],
            head: 0,
            length: 0,
            evidence: RxConsumerLedgerEvidence::default(),
        }
    }
}

impl<const CAPACITY: usize> SequenceLedger<CAPACITY> {
    fn push(&mut self, sequence: u32) {
        if self.length == CAPACITY {
            self.evidence.overflow = self.evidence.overflow.saturating_add(1);
            return;
        }
        let index = (self.head + self.length) % CAPACITY;
        self.entries[index] = Some(sequence);
        self.length += 1;
    }

    fn consume(&mut self, sequence: u32) {
        let position = (0..self.length).find(|offset| self.get(*offset) == Some(sequence));
        let Some(position) = position else {
            self.record_divergence(self.front(), sequence);
            self.evidence.unexpected_consumer = self.evidence.unexpected_consumer.saturating_add(1);
            return;
        };
        if position != 0 {
            self.record_divergence(self.front(), sequence);
            self.evidence.skipped_before_observed = self
                .evidence
                .skipped_before_observed
                .saturating_add(position as u32);
            for _ in 0..position {
                self.pop_front();
            }
        }
        self.pop_front();
        self.evidence.matched = self.evidence.matched.saturating_add(1);
    }

    fn finish(mut self) -> RxConsumerLedgerEvidence {
        self.evidence.enqueued_not_consumed = self.length as u32;
        self.evidence
    }

    fn get(&self, offset: usize) -> Option<u32> {
        self.entries[(self.head + offset) % CAPACITY]
    }

    fn front(&self) -> Option<u32> {
        (self.length != 0)
            .then(|| self.entries[self.head])
            .flatten()
    }

    fn pop_front(&mut self) {
        if self.length == 0 {
            return;
        }
        self.entries[self.head] = None;
        self.head = (self.head + 1) % CAPACITY;
        self.length -= 1;
    }

    fn record_divergence(&mut self, expected: Option<u32>, observed: u32) {
        if self.evidence.first_observed.is_none() {
            self.evidence.first_expected = expected;
            self.evidence.first_observed = Some(observed);
        }
    }
}

/// Mutable state shared by the post-reorder publisher and UDP consumer only
/// in the explicit delivery diagnostic profile.
pub struct RxDeliveryTracker<const LEDGER_CAPACITY: usize> {
    session_id: Option<u64>,
    completed_session_id: Option<u64>,
    post_reorder: SequenceTracker,
    network_enqueued: SequenceTracker,
    udp_consumer: SequenceTracker,
    ledger: SequenceLedger<LEDGER_CAPACITY>,
    mac: MacOrderTracker,
    network_queue_full: u32,
    network_invalid_length: u32,
    network_pool_exhausted: u32,
    network_link_down: u32,
}

impl<const LEDGER_CAPACITY: usize> RxDeliveryTracker<LEDGER_CAPACITY> {
    /// Correlations from exactly one completed session; unavailable identities
    /// are not invented. The total may exceed the retained sample capacity.
    /// Reporting must happen outside the packet observer and critical section.
    pub fn completed_gap_samples(
        &self,
        session_id: u64,
    ) -> Option<(u32, &[Option<RxForwardGapEvidence>])> {
        (self.completed_session_id == Some(session_id))
            .then_some((self.mac.correlated_gaps, &self.mac.gap_samples[..]))
    }

    pub fn new() -> Self {
        Self {
            session_id: None,
            completed_session_id: None,
            post_reorder: SequenceTracker::default(),
            network_enqueued: SequenceTracker::default(),
            udp_consumer: SequenceTracker::default(),
            ledger: SequenceLedger::default(),
            mac: MacOrderTracker::default(),
            network_queue_full: 0,
            network_invalid_length: 0,
            network_pool_exhausted: 0,
            network_link_down: 0,
        }
    }

    pub fn begin(&mut self, session_id: u64) {
        *self = Self::new();
        self.session_id = Some(session_id);
    }

    pub fn admitted(&mut self, sequence: i32, mac: Option<(u8, u16)>) {
        if self.session_id.is_none() {
            return;
        }
        let disposition = self.post_reorder.observe(sequence);
        self.mac.observe(disposition, mac, sequence);
        self.network_enqueued.observe(sequence);
        if sequence >= 0 {
            self.ledger.push(sequence as u32);
        }
    }

    pub fn dropped(&mut self, sequence: i32, mac: Option<(u8, u16)>, reason: NetworkDropReason) {
        if self.session_id.is_none() {
            return;
        }
        let disposition = self.post_reorder.observe(sequence);
        self.mac.observe(disposition, mac, sequence);
        match reason {
            NetworkDropReason::QueueFull => {
                self.network_queue_full = self.network_queue_full.saturating_add(1)
            }
            NetworkDropReason::PoolExhausted => {
                self.network_pool_exhausted = self.network_pool_exhausted.saturating_add(1)
            }
            NetworkDropReason::LinkDown => {
                self.network_link_down = self.network_link_down.saturating_add(1)
            }
            NetworkDropReason::InvalidLength => {
                self.network_invalid_length = self.network_invalid_length.saturating_add(1)
            }
        }
    }

    pub fn consumed(&mut self, session_id: u64, sequence: i32) {
        if self.session_id != Some(session_id) {
            return;
        }
        self.udp_consumer.observe(sequence);
        if sequence >= 0 {
            self.ledger.consume(sequence as u32);
        }
    }

    pub fn finish(
        &mut self,
        session_id: u64,
        reorder: RxReorderDeliveryEvidence,
    ) -> Option<RxDeliveryEvidence> {
        if self.session_id != Some(session_id) {
            return None;
        }
        self.session_id = None;
        self.completed_session_id = Some(session_id);
        let ledger = core::mem::take(&mut self.ledger).finish();
        Some(RxDeliveryEvidence {
            post_reorder: self.post_reorder.evidence,
            network_enqueued: self.network_enqueued.evidence,
            udp_consumer: self.udp_consumer.evidence,
            consumer_ledger: ledger,
            mac_order: self.mac.evidence,
            reorder,
            network_queue_full: self.network_queue_full,
            network_invalid_length: self.network_invalid_length,
            network_pool_exhausted: self.network_pool_exhausted,
            network_link_down: self.network_link_down,
        })
    }
}

impl<const LEDGER_CAPACITY: usize> Default for RxDeliveryTracker<LEDGER_CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
