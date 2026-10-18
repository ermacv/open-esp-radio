use super::*;

#[test]
fn block_ack_status_is_functional_link_state_and_resets_at_link_edges() {
    let status = StationStatusChannel::new();
    status.publish_link(
        StationLinkState::Connected,
        Some((
            40,
            StationLinkSecurity::Wpa2Personal {
                management_protection: true,
            },
        )),
    );
    assert_eq!(status.snapshot().association_bandwidth_mhz, Some(40));
    assert_eq!(
        status.snapshot().link_security,
        Some(StationLinkSecurity::Wpa2Personal {
            management_protection: true,
        })
    );
    status.publish_tx_block_ack(0, true);
    status.publish_tx_block_ack(3, true);
    assert_eq!(status.snapshot().tx_block_ack_operational_tids, 0b0000_1001);

    let revision = status.snapshot().revision;
    status.publish_tx_block_ack(3, true);
    assert_eq!(status.snapshot().revision, revision);

    status.publish_link(StationLinkState::Disconnected(None), None);
    assert_eq!(status.snapshot().tx_block_ack_operational_tids, 0);
    assert_eq!(status.snapshot().association_bandwidth_mhz, None);
    assert_eq!(status.snapshot().link_security, None);
}
