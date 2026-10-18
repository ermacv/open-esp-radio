use super::*;

#[test]
fn network_link_requires_an_authorized_peer_in_every_ap_composition() {
    assert!(matches!(
        access_point_network_link_state(0),
        LinkState::Down
    ));
    assert!(matches!(access_point_network_link_state(1), LinkState::Up));
    assert!(matches!(
        access_point_network_link_state(AP_MAX_CLIENTS as u8),
        LinkState::Up
    ));
}
