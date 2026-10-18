use super::*;
use oer_hil_protocol::{
    base::LinkHealth, network::Finished, network::FlowTransportEvidence, network::ResultSummary,
    network::TransportEvidence, system::StackUsage, system::StackWatermark,
};

fn loopback_flow() -> (UdpSocket, UdpSocket) {
    let sink = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
    let source = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
    sink.connect(source.local_addr().unwrap()).unwrap();
    source.connect(sink.local_addr().unwrap()).unwrap();
    (sink, source)
}

fn packet(identity: UdpSessionPayloadIdentity, sequence: u32) -> [u8; PAYLOAD_BYTES] {
    let mut packet = [0x5a; PAYLOAD_BYTES];
    packet[..4].copy_from_slice(&sequence.to_be_bytes());
    assert!(identity.write_to(&mut packet));
    packet
}

fn target_evidence(rx_units: u64, tx_units: u64, passed: bool) -> SessionEvidence {
    let transport = TransportEvidence {
        rx_maximum_silence_micros: None,
        rx_bytes: rx_units * PAYLOAD_BYTES as u64,
        tx_bytes: tx_units * PAYLOAD_BYTES as u64,
        rx_units,
        tx_units,
        rx_late_bytes: 0,
        rx_late_units: 0,
        elapsed_micros: 3_000_000,
        transport_errors: 0,
    };
    let watermark = StackWatermark {
        capacity_bytes: 1,
        free_bytes: 1,
        used_bytes: 0,
        minimum_free_bytes: 1,
    };
    SessionEvidence {
        transport,
        flow_transport: [
            Some(FlowTransportEvidence::from_session_total(0, transport)),
            None,
        ],
        radio: None,
        tx_timing: None,
        rx_delivery: None,
        network_scheduler: None,
        rx_zero_copy: None,
        cpu_load: None,
        stack: StackUsage {
            cpu0_irq: None,
            cpu1_irq: None,
            cpu0: watermark,
            cpu1: watermark,
        },
        link: LinkHealth {
            rx_frames: 1,
            rx_cobs_errors: 0,
            rx_checksum_errors: 0,
            rx_decode_errors: 0,
            rx_overflows: 0,
            tx_frames: 1,
            tx_dropped: 0,
            text_dropped: 0,
            text_truncated: 0,
        },
        finished: Finished {
            summary: ResultSummary {
                verdict: if passed {
                    oer_hil_protocol::network::SessionVerdict::Passed
                } else {
                    oer_hil_protocol::network::SessionVerdict::Failed(
                        oer_hil_protocol::network::SessionFailure::NoDatagrams,
                    )
                },
                evidence_records: 4,
            },
            evidence_crc32c: 0,
        },
    }
}

#[test]
fn bounded_loopback_host_sink_requires_exact_new_session_content_and_sequence() {
    let identity = UdpSessionPayloadIdentity::new(23);
    let timeout = Duration::from_millis(75);
    let (sink, source) = loopback_flow();
    for sequence in 0..4 {
        source.send(&packet(identity, sequence)).unwrap();
    }
    assert_eq!(
        receive_tx_payloads(&sink, identity, 4, timeout).unwrap(),
        HostReceipt {
            datagrams: 4,
            bytes: 4 * PAYLOAD_BYTES as u64,
        }
    );

    let (sink, source) = loopback_flow();
    source
        .send(&packet(UdpSessionPayloadIdentity::new(22), 0))
        .unwrap();
    assert!(receive_tx_payloads(&sink, identity, 1, timeout).is_err());

    let (sink, source) = loopback_flow();
    let mut corrupted = packet(identity, 0);
    corrupted[PAYLOAD_BYTES - 1] ^= 1;
    source.send(&corrupted).unwrap();
    assert!(receive_tx_payloads(&sink, identity, 1, timeout).is_err());

    let (sink, source) = loopback_flow();
    source.send(&packet(identity, 0)).unwrap();
    source.send(&packet(identity, 0)).unwrap();
    assert!(receive_tx_payloads(&sink, identity, 2, timeout).is_err());

    let (sink, _source) = loopback_flow();
    assert!(receive_tx_payloads(&sink, identity, 1, timeout).is_err());
}

#[test]
fn target_session_evidence_requires_both_application_directions_and_completion() {
    let offer = HostOffer {
        datagrams: RX_DATAGRAMS,
        bytes: RX_DATAGRAMS * PAYLOAD_BYTES as u64,
    };
    let receipt = HostReceipt {
        datagrams: 4,
        bytes: 4 * PAYLOAD_BYTES as u64,
    };
    assert!(require_recovery_exchange(offer, receipt, target_evidence(4, 4, true)).is_ok());
    assert!(require_recovery_exchange(offer, receipt, target_evidence(0, 4, true)).is_err());
    assert!(require_recovery_exchange(offer, receipt, target_evidence(4, 0, true)).is_err());
    assert!(require_recovery_exchange(offer, receipt, target_evidence(4, 4, false)).is_err());
    assert!(require_recovery_exchange(offer, receipt, target_evidence(4, 5, true)).is_err());
}
