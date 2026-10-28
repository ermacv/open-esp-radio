use super::*;
use oer_ieee80211_wire::security::{
    Pmkid,
    rsn::{
        RSN_CAPABILITY_MFPC, RSN_CAPABILITY_MFPR, RSN_GROUP_MANAGEMENT_ELEMENT_LEN,
        RsnExtendedCapabilities, rsn_group_management_element, rsnx_element,
    },
};

const BSSID: [u8; 6] = [0x30, 0x05, 0x5c, 0x11, 0x22, 0x33];
const SELECTED_RSN_IE_LEN: usize = 22;
/// The RSN element, a PMKID list naming one PMKID and a one-octet RSNXE.
const SELECTED_SECURITY_IES_CAPACITY: usize = SELECTED_RSN_IE_LEN + 18 + 3;

#[test]
fn spp_negotiation_follows_both_rsn_capabilities() {
    use oer_ieee80211_wire::security::{AssociationSecurity, Spp, SppCapability, SppError};
    for (advertised, expected) in [
        (SppCapability::NotCapable, Spp::NotNegotiated),
        (SppCapability::Capable, Spp::Negotiated),
        (SppCapability::Required, Spp::Negotiated),
    ] {
        let record = access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], advertised.rsn_bits());
        let selected = select_association_rsn(&record, StaSecurityPolicy::Wpa2Personal).unwrap();
        let AssociationSecurity::Rsn(association) = selected.security() else {
            panic!("personal association");
        };
        assert_eq!(association.spp, expected);
        assert_eq!(
            u16::from_le_bytes(selected.as_bytes()[20..22].try_into().unwrap()),
            SppCapability::Capable.rsn_bits()
        );
    }
    let malformed = access_point_with_rsn(
        &[[0, 0x0f, 0xac, 2]],
        oer_ieee80211_wire::security::rsn::RSN_CAPABILITY_SPP_AMSDU_REQUIRED,
    );
    assert_eq!(
        select_association_rsn(&malformed, StaSecurityPolicy::Wpa2Personal),
        Err(StaSecurityError::Spp(SppError::RequiredWithoutCapability))
    );
    assert_eq!(
        SppCapability::NotCapable.negotiate(SppCapability::Required),
        Err(SppError::PeerRequiresSupport)
    );
    assert_eq!(
        SppCapability::Required.negotiate(SppCapability::NotCapable),
        Err(SppError::LocalRequiresSupport)
    );
}

#[test]
fn a_wpa2_request_upgrades_to_sae_on_a_capable_transition_access_point() {
    let record = access_point_with_rsn(&[[0, 0x0f, 0xac, 8], [0, 0x0f, 0xac, 2]], 0x80);
    let selected = select_association_rsn(&record, StaSecurityPolicy::Wpa2Personal).unwrap();
    assert_eq!(
        selected.negotiated_akm(),
        AssociationAkm::Sae(SaePwe::HuntingAndPecking)
    );
    assert_eq!(selected.as_bytes().len(), SELECTED_RSN_IE_LEN + 3);
    assert_eq!(&selected.as_bytes()[8..14], &[1, 0, 0, 0x0f, 0xac, 4]);
    assert_eq!(&selected.as_bytes()[14..20], &[1, 0, 0, 0x0f, 0xac, 8]);
    // SPP A-MSDU capable, MFPC and, under SAE, MFPR.
    assert_eq!(&selected.as_bytes()[20..22], &[0xc0, 4]);
    // The RSNXE announces hash to element.
    assert_eq!(&selected.as_bytes()[22..], &[244, 1, 0x20]);
    assert!(selected.security().protects_management());
    assert_eq!(
        select_association_rsn(&record, StaSecurityPolicy::Wpa3Personal)
            .unwrap()
            .negotiated_akm(),
        AssociationAkm::Sae(SaePwe::HuntingAndPecking)
    );
}

#[test]
fn a_resumed_sae_association_lists_its_pmkid_before_the_rsnxe() {
    let record = access_point_with_rsn(&[[0, 0x0f, 0xac, 8]], 0xc0);
    let selected = select_association_rsn(&record, StaSecurityPolicy::Wpa3Personal).unwrap();
    let resumed = selected.with_pmkid(Pmkid([0x5a; 16])).unwrap();
    let bytes = resumed.as_bytes();
    assert_eq!(bytes.len(), SELECTED_SECURITY_IES_CAPACITY);
    assert_eq!(&bytes[..2], &[48, 38]);
    assert_eq!(&bytes[2..22], &selected.as_bytes()[2..22]);
    assert_eq!(&bytes[22..24], &[1, 0]);
    assert_eq!(&bytes[24..40], &[0x5a; 16]);
    assert_eq!(&bytes[40..], &[244, 1, 0x20]);
    assert_eq!(resumed.negotiated_akm(), selected.negotiated_akm());
    assert!(
        oer_ieee80211_wire::security::rsn::RsnElement::parse(&bytes[..40])
            .unwrap()
            .akm_suites()
            .contains(oer_ieee80211_wire::security::rsn::ieee_suite(8))
    );
}

