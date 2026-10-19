use super::{LeDataLength, LeDataLengthMalformed, LeDataLengthProcedure, LeDataLengths};

fn length(octets: u16, time: u16) -> LeDataLength {
    LeDataLength::new(octets, time).unwrap()
}

fn body(rx: (u16, u16), tx: (u16, u16)) -> [u8; 8] {
    let mut body = [0; 8];
    body[0..2].copy_from_slice(&rx.0.to_le_bytes());
    body[2..4].copy_from_slice(&rx.1.to_le_bytes());
    body[4..6].copy_from_slice(&tx.0.to_le_bytes());
    body[6..8].copy_from_slice(&tx.1.to_le_bytes());
    body
}

#[test]
fn lengths_outside_the_specified_ranges_are_refused() {
    assert!(LeDataLength::new(26, 328).is_none());
    assert!(LeDataLength::new(252, 2120).is_none());
    assert!(LeDataLength::new(27, 327).is_none());
    assert!(LeDataLength::new(251, 17_041).is_none());
    assert_eq!(
        length(251, 17_040).supported(),
        LeDataLength::SUPPORTED_MAXIMUM
    );
}

#[test]
fn air_time_limits_the_1m_payload_and_counts_the_mic() {
    assert_eq!(LeDataLength::MINIMUM.payload_octets(0), 27);
    assert_eq!(LeDataLength::MINIMUM.payload_octets(4), 27);
    assert_eq!(LeDataLength::SUPPORTED_MAXIMUM.payload_octets(4), 251);
    // 1000 us fit 125 octets on air: 115 without a MIC, 111 with one.
    assert_eq!(length(251, 1_000).payload_octets(0), 115);
    assert_eq!(length(251, 1_000).payload_octets(4), 111);
}

#[test]
fn a_peer_request_is_answered_with_the_local_lengths_and_sets_the_effective_ones() {
    let mut procedure = LeDataLengthProcedure::new(LeDataLength::MINIMUM);
    let response = procedure
        .receive_request(&body((251, 2120), (251, 2120)))
        .unwrap();
    // Receive up to the supported maximum, send the local minimum.
    assert_eq!(response, [0x15, 251, 0, 0x48, 8, 27, 0, 0x48, 1]);
    assert_eq!(
        procedure.take_change(),
        Some(LeDataLengths {
            transmit: LeDataLength::MINIMUM,
            receive: LeDataLength::SUPPORTED_MAXIMUM,
        })
    );
    assert_eq!(procedure.take_change(), None);
    // The same announcement again changes nothing.
    procedure
        .receive_request(&body((251, 2120), (251, 2120)))
        .unwrap();
    assert_eq!(procedure.take_change(), None);
}

#[test]
fn a_peer_announcement_below_the_minimum_is_malformed_and_above_the_maximum_is_limited() {
    let mut procedure = LeDataLengthProcedure::new(LeDataLength::SUPPORTED_MAXIMUM);
    assert_eq!(
        procedure.receive_request(&body((26, 328), (27, 328))),
        Err(LeDataLengthMalformed)
    );
    assert_eq!(
        procedure.receive_request(&body((27, 328), (27, 328))[..7]),
        Err(LeDataLengthMalformed)
    );
    procedure
        .receive_request(&body((300, 20_000), (300, 20_000)))
        .unwrap();
    let effective = procedure.effective();
    assert_eq!(effective.transmit, LeDataLength::SUPPORTED_MAXIMUM);
    assert_eq!(effective.receive, LeDataLength::SUPPORTED_MAXIMUM);
}

#[test]
fn a_local_request_completes_with_the_peer_response_only_after_transmission() {
    let mut procedure = LeDataLengthProcedure::new(LeDataLength::MINIMUM);
    assert_eq!(procedure.queued_request(), None);
    procedure.set_transmit(length(251, 2120));
    assert_eq!(
        procedure.queued_request(),
        Some([0x14, 251, 0, 0x48, 8, 251, 0, 0x48, 8])
    );
    // An early response is not the answer to this request.
    procedure
        .receive_response(&body((251, 2120), (251, 2120)))
        .unwrap();
    assert_eq!(procedure.take_change(), None);
    procedure.request_enqueued();
    assert!(procedure.local_request_pending());
    procedure
        .receive_response(&body((100, 2120), (60, 2120)))
        .unwrap();
    assert!(!procedure.local_request_pending());
    let effective = procedure.take_change().unwrap();
    assert_eq!(effective.transmit, length(100, 2120));
    assert_eq!(effective.receive, length(60, 2120));
}

#[test]
fn an_unsupported_peer_leaves_the_minimum_lengths() {
    let mut procedure = LeDataLengthProcedure::new(LeDataLength::SUPPORTED_MAXIMUM);
    procedure.start_if_extended();
    assert!(procedure.queued_request().is_some());
    procedure.request_enqueued();
    procedure.abandon();
    assert!(!procedure.local_request_pending());
    assert_eq!(procedure.effective(), LeDataLengths::MINIMUM);
    assert_eq!(procedure.take_change(), None);
}

#[test]
fn a_minimum_suggestion_starts_no_procedure() {
    let mut procedure = LeDataLengthProcedure::new(LeDataLength::MINIMUM);
    procedure.start_if_extended();
    assert_eq!(procedure.queued_request(), None);
    procedure.set_transmit(LeDataLength::MINIMUM);
    assert_eq!(procedure.queued_request(), None);
}
