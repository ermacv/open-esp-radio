use super::*;
use crate::air::test_support::frame;

const AP: MacAddress = MacAddress([0x32, 0, 0, 0, 0, 1]);

fn probe(
    time_micros: u64,
    kind: FrameKind,
    source: MacAddress,
    receiver: MacAddress,
    sequence: u16,
) -> AirFrame {
    let mut record = frame(time_micros, kind);
    record.transmitter = Some(source);
    record.receiver = Some(receiver);
    record.sequence = Some(sequence);
    record.retry = Some(false);
    record
}

fn response(time_micros: u64, receiver: MacAddress, sequence: u16) -> AirFrame {
    probe(
        time_micros,
        FrameKind::PROBE_RESPONSE,
        AP,
        receiver,
        sequence,
    )
}

fn capture() -> Vec<AirFrame> {
    let mut frames = (0..model::REQUESTS)
        .map(|index| {
            let request = model::request(index).unwrap();
            probe(
                request.offset_us,
                FrameKind::PROBE_REQUEST,
                MacAddress(request.source),
                MacAddress::BROADCAST,
                index,
            )
        })
        .collect::<Vec<_>>();
    frames.push(response(1_001_000, FIXED_SOURCE, 40));
    frames.push(response(
        6_001_000,
        MacAddress([0x02, 0x4f, 0x45, 0x52, 0x00, 0x01]),
        41,
    ));
    frames
}

#[test]
fn full_plan_requires_air_evidence_and_responses_from_the_selected_ap() {
    assert!(Evidence::default().validate().is_err());
    assert!(analyze(&capture(), AP).unwrap().validate().is_ok());
    let other = MacAddress([2, 0, 0, 0, 0, 2]);
    assert!(analyze(&capture(), other).unwrap().validate().is_err());
    assert!(analyze(&capture()[1..], AP).unwrap().validate().is_err());
}

#[test]
fn retries_duplicates_and_invalid_metadata_fail_closed() {
    let mut retry = capture();
    retry.last_mut().unwrap().retry = Some(true);
    assert!(analyze(&retry, AP).unwrap().validate().is_err());
    let mut invalid = capture();
    invalid.last_mut().unwrap().retry = None;
    assert!(analyze(&invalid, AP).is_err());
    let mut unsequenced = capture();
    unsequenced.last_mut().unwrap().sequence = None;
    assert!(analyze(&unsequenced, AP).is_err());
    let mut duplicate = capture();
    duplicate.push(duplicate.last().unwrap().clone());
    assert!(analyze(&duplicate, AP).is_err());
    let mut forged = capture();
    forged[200].transmitter = Some(MacAddress([0x02, 0x4f, 0x45, 0x52, 0x00, 0x01]));
    assert!(analyze(&forged, AP).is_err());
}

#[test]
fn response_budget_is_global_across_receivers() {
    let mut frames = capture();
    for index in 0..106 {
        frames.push(response(
            3_000_000 + u64::from(index),
            MacAddress(model::request(201 + index).unwrap().source),
            100 + index,
        ));
    }
    let evidence = analyze(&frames, AP).unwrap();
    assert_eq!(evidence.maximum_responses_in_one_second, 106);
    assert!(evidence.validate().is_err());
}
