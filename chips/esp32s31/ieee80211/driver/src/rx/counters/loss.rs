//! Independent, saturating loss accounting. Each row has one writer (P2);
//! P1 only reads snapshots. Atomic load/store needs no retry loop or lock.
use crate::rx::{
    transaction::{Discard, Hooks},
    upper::W1_INGRESS,
};
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use oer_esp32s31_ieee80211_mac::rx::{RxError, RxSegment, view_normalized_rx_frame};
use oer_ieee80211_wire::data::traffic::{Traffic, TrafficCounts};

#[derive(bytemuck::Zeroable, Default)]
struct AtomicTraffic {
    counts: [AtomicU32; 8],
}
impl AtomicTraffic {
    const fn new() -> Self {
        Self {
            counts: [const { AtomicU32::new(0) }; 8],
        }
    }
    fn record(&self, traffic: Traffic) {
        let index = match traffic {
            Traffic::EchoRequest => 0,
            Traffic::EchoReply => 1,
            Traffic::OtherIcmp => 2,
            Traffic::OtherIpv4 => 3,
            Traffic::Arp => 4,
            Traffic::Ipv6 => 5,
            Traffic::Other => 6,
            Traffic::Unidentified => 7,
        };
        if let Some(counter) = self.counts.get(index) {
            counter.store(
                counter.load(Ordering::Relaxed).saturating_add(1),
                Ordering::Relaxed,
            );
        }
    }
    fn snapshot(&self) -> TrafficCounts {
        let [
            echo_request,
            echo_reply,
            other_icmp,
            other_ipv4,
            arp,
            ipv6,
            other,
            unidentified,
        ] = self
            .counts
            .each_ref()
            .map(|value| value.load(Ordering::Relaxed));
        TrafficCounts {
            echo_request,
            echo_reply,
            other_icmp,
            other_ipv4,
            arp,
            ipv6,
            other,
            unidentified,
        }
    }
}

/// Exclusive terminal outcomes at DMA admission and the protocol port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxPortOutcome {
    Delivered,
    Filtered,
    Prefix(RxError),
}

