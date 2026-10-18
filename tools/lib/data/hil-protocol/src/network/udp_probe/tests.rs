use super::*;

#[test]
fn response_is_correlated_and_cannot_be_confused_with_request_or_data() {
    for nonce in [0, 1, u64::MAX] {
        for response in [false, true] {
            let probe = UdpProbe { nonce, response };
            assert_eq!(UdpProbe::decode(&probe.encode()), Some(probe));
            assert_eq!(UdpProbe::decode(&probe.encode()[..15]), None);
        }
    }
    assert_eq!(UdpProbe::decode(&[0; 16]), None);
    assert_eq!(UdpProbe::decode(&[0xff; 17]), None);
}
