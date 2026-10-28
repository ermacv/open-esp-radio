use super::PhyMode;
use crate::seq;
use oer_ieee80211_wire::station::association::AssociationPhy;
use oer_ieee80211_wire::{
    scan::ScanRecord,
    security::{AssociationAkm, Spp},
    station::*,
};
const LOCAL: [u8; 6] = [0x02, 0, 0, 0x12, 0x34, 0x56];
const BSSID: [u8; 6] = [0x30, 0x05, 0x5c, 0x11, 0x22, 0x33];
const SELECTED_RSN_IE_LEN: usize = 22;
// A synthetic profile keeps framing/admission tests independent of any chip.
// Neither test below emits a local HT/HE element.
const TEST_ASSOCIATION_CAPABILITIES: AssociationCapabilities = AssociationCapabilities {
    ht20: [0; 28],
    ht40: [0; 28],
    he20_ht: [0; 28],
    he20: [0; 24],
    he20_extended: [0; 14],
    wmm: [0; 9],
};

#[test]
fn non_he_modes_refuse_he_power_elements() {
    let mut peer = access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], 0);
    peer.set_ht_capability_ie(Some([0; 28]));
    let power = StaPowerCapability::new(0, 10).unwrap();
    for mode in [PhyMode::Legacy, PhyMode::Ht20] {
        assert!(matches!(
            mode.elements(&peer, &TEST_ASSOCIATION_CAPABILITIES, Some(power), None),
            Err(AssociationRequestError::UnexpectedPowerCapability)
        ));
    }
}

/// The selection a WPA2-Personal station makes for a PSK access point
/// without RSN capabilities.
fn wpa2_rsn() -> SelectedRsn {
    SelectedRsn::personal(AssociationAkm::Psk, Spp::NotNegotiated, false)
}

fn access_point_with_rsn(akms: &[[u8; 4]], capabilities: u16) -> ScanRecord {
    let mut record = ScanRecord::new(
        oer_ieee80211_wire::address::Bssid::new(
            oer_ieee80211_wire::address::IndividualAddress::new(BSSID).unwrap(),
        )
        .unwrap(),
        6,
        -40,
    );
    record.set_ssid(b"test").unwrap();
    record.bssid = oer_ieee80211_wire::address::Bssid::new(
        oer_ieee80211_wire::address::IndividualAddress::new(BSSID).unwrap(),
    )
    .unwrap();
    record.channel = 6;
    record.privacy = true;
    record.rsn = true;
    record.rsn_ie_count = 1;
    record
        .set_supported_rates(&[0x82, 0x84, 0x8b, 0x96])
        .unwrap();
    let mut rsn_ie = [0; oer_ieee80211_wire::scan::RSN_IE_CAPACITY];
    let mut offset = 2;
    rsn_ie[offset..offset + 2].copy_from_slice(&1_u16.to_le_bytes());
    offset += 2;
    rsn_ie[offset..offset + 4].copy_from_slice(&[0, 0x0f, 0xac, 4]);
    offset += 4;
    rsn_ie[offset..offset + 2].copy_from_slice(&1_u16.to_le_bytes());
    offset += 2;
    rsn_ie[offset..offset + 4].copy_from_slice(&[0, 0x0f, 0xac, 4]);
    offset += 4;
    rsn_ie[offset..offset + 2].copy_from_slice(&(akms.len() as u16).to_le_bytes());
    offset += 2;
    for akm in akms {
        rsn_ie[offset..offset + 4].copy_from_slice(akm);
        offset += 4;
    }
    rsn_ie[offset..offset + 2].copy_from_slice(&capabilities.to_le_bytes());
    offset += 2;
    rsn_ie[0] = 48;
    rsn_ie[1] = (offset - 2) as u8;
    record.set_rsn_ie(&rsn_ie[..offset]).unwrap();
    record
}
#[test]
fn association_request_contains_selected_rsn() {
    let record = access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], 0);
    let mut output = [0; 96];
    let length = AssociationRequest {
        source: oer_ieee80211_wire::address::OwnAddress::new(
            oer_ieee80211_wire::address::IndividualAddress::new(LOCAL).unwrap(),
        ),
        access_point: &record,
        sequence_number: seq(2),
        listen_interval: 1,
        phy: PhyMode::Legacy,
        security: &wpa2_rsn(),
        power_capability: None,
        he_ul_mu_power: None,
    }
    .encode(&mut output, &TEST_ASSOCIATION_CAPABILITIES)
    .unwrap();
    assert_eq!(&output[0..2], &[0, 0]);
    assert_eq!(&output[4..10], &BSSID);
    assert_eq!(&output[28..34], &[0, 4, b't', b'e', b's', b't']);
    assert_eq!(
        &output[length - SELECTED_RSN_IE_LEN..length],
        &[
            48, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 4,
        ]
    );
}

#[test]
fn ht20_request_fails_closed_when_the_ap_did_not_advertise_ht() {
    let record = access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], 0);
    assert_eq!(
        AssociationRequest {
            source: oer_ieee80211_wire::address::OwnAddress::new(
                oer_ieee80211_wire::address::IndividualAddress::new(LOCAL).unwrap()
            ),
            access_point: &record,
            sequence_number: seq(2),
            listen_interval: 1,
            phy: PhyMode::Ht20,
            security: &wpa2_rsn(),
            power_capability: None,
            he_ul_mu_power: None,
        }
        .encode(&mut [0; 160], &TEST_ASSOCIATION_CAPABILITIES),
        Err(AssociationRequestError::HtUnsupportedByAccessPoint)
    );
}

#[test]
fn association_encoder_uses_the_explicit_local_profile() {
    let mut record = access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], 0);
    record.set_ht_capability_ie(Some([0; oer_ieee80211_wire::ht::HT_CAPABILITY_IE_LEN]));
    let mut capabilities = TEST_ASSOCIATION_CAPABILITIES;
    capabilities.ht20[..6].copy_from_slice(&[45, 26, 0, 0, 0, 1]);
    capabilities.wmm = [221, 7, 0, 0x50, 0xf2, 2, 0, 1, 0];
    let mut output = [0; 160];
    let length = AssociationRequest {
        source: oer_ieee80211_wire::address::OwnAddress::new(
            oer_ieee80211_wire::address::IndividualAddress::new(LOCAL).unwrap(),
        ),
        access_point: &record,
        sequence_number: seq(2),
        listen_interval: 1,
        phy: PhyMode::Ht20,
        security: &wpa2_rsn(),
        power_capability: None,
        he_ul_mu_power: None,
    }
    .encode(&mut output, &capabilities)
    .unwrap();

    let elements = &output[length - 37..length];
    assert_eq!(&elements[..6], &[45, 26, 0, 0, 0, 1]);
    assert_eq!(&elements[28..], &[221, 7, 0, 0x50, 0xf2, 2, 0, 1, 0]);
}