#[test]
fn the_access_point_rsnxe_selects_hash_to_element() {
    let mut record = access_point_with_rsn(&[[0, 0x0f, 0xac, 8]], 0xc0);
    record.set_rsnxe(&[244, 1, 0x20]).unwrap();
    assert_eq!(
        select_association_rsn(&record, StaSecurityPolicy::Wpa3Personal)
            .unwrap()
            .negotiated_akm(),
        AssociationAkm::Sae(SaePwe::HashToElement)
    );
}

#[test]
fn sae_needs_management_frame_protection_and_wpa3_needs_sae() {
    // SAE without MFPC falls back to PSK for a WPA2 request only.
    let incapable = access_point_with_rsn(&[[0, 0x0f, 0xac, 8], [0, 0x0f, 0xac, 2]], 0);
    let selected = select_association_rsn(&incapable, StaSecurityPolicy::Wpa2Personal).unwrap();
    assert_eq!(selected.negotiated_akm(), AssociationAkm::Psk);
    assert_eq!(selected.as_bytes().len(), SELECTED_RSN_IE_LEN);
    assert_eq!(
        select_association_rsn(&incapable, StaSecurityPolicy::Wpa3Personal),
        Err(StaSecurityError::UnsupportedAkm)
    );
    let psk_only = access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], 0x80);
    assert_eq!(
        select_association_rsn(&psk_only, StaSecurityPolicy::Wpa3Personal),
        Err(StaSecurityError::UnsupportedAkm)
    );
}

#[test]
fn an_access_point_without_mfpc_keeps_management_frames_unprotected() {
    let selected = select_association_rsn(
        &access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], 0),
        StaSecurityPolicy::Wpa2Personal,
    )
    .unwrap();
    assert_eq!(&selected.as_bytes()[20..22], &[0, 4]);
    assert!(!selected.security().protects_management());
}

#[test]
fn psk_sha256_is_preferred_whenever_offered() {
    let record = access_point_with_rsn(&[[0, 0x0f, 0xac, 2], [0, 0x0f, 0xac, 6]], 0);
    assert_eq!(
        &select_association_rsn(&record, StaSecurityPolicy::Wpa2Personal)
            .unwrap()
            .as_bytes()[14..20],
        &[1, 0, 0, 0x0f, 0xac, 6]
    );
}

#[test]
fn required_management_frame_protection_needs_capability() {
    let required = access_point_with_rsn(
        &[[0, 0x0f, 0xac, 6]],
        RSN_CAPABILITY_MFPR | RSN_CAPABILITY_MFPC,
    );
    let selected = select_association_rsn(&required, StaSecurityPolicy::Wpa2Personal).unwrap();
    assert!(selected.security().protects_management());
    // The station stays capable only; it does not require protection.
    assert_eq!(&selected.as_bytes()[20..22], &[0x80, 4]);
    assert_eq!(
        select_association_rsn(
            &access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], RSN_CAPABILITY_MFPR),
            StaSecurityPolicy::Wpa2Personal
        ),
        Err(StaSecurityError::MalformedRsn)
    );
}

#[test]
fn only_bip_cmac_128_protects_group_management_frames() {
    let mut gmac = access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], RSN_CAPABILITY_MFPC);
    append_rsn_tail(&mut gmac, &[0, 0, 0x00, 0x0f, 0xac, 11]);
    assert_eq!(
        select_association_rsn(&gmac, StaSecurityPolicy::Wpa2Personal),
        Err(StaSecurityError::UnsupportedGroupManagementCipher)
    );
}

#[test]
fn complete_optional_rsn_tails_are_consumed_exactly() {
    let mut with_pmkid = access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], 0);
    let mut pmkid_tail = [0x5a; 18];
    pmkid_tail[..2].copy_from_slice(&1_u16.to_le_bytes());
    append_rsn_tail(&mut with_pmkid, &pmkid_tail);
    assert!(select_association_rsn(&with_pmkid, StaSecurityPolicy::Wpa2Personal).is_ok());

    let mut with_group_management = access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], 1 << 7);
    append_rsn_tail(&mut with_group_management, &[0, 0, 0x00, 0x0f, 0xac, 6]);
    assert!(
        select_association_rsn(&with_group_management, StaSecurityPolicy::Wpa2Personal).is_ok()
    );
}

