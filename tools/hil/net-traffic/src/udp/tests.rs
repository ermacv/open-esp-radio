use super::*;
use std::net::Ipv4Addr;

#[test]
fn sending_a_probe_is_not_reverse_path_confirmation() {
    let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    socket.connect(peer.local_addr().unwrap()).unwrap();
    let error = confirm_reverse_flow(&socket, std::time::Duration::from_millis(30)).unwrap_err();
    assert!(error.to_string().contains("matching response"));
}

#[test]
fn reverse_confirmation_rejects_stale_nonce_and_retries_a_lost_reply() {
    use oer_hil_protocol::network::UdpProbe;
    use std::{thread, time::Duration};
    let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    socket.connect(peer.local_addr().unwrap()).unwrap();
    let responder = thread::spawn(move || {
        let mut bytes = [0_u8; UdpProbe::LENGTH];
        let (length, host) = peer.recv_from(&mut bytes).unwrap();
        let request = UdpProbe::decode(&bytes[..length]).unwrap();
        assert!(!request.response);
        // A reflected request and an old response must not admit measured Start.
        peer.send_to(&bytes, host).unwrap();
        peer.send_to(
            &UdpProbe {
                nonce: request.nonce.wrapping_sub(1),
                response: true,
            }
            .encode(),
            host,
        )
        .unwrap();
        let (length, again) = peer.recv_from(&mut bytes).unwrap();
        assert_eq!(again, host);
        assert_eq!(UdpProbe::decode(&bytes[..length]), Some(request));
        peer.send_to(
            &UdpProbe {
                response: true,
                ..request
            }
            .encode(),
            host,
        )
        .unwrap();
    });
    confirm_reverse_flow(&socket, Duration::from_secs(2)).unwrap();
    responder.join().unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn kernel_drop_counter_observes_overflow_without_a_following_received_packet() {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    rustix::net::sockopt::set_socket_recv_buffer_size(&socket, 4096).unwrap();
    let before = kernel_drops(&socket).unwrap().unwrap();
    let sender = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    for _ in 0..256 {
        sender
            .send_to(&[0_u8; 1472], socket.local_addr().unwrap())
            .unwrap();
    }
    let after = kernel_drops(&socket).unwrap().unwrap();
    assert!(after > before, "unread tiny queue must report its own loss");
}
