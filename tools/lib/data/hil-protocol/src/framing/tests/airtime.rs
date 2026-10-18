use super::*;
use crate::{wifi::WifiAirtimePeer, wifi::WifiAirtimePeerEvidence, wifi::WifiAirtimeReport};

#[test]
fn worst_case_airtime_records_fit_existing_wire_frames() {
    round_trip(Envelope::new(
        u64::MAX,
        u32::MAX,
        u64::MAX,
        u32::MAX,
        crate::wifi::AirtimePeer(WifiAirtimePeerEvidence {
            peer: WifiAirtimePeer::Unicast {
                address: [255; 6],
                association_id: u16::MAX,
                association_epoch: u32::MAX,
            },
            grants: u64::MAX,
            granted_micros: u64::MAX,
            settlements: u64::MAX,
            settled_grants_micros: u64::MAX,
            charged_micros: u64::MAX,
            cancellations: u64::MAX,
            cancelled_grants_micros: u64::MAX,
            balance_after_last_event_micros: i64::MIN,
            outstanding: u32::MAX,
            outstanding_micros: u64::MAX,
            maximum_outstanding: u32::MAX,
        }),
    ));
    round_trip(Envelope::new(
        u64::MAX,
        u32::MAX,
        u64::MAX,
        u32::MAX,
        crate::wifi::AirtimeReport(WifiAirtimeReport {
            peer_records: u8::MAX,
            dropped_events: u64::MAX,
            saturated: true,
        }),
    ));
}

#[test]
fn worst_case_aggregate_fill_record_fits_one_wire_frame() {
    let body = crate::wifi::AccessPointAggregateFill(crate::wifi::WifiApAggregateFill {
        generation: u32::MAX,
        association_id: u16::MAX,
        aggregates: u32::MAX,
        subframes: u32::MAX,
        maximum_subframes: u8::MAX,
        histogram: [u32::MAX; 5],
    });
    let expected = Envelope::new(u64::MAX, u32::MAX, u64::MAX, u32::MAX, body);
    let mut encoder = FrameEncoder::new();
    let mut decoder = FrameDecoder::new();
    let actual = receive(&mut decoder, encoder.encode(&expected).unwrap()).map(Result::unwrap);
    assert_eq!(actual, Some(expected));
}
