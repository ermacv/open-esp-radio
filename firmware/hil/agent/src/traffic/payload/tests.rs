use super::*;

fn datagram(fill: u8) -> [u8; 64] {
    let mut payload = [fill; 64];
    payload[..12].fill(0);
    payload
}

#[test]
fn an_unidentified_session_never_scans_the_payload() {
    let check = PayloadFillCheck::for_flows([None, None]);
    assert!(!check.fill_matches(&datagram(0x5a)));
    assert!(!check.fill_matches(&datagram(0x00)));
}

#[test]
fn an_identified_session_checks_every_datagram() {
    let check = PayloadFillCheck::for_flows([None, Some(UdpSessionPayloadIdentity::new(7))]);
    assert!(check.fill_matches(&datagram(0x5a)));
    let mut altered = datagram(0x5a);
    altered[63] = 0;
    assert!(!check.fill_matches(&altered));
}
