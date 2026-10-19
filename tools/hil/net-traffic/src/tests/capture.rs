//! Sessions and readiness over a fake serial link to the target.

use std::{
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    thread,
    time::Duration,
};

use oer_hil_link::{error::LinkError, test_support::*};
use oer_hil_protocol::{
    Envelope,
    base::LinkHealth,
    network::WifiNetworkInterface,
    network::{
        Direction, EvidenceRecord, Finished, FlowTransportEvidence, Transport, TransportEvidence,
        evidence_crc32c,
    },
    system::StackUsage,
};

use crate::{NetworkSession as _, SessionHandle, prepare_udp_reverse_flow};

fn is_link_failure(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut cause = Some(error);
    while let Some(error) = cause {
        if error.is::<LinkError>() {
            return true;
        }
        cause = error.source();
    }
    false
}

#[test]
fn real_rx_probe_can_pass_without_any_target_to_host_payload() {
    use oer_hil_protocol::{
        network::Direction, network::NetworkInfo, network::ServiceInfo, network::Transport,
        network::WifiNetworkInterface,
    };

    let output = Output::new();
    let (capture, input) = capture(&output, false);
    activate(&capture, &input);
    let receiver = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
    receiver
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let rx_port = receiver.local_addr().unwrap().port();
    for (sequence, event) in [
        (
            1,
            any(oer_hil_protocol::network::Ready(NetworkInfo {
                network_interface: WifiNetworkInterface::Station,
                address: Ipv4Addr::LOCALHOST.octets(),
                prefix_length: 8,
                gateway: None,
            })),
        ),
        (
            2,
            any(oer_hil_protocol::network::ServiceReady(ServiceInfo {
                network_interface: WifiNetworkInterface::Station,
                transport: Transport::Udp,
                direction: Direction::Rx,
                local_port: rx_port,
                maximum_payload_bytes: 64,
            })),
        ),
        (
            3,
            any(oer_hil_protocol::network::ServiceReady(ServiceInfo {
                network_interface: WifiNetworkInterface::Station,
                transport: Transport::Udp,
                direction: Direction::Tx,
                local_port: 4_324,
                maximum_payload_bytes: 64,
            })),
        ),
    ] {
        input.send(Ok(event.frame(7, sequence, 0, 0))).unwrap();
    }
    let target_input = input.clone();
    let target = thread::spawn(move || {
        let mut buffer = [0; 64];
        let (length, _) = receiver.recv_from(&mut buffer).unwrap();
        assert_eq!(length, 64);
        assert_eq!(&buffer[..4], &(-1_i32).to_be_bytes());
        target_input
            .send(Ok(frame(Envelope::new(
                7,
                4,
                0,
                0,
                oer_hil_protocol::network::ServiceReady(ServiceInfo {
                    network_interface: WifiNetworkInterface::Station,
                    transport: Transport::Udp,
                    direction: Direction::Rx,
                    local_port: rx_port,
                    maximum_payload_bytes: 64,
                }),
            ))))
            .unwrap();
    });
    let ready = crate::probe_udp_rx_ready(
        &capture,
        Ipv4Addr::LOCALHOST,
        rx_port,
        Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!(ready.address, Ipv4Addr::LOCALHOST);
    target.join().unwrap();
    // There was no reverse UDP payload, session identity, or TX completion.
}

#[test]
fn target_session_failure_does_not_turn_into_an_evidence_timeout() {
    let output = Output::new();
    let (capture, input) = capture(&output, false);
    activate(&capture, &input);
    input
        .send(Ok(frame(Envelope::new(
            7,
            1,
            9,
            0,
            oer_hil_protocol::network::Failed(oer_hil_protocol::network::FailureCode::Network),
        ))))
        .unwrap();
    let session = SessionHandle {
        session_id: 9,
        first_event: 1,
        flow_ids: [Some(0), None],
    };
    let error = capture
        .wait_for_session(session, Duration::from_secs(3))
        .unwrap_err();
    assert_eq!(error.to_string(), "target session 9 failed: Network");
    assert!(!is_link_failure(&*error));
}

fn result_events(rx_frames: u32) -> Vec<AnyMessage> {
    use oer_hil_protocol::{network::ResultSummary, system::StackWatermark};
    let transport = TransportEvidence {
        rx_maximum_silence_micros: None,
        rx_bytes: 8,
        tx_bytes: 0,
        rx_units: 2,
        tx_units: 0,
        rx_late_bytes: 0,
        rx_late_units: 0,
        elapsed_micros: 100,
        transport_errors: 0,
    };
    let watermark = StackWatermark {
        capacity_bytes: 100,
        free_bytes: 90,
        used_bytes: 10,
        minimum_free_bytes: 20,
    };
    let records = [
        EvidenceRecord::Transport(transport),
        EvidenceRecord::FlowTransport(FlowTransportEvidence::from_session_total(0, transport)),
        EvidenceRecord::Link(LinkHealth {
            rx_frames,
            rx_cobs_errors: 0,
            rx_checksum_errors: 0,
            rx_decode_errors: 0,
            rx_overflows: 0,
            tx_frames: 5,
            tx_dropped: 0,
            text_dropped: 0,
            text_truncated: 0,
        }),
        EvidenceRecord::Stack(StackUsage {
            cpu0_irq: None,
            cpu1_irq: None,
            cpu0: watermark,
            cpu1: watermark,
        }),
    ];
    let finished = Finished {
        summary: ResultSummary {
            verdict: oer_hil_protocol::network::SessionVerdict::Passed,
            evidence_records: 4,
        },
        evidence_crc32c: evidence_crc32c(&records).unwrap(),
    };
    records
        .into_iter()
        .map(|record| any(oer_hil_protocol::network::Evidence(record)))
        .chain([any(finished)])
        .collect()
}

fn publish_result(input: &Input, sequence: u32, request: u32, rx_frames: u32) {
    for (offset, event) in result_events(rx_frames).into_iter().enumerate() {
        input
            .send(Ok(event.frame(7, sequence + offset as u32, 9, request)))
            .unwrap();
    }
}

fn replay_before_acknowledgement(changed: bool) {
    let output = Output::new();
    let (capture, input, commands) = capture_with_commands(&output);
    activate(&capture, &input);
    publish_result(&input, 1, 0, 5);
    let session = SessionHandle {
        session_id: 9,
        first_event: 1,
        flow_ids: [Some(0), None],
    };
    capture
        .wait_for_session(session, Duration::from_secs(2))
        .unwrap();
    let input_guard = input.clone();
    let target = thread::spawn(move || {
        let (replay, oer_hil_protocol::network::ReplayResult) = receive(&commands);
        assert_eq!(replay.session_id, 9);
        // Model live link counters changing between the first result and its
        // replay: the historical target bug changed both evidence and CRC.
        publish_result(&input, 6, replay.request_id, if changed { 6 } else { 5 });
        if !changed {
            let (ack, oer_hil_protocol::network::AcknowledgeResult) = receive(&commands);
            input
                .send(Ok(frame(Envelope::new(
                    7,
                    11,
                    9,
                    ack.request_id,
                    oer_hil_protocol::base::Accepted,
                ))))
                .unwrap();
        }
    });
    let result = capture.acknowledge_session(session);
    if changed {
        let error = result.unwrap_err();
        assert!(error.to_string().contains("changed the retained result"));
        assert!(is_link_failure(&*error));
    } else {
        result.unwrap();
    }
    target.join().unwrap();
    drop(capture);
    drop(input_guard);
}

#[test]
fn acknowledgement_requires_an_identical_replay() {
    replay_before_acknowledgement(false);
}

#[test]
fn changed_replay_is_rejected_before_result_removal() {
    replay_before_acknowledgement(true);
}

#[test]
fn reverse_probe_requires_network_and_bound_service_on_the_same_interface() {
    use oer_hil_protocol::{network::NetworkInfo, network::ServiceInfo};
    let output = Output::new();
    let (capture, input) = capture(&output, false);
    activate(&capture, &input);
    let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    peer.set_read_timeout(Some(Duration::from_millis(20)))
        .unwrap();
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    socket.connect(peer.local_addr().unwrap()).unwrap();
    let network = |interface| {
        oer_hil_protocol::network::Ready(NetworkInfo {
            network_interface: interface,
            address: [10, 43, 0, 1],
            prefix_length: 24,
            gateway: None,
        })
    };
    let source_port = peer.local_addr().unwrap().port();
    let service = |local_port| {
        oer_hil_protocol::network::ServiceReady(ServiceInfo {
            network_interface: WifiNetworkInterface::AccessPoint,
            transport: Transport::Udp,
            direction: Direction::Tx,
            local_port,
            maximum_payload_bytes: 1472,
        })
    };
    for (sequence, event) in [
        (1, any(service(source_port.wrapping_add(1)))),
        (2, any(network(WifiNetworkInterface::Station))),
    ] {
        input.send(Ok(event.frame(7, sequence, 0, 0))).unwrap();
    }
    capture
        .wait_for_message_after(0, Duration::from_secs(1), |event| {
            event.message_sequence == 2
        })
        .unwrap()
        .unwrap();
    assert!(
        prepare_udp_reverse_flow(
            &capture,
            WifiNetworkInterface::AccessPoint,
            &socket,
            Duration::ZERO
        )
        .is_err()
    );
    assert!(
        peer.recv(&mut [0; 4]).is_err(),
        "neither service alone nor another interface's IP permits a probe"
    );
    input
        .send(Ok(frame(Envelope::new(
            7,
            3,
            0,
            0,
            network(WifiNetworkInterface::AccessPoint),
        ))))
        .unwrap();
    capture
        .wait_for_message_after(0, Duration::from_secs(1), |event| {
            event.message_sequence == 3
        })
        .unwrap()
        .unwrap();
    assert!(
        prepare_udp_reverse_flow(
            &capture,
            WifiNetworkInterface::AccessPoint,
            &socket,
            Duration::ZERO
        )
        .is_err()
    );
    assert!(
        peer.recv(&mut [0; 4]).is_err(),
        "another TX port must not authorize this flow"
    );
    input
        .send(Ok(frame(Envelope::new(7, 4, 0, 0, service(source_port)))))
        .unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    let responder = std::thread::spawn(move || {
        let mut bytes = [0; oer_hil_protocol::network::UdpProbe::LENGTH];
        let (length, source) = peer.recv_from(&mut bytes).unwrap();
        let mut probe = oer_hil_protocol::network::UdpProbe::decode(&bytes[..length]).unwrap();
        assert!(!probe.response);
        probe.response = true;
        peer.send_to(&probe.encode(), source).unwrap();
    });
    prepare_udp_reverse_flow(
        &capture,
        WifiNetworkInterface::AccessPoint,
        &socket,
        Duration::from_secs(1),
    )
    .unwrap();
    responder.join().unwrap();
    // Already received declarations are state, not an edge that must happen again.
    super::super::readiness::wait_for_services(
        &capture,
        WifiNetworkInterface::AccessPoint,
        &[(Transport::Udp, Direction::Tx, source_port)],
        Duration::ZERO,
    )
    .unwrap();
    drop(capture);
}
