use super::WifiNetworkInterface;
use super::*;

fn flow(flow_id: u8, address: [u8; 4], port: u16) -> SessionFlowConfig {
    let traffic = FlowConfig {
        payload_bytes: 1_472,
        offered_rate_bps: Some(60_000_000),
        pacing_group_datagrams: None,
    };
    SessionFlowConfig {
        flow_id,
        peer: Some(Ipv4Endpoint { address, port }),
        target_rx: Some(traffic),
        target_tx: Some(traffic),
        payload_identity: None,
    }
}

fn two_flow_session() -> SessionConfig {
    SessionConfig {
        network_interface: WifiNetworkInterface::AccessPoint,
        transport: Transport::Udp,
        direction: Direction::Bidirectional,
        completion: Completion::DurationMillis(10_000),
        flows: [
            Some(flow(3, [192, 168, 4, 2], 9_002)),
            Some(flow(9, [192, 168, 4, 3], 9_003)),
        ],
        link_requirements: SessionLinkRequirements::NONE,
    }
}

#[test]
fn multi_flow_structure_requires_the_explicit_property() {
    let session = two_flow_session();
    assert!(!session.structurally_valid(1_472, false));
    assert!(session.structurally_valid(1_472, true));
}

#[test]
fn udp_stage_payload_identity_rejects_stale_or_short_application_payload() {
    let identity = UdpSessionPayloadIdentity::new(0x7a45_0123_fedc_9001);
    let stale = UdpSessionPayloadIdentity::new(0x7a45_0123_fedc_9002);
    let mut payload = [0x5a; 64];
    payload[..4].copy_from_slice(&3_u32.to_be_bytes());
    assert!(identity.write_to(&mut payload));
    assert!(identity.matches(&payload));
    assert!(!stale.matches(&payload));
    assert_eq!(
        UdpSessionPayloadIdentity::from_payload(&payload),
        Some(identity)
    );
    assert!(!identity.matches(&payload[..11]));
    assert!(!identity.write_to(&mut payload[..11]));
    assert!(identity.accepts_next_data(Some(identity), true, Some(3), 3));
    assert!(!identity.accepts_next_data(Some(stale), true, Some(3), 3));
    assert!(!identity.accepts_next_data(Some(identity), false, Some(3), 3));
    assert!(!identity.accepts_next_data(Some(identity), true, Some(2), 3));
    assert!(!identity.accepts_next_data(Some(identity), true, None, 3));
    payload[12] = 0;
    assert!(!identity.matches(&payload));

    let mut session = two_flow_session();
    session.flows[0].as_mut().unwrap().payload_identity = Some(identity);
    assert!(session.structurally_valid(1_472, true));
    session.transport = Transport::Tcp;
    assert!(!session.structurally_valid(1_472, true));
}

#[test]
fn per_flow_pacing_is_nonzero_and_owned_only_by_udp_target_tx() {
    let mut session = two_flow_session();
    session.flows[1]
        .as_mut()
        .unwrap()
        .target_tx
        .as_mut()
        .unwrap()
        .pacing_group_datagrams = Some(2);
    assert!(session.structurally_valid(1_472, true));

    session.flows[1]
        .as_mut()
        .unwrap()
        .target_tx
        .as_mut()
        .unwrap()
        .pacing_group_datagrams = Some(0);
    assert!(!session.structurally_valid(1_472, true));

    session.flows[1]
        .as_mut()
        .unwrap()
        .target_tx
        .as_mut()
        .unwrap()
        .pacing_group_datagrams = None;
    session.flows[1]
        .as_mut()
        .unwrap()
        .target_rx
        .as_mut()
        .unwrap()
        .pacing_group_datagrams = Some(2);
    assert!(!session.structurally_valid(1_472, true));
}

#[test]
fn multi_flow_structure_rejects_ambiguous_identities() {
    let mut duplicate_id = two_flow_session();
    duplicate_id.flows[1].as_mut().unwrap().flow_id = 3;
    assert!(!duplicate_id.structurally_valid(1_472, true));

    let mut duplicate_peer = two_flow_session();
    duplicate_peer.flows[1].as_mut().unwrap().peer = duplicate_peer.flows[0].unwrap().peer;
    assert!(!duplicate_peer.structurally_valid(1_472, true));
}

#[test]
fn multi_flow_structure_rejects_missing_peer_or_direction() {
    let mut missing_peer = two_flow_session();
    missing_peer.flows[1].as_mut().unwrap().peer = None;
    assert!(!missing_peer.structurally_valid(1_472, true));

    let mut missing_rx = two_flow_session();
    missing_rx.flows[1].as_mut().unwrap().target_rx = None;
    assert!(!missing_rx.structurally_valid(1_472, true));
}

#[test]
fn silence_keeps_single_rx_flow_scope_through_transport_projection() {
    let one = FlowTransportEvidence {
        flow_id: 0,
        rx_maximum_silence_micros: Some(3_000_000),
        rx_bytes: 1200,
        tx_bytes: 0,
        rx_units: 1,
        tx_units: 0,
        rx_late_bytes: 0,
        rx_late_units: 0,
        elapsed_micros: 12_000_000,
        transport_errors: 0,
    };
    assert_eq!(
        one.as_session_total().rx_maximum_silence_micros,
        Some(3_000_000)
    );
    assert_eq!(
        TransportEvidence::from_flows([Some(one), None]).rx_maximum_silence_micros,
        Some(3_000_000)
    );
    let two = FlowTransportEvidence { flow_id: 1, ..one };
    assert_eq!(
        TransportEvidence::from_flows([Some(one), Some(two)]).rx_maximum_silence_micros,
        None
    );
}

#[test]
fn missing_second_flow_observation_cannot_qualify_whole_session_continuity() {
    let first = FlowTransportEvidence {
        flow_id: 0,
        rx_maximum_silence_micros: Some(0),
        rx_bytes: 1200,
        tx_bytes: 0,
        rx_units: 1,
        tx_units: 0,
        rx_late_bytes: 0,
        rx_late_units: 0,
        elapsed_micros: 12_000_000,
        transport_errors: 0,
    };
    let second = FlowTransportEvidence {
        flow_id: 1,
        rx_maximum_silence_micros: None,
        ..first
    };
    assert_eq!(
        TransportEvidence::from_flows([Some(first), Some(second)]).rx_maximum_silence_micros,
        None
    );
}

#[test]
fn a_failed_session_carries_its_first_failed_check_over_the_wire() {
    extern crate std;
    use std::string::ToString as _;
    let finished = Finished {
        summary: ResultSummary {
            verdict: SessionVerdict::Failed(SessionFailure::NoTerminal {
                received: 41_000,
                highest_sequence: 41_950,
            }),
            evidence_records: 7,
        },
        evidence_crc32c: 0x1234_5678,
    };
    let mut buffer = [0; 64];
    let encoded = postcard::to_slice(&finished, &mut buffer).unwrap();
    let decoded: Finished = postcard::from_bytes(encoded).unwrap();
    assert_eq!(decoded, finished);
    assert!(!decoded.summary.verdict.passed());
    assert_eq!(
        decoded.summary.verdict.to_string(),
        "failed: no terminal marker (received 41000, highest sequence 41950)"
    );
    assert!(SessionVerdict::Passed.passed());
    assert_eq!(SessionVerdict::Passed.failure(), None);
}
