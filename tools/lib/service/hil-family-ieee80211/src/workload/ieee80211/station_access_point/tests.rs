use super::*;

#[test]
fn fairness_is_symmetric_and_bounded() {
    assert!(validate_fairness("rx", 10_000, 12_000, 20).is_ok());
    assert!(validate_fairness("rx", 12_001, 10_000, 20).is_err());
    assert!(validate_fairness("rx", 10_000, 12_001, 20).is_err());
}

#[test]
fn access_point_epoch_rejects_one_beacon_period_lateness() {
    let evidence = oer_hil_protocol::wifi::WifiAccessPointEvidence {
        beacons_transmitted: 1,
        maximum_beacon_lateness_micros: 102_400,
        ..Default::default()
    };
    assert!(validate_access_point_epoch(evidence).is_err());
}

fn local_flow() -> (HostFlow, UdpSocket) {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    socket.connect(peer.local_addr().unwrap()).unwrap();
    peer.connect(socket.local_addr().unwrap()).unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let port = socket.local_addr().unwrap().port();
    (
        HostFlow {
            target: Ipv4Addr::LOCALHOST,
            peer: Ipv4Addr::LOCALHOST,
            port,
            socket,
        },
        peer,
    )
}

#[test]
fn both_reverse_probes_finish_before_collectors_start() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("collectors");
    let (station, station_peer) = local_flow();
    let (access_point, access_point_peer) = local_flow();
    let responders: Vec<_> = [&station_peer, &access_point_peer]
        .into_iter()
        .map(|peer| {
            let peer = peer.try_clone().unwrap();
            thread::spawn(move || {
                let mut bytes = [0; oer_hil_protocol::network::UdpProbe::LENGTH];
                let length = peer.recv(&mut bytes).unwrap();
                let mut probe = oer_hil_protocol::network::UdpProbe::decode(&bytes[..length])
                    .expect("real reverse-path challenge");
                assert!(!probe.response);
                probe.response = true;
                peer.send(&probe.encode()).unwrap();
            })
        })
        .collect();
    let mut ready_interfaces = Vec::new();
    let receivers = prepare_receivers(
        [&station, &access_point],
        crate::scenario::Direction::Tx,
        Duration::from_secs(5),
        &output,
        |interface, socket| {
            // Receiver::start creates its output directory synchronously. No
            // collector may own either socket during either readiness probe.
            assert!(!output.exists());
            oer_hil_net_traffic::udp::confirm_reverse_flow(socket, Duration::from_secs(2))?;
            ready_interfaces.push(interface);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(
        ready_interfaces,
        [
            WifiNetworkInterface::Station,
            WifiNetworkInterface::AccessPoint
        ]
    );
    for responder in responders {
        responder.join().unwrap();
    }
    for peer in [&station_peer, &access_point_peer] {
        peer.send(&0_u32.to_be_bytes()).unwrap();
    }
    for receiver in receivers {
        let bursts = receiver.unwrap().finish(Some(1)).unwrap();
        assert_eq!(bursts.len(), 1);
        assert_eq!(bursts[0].datagrams, 1);
        assert_eq!(bursts[0].bytes, 4, "probe replies are not measured data");
    }
}

#[test]
fn failed_second_preflight_starts_neither_collector() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("collectors");
    let (station, _station_peer) = local_flow();
    let (access_point, _access_point_peer) = local_flow();
    let error = prepare_receivers(
        [&station, &access_point],
        crate::scenario::Direction::Bidirectional,
        Duration::from_secs(5),
        &output,
        |interface, _| {
            assert!(!output.exists());
            if interface == WifiNetworkInterface::AccessPoint {
                Err("AP reverse path unavailable".into())
            } else {
                Ok(())
            }
        },
    )
    .err()
    .expect("failed preflight must prevent collection and session admission");
    assert_eq!(error.to_string(), "AP reverse path unavailable");
    assert!(!output.exists());
}