#[derive(bytemuck::Zeroable, Default)]
pub struct RxLossCounters {
    dma: AtomicTraffic,
    staged: AtomicTraffic,
    chained: AtomicTraffic,
    length: AtomicTraffic,
    pool: AtomicTraffic,
    prefix: AtomicTraffic,
    integrity: AtomicTraffic,
    filtered: AtomicTraffic,
    delivered: AtomicTraffic,
    retry_echo: AtomicU32,
    hardware_previous: [AtomicU32; 8],
    hardware_total: [AtomicU32; 8],
    hardware_seen: AtomicBool,
    retired: AtomicBool,
    pending: AtomicU32,
    reserved: AtomicU32,
    scoped: AtomicBool,
    local: [AtomicU8; 6],
    bssid: [AtomicU8; 6],
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxLossReport {
    pub dma: TrafficCounts,
    pub staged: TrafficCounts,
    pub chained: TrafficCounts,
    pub length: TrafficCounts,
    pub pool: TrafficCounts,
    pub prefix: TrafficCounts,
    pub integrity: TrafficCounts,
    pub filtered: TrafficCounts,
    pub delivered: TrafficCounts,
    /// Recognizable echoes carrying MAC Retry; not presumed to be duplicates.
    pub retry_echo: u32,
    /// MPDU, END, data-success, FCS, abort, other-unicast, buffer-full, FIFO-overflow.
    pub hardware: [u32; 8],
    pub retired: bool,
    pub pending: u32,
    pub reserved: u32,
    pub scoped: bool,
}
impl RxLossCounters {
    pub const fn new() -> Self {
        Self {
            dma: AtomicTraffic::new(),
            staged: AtomicTraffic::new(),
            chained: AtomicTraffic::new(),
            length: AtomicTraffic::new(),
            pool: AtomicTraffic::new(),
            prefix: AtomicTraffic::new(),
            integrity: AtomicTraffic::new(),
            filtered: AtomicTraffic::new(),
            delivered: AtomicTraffic::new(),
            retry_echo: AtomicU32::new(0),
            hardware_previous: [const { AtomicU32::new(0) }; 8],
            hardware_total: [const { AtomicU32::new(0) }; 8],
            hardware_seen: AtomicBool::new(false),
            retired: AtomicBool::new(false),
            pending: AtomicU32::new(0),
            reserved: AtomicU32::new(0),
            scoped: AtomicBool::new(false),
            local: [const { AtomicU8::new(0) }; 6],
            bssid: [const { AtomicU8::new(0) }; 6],
        }
    }
    pub fn report(&self) -> RxLossReport {
        RxLossReport {
            dma: self.dma.snapshot(),
            staged: self.staged.snapshot(),
            chained: self.chained.snapshot(),
            length: self.length.snapshot(),
            pool: self.pool.snapshot(),
            prefix: self.prefix.snapshot(),
            integrity: self.integrity.snapshot(),
            filtered: self.filtered.snapshot(),
            delivered: self.delivered.snapshot(),
            retry_echo: self.retry_echo.load(Ordering::Relaxed),
            retired: self.retired.load(Ordering::Relaxed),
            pending: self.pending.load(Ordering::Relaxed),
            reserved: self.reserved.load(Ordering::Relaxed),
            scoped: self.scoped.load(Ordering::Acquire),
            hardware: self
                .hardware_total
                .each_ref()
                .map(|v| v.load(Ordering::Relaxed)),
        }
    }
    /// Select the image's echo peer before starting RX. This filters only
    /// diagnostic echo classes, never MAC admission or buffer ownership.
    pub(crate) fn scope_station(&self, local: [u8; 6], bssid: [u8; 6]) {
        for (place, byte) in self.local.iter().zip(local) {
            place.store(byte, Ordering::Relaxed);
        }
        for (place, byte) in self.bssid.iter().zip(bssid) {
            place.store(byte, Ordering::Relaxed);
        }
        self.scoped.store(true, Ordering::Release);
    }
    fn classify(&self, segment: Option<RxSegment<'_>>) -> Traffic {
        let Some(frame) = segment.and_then(|s| view_normalized_rx_frame(&s, W1_INGRESS).ok())
        else {
            return Traffic::Unidentified;
        };
        self.classify_mpdu(frame.mpdu)
    }
    fn classify_mpdu(&self, bytes: &[u8]) -> Traffic {
        let traffic = Traffic::mpdu(bytes);
        if matches!(traffic, Traffic::EchoRequest | Traffic::EchoReply)
            && self.scoped.load(Ordering::Acquire)
        {
            let local = self.local.each_ref().map(|v| v.load(Ordering::Relaxed));
            let bssid = self.bssid.each_ref().map(|v| v.load(Ordering::Relaxed));
            if !oer_ieee80211_wire::frame::DecodedHeader::decode(bytes).is_some_and(|h| {
                h.receiver() == local && h.transmitter() == bssid && h.control().ds() == 2
            }) {
                return Traffic::Other;
            }
        }
        traffic
    }
    pub(crate) fn retirement(&self, pending: usize, reserved: u32) {
        self.pending.store(
            u32::try_from(pending).unwrap_or(u32::MAX),
            Ordering::Relaxed,
        );
        self.reserved.store(reserved, Ordering::Relaxed);
        self.retired.store(true, Ordering::Relaxed);
    }
    pub(crate) fn hardware(&self, sample: oer_esp32s31_hal::types::MacRxPrimaryStatistics) {
        let values = [
            sample.mpdu_count,
            sample.end,
            sample.data_success,
            sample.fcs_error,
            sample.abort,
            sample.other_unicast,
            sample.buffer_full,
            sample.fifo_overflow,
        ];
        self.hardware_values(values);
    }
    fn hardware_values(&self, values: [u16; 8]) {
        let seen = self.hardware_seen.load(Ordering::Relaxed);
        for ((previous, total), value) in self
            .hardware_previous
            .iter()
            .zip(&self.hardware_total)
            .zip(values)
        {
            if seen {
                let delta = value.wrapping_sub(previous.load(Ordering::Relaxed) as u16);
                total.store(
                    total
                        .load(Ordering::Relaxed)
                        .saturating_add(u32::from(delta)),
                    Ordering::Relaxed,
                );
            }
            previous.store(u32::from(value), Ordering::Relaxed);
        }
        self.hardware_seen.store(true, Ordering::Relaxed);
    }
    pub(crate) fn port(&self, segment: RxSegment<'_>, outcome: RxPortOutcome) {
        let traffic = self.classify(Some(segment));
        let row = match outcome {
            RxPortOutcome::Delivered => &self.delivered,
            RxPortOutcome::Filtered => &self.filtered,
            RxPortOutcome::Prefix(RxError::MicFailure | RxError::Quarantined) => &self.integrity,
            RxPortOutcome::Prefix(_) => &self.prefix,
        };
        row.record(traffic);
    }
}
impl Hooks for &RxLossCounters {
    fn unit_taken(&self, segment: Option<RxSegment<'_>>, discard: Option<Discard>) {
        let traffic = self.classify(segment);
        self.dma.record(traffic);
        if traffic == Traffic::EchoRequest
            && segment
                .and_then(|s| view_normalized_rx_frame(&s, W1_INGRESS).ok())
                .and_then(|f| oer_ieee80211_wire::frame::DecodedHeader::decode(f.mpdu))
                .is_some_and(|h| h.control().retry())
        {
            self.retry_echo.store(
                self.retry_echo.load(Ordering::Relaxed).saturating_add(1),
                Ordering::Relaxed,
            );
        }
        let row = match discard {
            None => &self.staged,
            Some(Discard::Chained | Discard::Oversized) => &self.chained,
            Some(Discard::Empty | Discard::TooLong) => &self.length,
            Some(Discard::OverloadBulk) => &self.pool,
        };
        row.record(traffic);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn foreign_echoes_never_inflate_the_scoped_dma_request_count() {
        let local = [2, 0, 0, 0, 0, 1];
        let bssid = [2, 0, 0, 0, 0, 2];
        let mut ip = [0; 28];
        ip[0] = 0x45;
        ip[2..4].copy_from_slice(&28u16.to_be_bytes());
        ip[9] = 1;
        ip[20] = 8;
        let loss = RxLossCounters::new();
        loss.scope_station(local, bssid);
        for (destination, expected) in [
            (local, Traffic::EchoRequest),
            ([2, 0, 0, 0, 0, 3], Traffic::Other),
        ] {
            let mut bytes = [0; 128];
            let len = oer_ieee80211_wire::ap::ApDataFrame {
                access_point: bssid,
                destination,
                sequence_number: oer_ieee80211_wire::sequence::SequenceNumber::new(1).unwrap(),
                ether_type: 0x0800,
                payload: &ip,
            }
            .encode(&mut bytes)
            .unwrap();
            assert_eq!(loss.classify_mpdu(&bytes[..len]), expected);
        }
    }
    #[test]
    fn hardware_wrap_is_accumulated_and_retirement_does_not_invent_an_empty_ring() {
        let loss = RxLossCounters::new();
        assert!(!loss.report().retired);
        loss.hardware_values([65534; 8]);
        loss.hardware_values([3; 8]);
        assert_eq!(loss.report().hardware, [5; 8]);
        loss.hardware_total[0].store(u32::MAX - 1, Ordering::Relaxed);
        loss.hardware_values([7; 8]);
        assert_eq!(loss.report().hardware[0], u32::MAX);
        loss.retirement(2, 1);
        assert_eq!(
            (
                loss.report().retired,
                loss.report().pending,
                loss.report().reserved
            ),
            (true, 2, 1)
        );
    }
    #[test]
    fn new_counts_saturate_independently_of_the_legacy_wrapping_counts() {
        let counts = AtomicTraffic::new();
        counts.counts[0].store(u32::MAX - 1, Ordering::Relaxed);
        for _ in 0..3 {
            counts.record(Traffic::EchoRequest);
        }
        counts.record(Traffic::Arp);
        assert_eq!(counts.snapshot().echo_request, u32::MAX);
        assert_eq!(counts.snapshot().arp, 1);
        let loss = RxLossCounters::new();
        (&loss).unit_taken(None, Some(Discard::Chained));
        assert_eq!(loss.report().dma.unidentified, 1);
        assert_eq!(loss.report().chained.unidentified, 1);
        assert_eq!(loss.report().staged.total(), 0);
    }
}