#[test]
fn truncated_or_trailing_rsn_optional_fields_are_rejected() {
    let mut truncated_pmkid = [0; 17];
    truncated_pmkid[..2].copy_from_slice(&1_u16.to_le_bytes());
    let truncated_group_management = [0, 0, 0x00, 0x0f, 0xac];
    let group_management_without_mfpc = [0, 0, 0x00, 0x0f, 0xac, 6];
    let trailing_after_group_management = [0, 0, 0x00, 0x0f, 0xac, 6, 0xa5];

    for tail in [
        &[0xa5][..],
        &truncated_pmkid,
        &truncated_group_management,
        &group_management_without_mfpc,
        &trailing_after_group_management,
    ] {
        let mut record = access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], 0);
        append_rsn_tail(&mut record, tail);
        assert_eq!(
            select_association_rsn(&record, StaSecurityPolicy::Wpa2Personal),
            Err(StaSecurityError::MalformedRsn)
        );
    }
}

#[test]
fn open_ap_needs_no_security_ie() {
    let mut record = ScanRecord::new(
        oer_ieee80211_wire::address::Bssid::new(
            oer_ieee80211_wire::address::IndividualAddress::new(BSSID).unwrap(),
        )
        .unwrap(),
        6,
        -40,
    );
    record.set_supported_rates(&[0x82]).unwrap();
    assert!(
        select_association_rsn(&record, StaSecurityPolicy::Open)
            .unwrap()
            .as_bytes()
            .is_empty()
    );
    assert_eq!(
        select_association_rsn(&record, StaSecurityPolicy::Wpa2Personal),
        Err(StaSecurityError::SecurityModeMismatch)
    );
}

#[test]
fn a_second_pmkid_is_rejected_without_growing_the_selected_elements() {
    let record = access_point_with_rsn(&[[0, 0x0f, 0xac, 8]], 0xc0);
    let selected = select_association_rsn(&record, StaSecurityPolicy::Wpa3Personal).unwrap();
    let resumed = selected.with_pmkid(Pmkid([0x5a; 16])).unwrap();
    assert_eq!(resumed.with_pmkid(Pmkid([0xa5; 16])), None);
    assert_eq!(resumed.as_bytes().len(), SELECTED_SECURITY_IES_CAPACITY);
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

fn append_rsn_tail(record: &mut ScanRecord, tail: &[u8]) {
    let mut rsn_ie = record.rsn_ie_bytes().to_vec();
    rsn_ie.extend_from_slice(tail);
    rsn_ie[1] = (rsn_ie.len() - 2) as u8;
    record.set_rsn_ie(&rsn_ie).unwrap();
}

/// The authentication a personal selection negotiated.
trait NegotiatedAkm {
    fn negotiated_akm(&self) -> oer_ieee80211_wire::security::AssociationAkm;
}

impl NegotiatedAkm for SelectedRsn {
    fn negotiated_akm(&self) -> oer_ieee80211_wire::security::AssociationAkm {
        match self.security() {
            oer_ieee80211_wire::security::AssociationSecurity::Rsn(association) => association.akm,
            oer_ieee80211_wire::security::AssociationSecurity::Open => {
                panic!("an Open selection has no AKM")
            }
        }
    }
}

#[test]
fn a_wpa3_access_point_profile_selects_sae_with_hash_to_element() {
    // The elements a WPA3-Personal access point advertises: SAE with MFPC,
    // MFPR and BIP-CMAC-128, and an RSNXE announcing hash to element.
    let rsne: [u8; RSN_GROUP_MANAGEMENT_ELEMENT_LEN] = rsn_group_management_element(
        RSN_CIPHER_CCMP,
        RSN_CIPHER_CCMP,
        RSN_AKM_SAE,
        RSN_CAPABILITY_MFPC | RSN_CAPABILITY_MFPR,
        RSN_CIPHER_BIP_CMAC_128,
    );
    let mut record = access_point_with_rsn(&[[0, 0x0f, 0xac, 8]], 0);
    record.set_rsn_ie(&rsne).unwrap();
    record
        .set_rsnxe(&rsnx_element(
            RsnExtendedCapabilities::from_bytes([0]).with_sae_h2e(true),
        ))
        .unwrap();
    for policy in [
        StaSecurityPolicy::Wpa3Personal,
        StaSecurityPolicy::Wpa2Personal,
    ] {
        let selected = select_association_rsn(&record, policy).unwrap();
        assert_eq!(
            selected.negotiated_akm(),
            AssociationAkm::Sae(SaePwe::HashToElement)
        );
        assert!(selected.security().protects_management());
    }
}

#[test]
fn security_matching_admits_one_exact_mode() {
    let record = access_point_with_rsn(&[[0, 0x0f, 0xac, 2]], 0);
    assert!(matches_security(&record, LinkProtection::Ccmp));
    assert!(!matches_security(&record, LinkProtection::Open));
    let mut mixed = record;
    mixed.legacy_wpa = true;
    assert!(!matches_security(&mixed, LinkProtection::Ccmp));
}
