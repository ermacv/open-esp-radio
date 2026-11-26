//! The HE settings' values stay within their fields.

use super::{BssColorScope, BssColorUse, HeAssociation, HeAssociationError, HeBssColor};
use crate::BssColor;
use oer_protocols_ieee80211_wire::he::{
    DefaultPeDuration, HeOperation, HeOperationFields, HePacketPadding, HePeerState,
};

fn aid(value: u16) -> oer_protocols_ieee80211_wire::management::AidField {
    oer_protocols_ieee80211_wire::management::AidField::from_bytes(value.to_le_bytes())
}

/// An HE Operation of default PE 4, BSS color 42 (partial) and the basic
/// set `0xfffc`.
fn operation_fields() -> HeOperationFields {
    HeOperationFields::from_bytes([0; 6])
        .with_default_pe(DefaultPeDuration::Us16)
        .with_bss_color(42)
        .map(|fields| fields.with_basic_mcs_nss(0xfffc))
        .unwrap()
        .with_partial_bss_color(true)
}

fn peer() -> HePeerState {
    HePeerState {
        capability_prefix: [0; 24],
        packet_padding: HePacketPadding::Us16,
        operation: operation_fields().with_er_su_disabled(true),
        max_co_hosted_bssid_indicator: Some(3),
    }
}

#[test]
fn an_association_keeps_the_peer_values_it_was_made_of() {
    let association = HeAssociation::new(&peer(), aid(1), 7).expect("in range");
    assert_eq!(association.association_id().value(), 1);
    assert_eq!(association.minimum_mpdu_start_spacing(), 7);
    assert_eq!(association.packet_padding(), HePacketPadding::Us16);
    assert_eq!(
        association.default_packet_extension(),
        DefaultPeDuration::Us16
    );
    assert_eq!(association.max_co_hosted_bssid_indicator(), Some(3));
    assert_eq!(
        association.color().get(),
        (
            BssColor::new(42).unwrap(),
            BssColorUse::Enabled,
            BssColorScope::Partial
        )
    );
    assert!(association.extended_range_single_user_disabled());
}

#[test]
fn a_bss_color_is_the_scan_s_and_a_disabled_one_is_transmitted_as_zero() {
    let mut operation = HeOperation {
        fields: operation_fields().with_partial_bss_color(false),
    };
    let color = HeBssColor::of(&operation).expect("a color");
    assert_eq!(
        color.get(),
        (
            BssColor::new(42).unwrap(),
            BssColorUse::Enabled,
            BssColorScope::Full
        )
    );
    assert_eq!(color.transmit_color().get(), 42);
    operation.fields = operation.fields.with_bss_color_disabled(true);
    assert_eq!(
        HeBssColor::of(&operation).unwrap().transmit_color(),
        BssColor::ZERO
    );
}

#[test]
fn values_outside_their_fields_make_no_association() {
    assert_eq!(
        HeAssociation::new(&peer(), aid(0), 0),
        Err(HeAssociationError::AssociationId(aid(0)))
    );
    assert_eq!(
        HeAssociation::new(&peer(), aid(2008), 0),
        Err(HeAssociationError::AssociationId(aid(2008)))
    );
    assert_eq!(
        HeAssociation::new(&peer(), aid(1), 8),
        Err(HeAssociationError::MinimumMpduStartSpacing(8))
    );
    let mut hosted = peer();
    hosted.max_co_hosted_bssid_indicator = Some(9);
    assert_eq!(
        HeAssociation::new(&hosted, aid(1), 0),
        Err(HeAssociationError::MaxCoHostedBssidIndicator(9))
    );
}
