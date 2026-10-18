use std::boxed::Box;

use oer_esp32s31_ieee80211_dma::descriptor::rx_armed_word;

use super::*;
use crate::rx::{
    test_ring::{BUFFER, CountingSink, Ring},
    transaction::Unavailable,
};

#[test]
fn chained_frame_is_a_counted_drop_and_the_next_frame_is_received() {
    let counters: &'static WifiRxCounters = Box::leak(Box::new(WifiRxCounters::new()));
    let (sink, admission) = (
        CountingSink::new(counters),
        CountingAdmission::new(counters),
    );
    let mut ring = Ring::new();

    ring.complete(&[BUFFER as u32, 100]);
    ring.drain(&sink, &admission);
    let report = counters.report();
    assert_eq!((report.chained_drops, report.units), (1, 0));

    ring.complete(&[300]);
    ring.drain(&sink, &admission);
    let report = counters.report();
    assert_eq!(
        (report.units, report.bytes, report.chained_drops),
        (1, 300, 1)
    );

    // A chain longer than the length field is counted apart.
    let mut oversized = [BUFFER as u32; 10];
    oversized[9] = 1_500;
    assert!(rx_armed_word(oversized.iter().sum::<u32>()).is_none());
    ring.complete(&oversized);
    ring.complete(&[400]);
    ring.drain(&sink, &admission);
    let report = counters.report();
    assert_eq!(
        (
            report.units,
            report.bytes,
            report.chained_drops,
            report.oversized_drops
        ),
        (2, 700, 1, 1)
    );
    assert!(ring.live.topology_snapshot().valid);
}

#[test]
fn the_preview_classifies_by_frame_control() {
    let counters = WifiRxCounters::new();
    let unit = CompletedUnit {
        head_index: 0,
        descriptor_count: 1,
        payload_length: 64,
    };
    let mut header = [0; 24];
    header[..2].copy_from_slice(&0x4088_u16.to_le_bytes());
    let preview = frame_control_preview(unit, header);
    assert_eq!(preview.frame_control, Some(0x4088));
    assert_eq!(preview.class, IngressClass::BulkProtectedData);
    assert_eq!(preview.route, IngressRoute::Standalone);
    // A beacon stays critical.
    header[..2].copy_from_slice(&0x0080_u16.to_le_bytes());
    assert_eq!(
        frame_control_preview(unit, header).class,
        IngressClass::Critical
    );
    assert_eq!(unclassified_preview(unit).class, IngressClass::Unclassified);
    assert_eq!(unclassified_preview(unit).route, IngressRoute::Malformed);
    // Admission keeps the default reserve and overload policy.
    let admission = CountingAdmission::new(&counters);
    assert_eq!(
        admission
            .credit_policy()
            .validate(2, 2)
            .unwrap()
            .critical_reserved_credits(),
        1
    );
    assert_eq!(
        admission.unavailable_disposition(preview),
        Unavailable::DiscardAndRecycle
    );
}

#[test]
fn admission_counts_every_drop_by_its_reason() {
    let counters = WifiRxCounters::new();
    let admission = CountingAdmission::new(&counters);
    let unit = CompletedUnit {
        head_index: 0,
        descriptor_count: 1,
        payload_length: 0,
    };
    let preview = Preview {
        unit,
        frame_control: None,
        class: IngressClass::Unclassified,
        route: IngressRoute::Malformed,
    };
    for reason in [
        Discard::Empty,
        Discard::TooLong,
        Discard::Chained,
        Discard::Oversized,
        Discard::OverloadBulk,
    ] {
        admission.observe(Observation::DiscardRetained { unit, reason });
    }
    admission.observe(Observation::OverloadDiscardedAndRecycled(preview));
    admission.observe(Observation::BulkAdmissionBlocked(preview));
    admission.observe(Observation::CriticalAdmissionBlocked(preview));
    admission.observe(Observation::Staged(unit));
    admission.observe(Observation::CriticalReserveAdmitted(preview));
    let report = counters.report();
    assert_eq!(
        (
            report.empty_drops,
            report.too_long_drops,
            report.chained_drops,
            report.oversized_drops,
            report.overload_drops,
            report.admission_blocks,
            report.units,
        ),
        (1, 1, 1, 1, 2, 2, 0)
    );
}
