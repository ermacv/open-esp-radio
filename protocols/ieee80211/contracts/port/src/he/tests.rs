//! The HE20 association's values stay within their fields.

use super::{He20Association, He20AssociationError};
use crate::BssColor;
use oer_protocols_ieee80211_wire::he::He20PeerState;

fn peer() -> He20PeerState {
    He20PeerState {
        capability_prefix: [0; 24],
        packet_padding_eight_us: 2,
        default_packet_extension_duration: 4,
        bss_color: 42,
        bss_color_enabled: true,
        partial_bss_color: true,
        basic_mcs_nss_map: 0xfffc,
        rts_threshold: None,
        extended_range_single_user_disabled: true,
    }
}

#[test]
fn an_association_keeps_the_peer_values_it_was_made_of() {
    let association = He20Association::new(&peer(), 1, 7).expect("in range");
    assert_eq!(association.association_id(), 1);
    assert_eq!(association.minimum_mpdu_start_spacing(), 7);
    assert_eq!(association.packet_padding_eight_us(), 2);
    assert_eq!(association.default_packet_extension(), 4);
    assert_eq!(
        association.color(),
        (BssColor::new(42).expect("color"), true, true)
    );
    assert_eq!(association.transmit_color().get(), 42);
    assert!(association.extended_range_single_user_disabled());
}

#[test]
fn a_disabled_color_is_transmitted_as_zero() {
    let mut peer = peer();
    peer.bss_color_enabled = false;
    let association = He20Association::new(&peer, 2007, 0).expect("in range");
    assert_eq!(association.transmit_color(), BssColor::ZERO);
    assert_eq!(association.color().0.get(), 42);
}

#[test]
fn values_outside_their_fields_make_no_association() {
    assert_eq!(
        He20Association::new(&peer(), 0, 0),
        Err(He20AssociationError::AssociationId(0))
    );
    assert_eq!(
        He20Association::new(&peer(), 2008, 0),
        Err(He20AssociationError::AssociationId(2008))
    );
    assert_eq!(
        He20Association::new(&peer(), 1, 8),
        Err(He20AssociationError::MinimumMpduStartSpacing(8))
    );
    let mut padded = peer();
    padded.packet_padding_eight_us = 3;
    assert_eq!(
        He20Association::new(&padded, 1, 0),
        Err(He20AssociationError::PacketPadding(3))
    );
    let mut extended = peer();
    extended.default_packet_extension_duration = 8;
    assert_eq!(
        He20Association::new(&extended, 1, 0),
        Err(He20AssociationError::DefaultPacketExtension(8))
    );
    let mut colored = peer();
    colored.bss_color = 64;
    assert_eq!(
        He20Association::new(&colored, 1, 0),
        Err(He20AssociationError::BssColor(64))
    );
}
