use oer_esp32s31_hal::types::MacKeyInstallOutcome;

use oer_esp32s31_ieee80211_mac::crypto::CcmpKeyHardware;

use oer_ieee80211_rsn::{Pmk, PtkContext};

use super::*;

#[derive(Default)]
struct Hardware {
    installs: std::vec::Vec<u8>,
    clears: std::vec::Vec<u8>,
}

impl CcmpKeyHardware for Hardware {
    fn install_sta_ccmp_entry(
        &mut self,
        index: u8,
        _identity: oer_esp32s31_hal::types::MacCcmpKeyIdentity,
        _temporal_key: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        self.installs.push(index);
        MacKeyInstallOutcome::Installed
    }

    fn clear_ccmp_entry(&mut self, index: u8) {
        self.clears.push(index);
    }
}

#[test]
fn teardown_clears_pairwise_before_group_and_cannot_alias_a_second_peer() {
    let mut hardware = Hardware::default();
    let gtk = RsnGtk::new(1, true, [0x55; 16]).unwrap();
    let mut pairwise = ApPairwiseKeyStorage::new();
    let mut security = ApSecurity::install_group(&mut hardware, &gtk, &mut pairwise).unwrap();
    let ptk = Pmk::derive(b"password", b"ssid").unwrap().derive_ptk(
        oer_ieee80211_mac::security::rsn::Akm::Psk,
        PtkContext {
            authenticator_address: [2; 6],
            supplicant_address: [3; 6],
            authenticator_nonce: [4; 32],
            supplicant_nonce: [5; 32],
        },
    );
    security
        .install_pairwise(&mut hardware, [3; 6], 1, &ptk)
        .unwrap();
    let binding = security.bind_pairwise([3; 6], 1).unwrap();
    assert_eq!(binding.hardware_index(), 8);
    assert_eq!(
        security
            .next_bound_pairwise_tx_ccmp_header(binding)
            .unwrap(),
        [3, 0, 0, 0x20, 0, 0, 0, 0]
    );
    assert_eq!(
        security.bind_pairwise([3; 6], 2),
        Err(ApSecurityError::WrongPeer)
    );
    assert_eq!(security.pairwise_hardware_index([3; 6]), Ok(8));
    assert_eq!(
        security.install_pairwise(&mut hardware, [3; 6], 2, &ptk),
        Err(ApSecurityError::PairwiseAlreadyInstalled),
    );
    security
        .install_pairwise(&mut hardware, [6; 6], 2, &ptk)
        .unwrap();
    let (report, _pairwise) = security.stop(&mut hardware);
    assert_eq!(hardware.installs, [2, 8, 9]);
    assert_eq!(hardware.clears, [8, 9, 2]);
    assert_eq!(
        report,
        ApSecurityStopReport::Wpa2Personal {
            pairwise_slots_cleared: 2,
            group_hardware_index: 2,
        }
    );
}

#[test]
fn all_public_aids_own_disjoint_pairwise_slots() {
    let mut hardware = Hardware::default();
    let gtk = RsnGtk::new(1, true, [0x55; 16]).unwrap();
    let mut pairwise = ApPairwiseKeyStorage::new();
    let mut security = ApSecurity::install_group(&mut hardware, &gtk, &mut pairwise).unwrap();
    let ptk = Pmk::derive(b"password", b"ssid").unwrap().derive_ptk(
        oer_ieee80211_mac::security::rsn::Akm::Psk,
        PtkContext {
            authenticator_address: [2; 6],
            supplicant_address: [3; 6],
            authenticator_nonce: [4; 32],
            supplicant_nonce: [5; 32],
        },
    );

    for association_id in 1..=15_u16 {
        let peer = [2, 0, 0, 0, 1, association_id as u8];
        security
            .install_pairwise(&mut hardware, peer, association_id, &ptk)
            .unwrap();
        assert_eq!(
            security.pairwise_hardware_index(peer),
            Ok(7 + association_id as u8),
        );
    }
    assert_eq!(
        hardware.installs,
        (2..=22)
            .filter(|index| *index == 2 || *index >= 8)
            .collect::<std::vec::Vec<_>>()
    );

    let overflow = security.install_pairwise(&mut hardware, [2, 0, 0, 0, 2, 1], 16, &ptk);
    assert_eq!(
        overflow,
        Err(ApSecurityError::Crypto(
            CryptoKeyError::InvalidAccessPointAssociationId,
        )),
    );
    let (report, _pairwise) = security.stop(&mut hardware);
    assert_eq!(
        report,
        ApSecurityStopReport::Wpa2Personal {
            pairwise_slots_cleared: 15,
            group_hardware_index: 2,
        }
    );
    assert_eq!(
        hardware.clears,
        (8..=22)
            .chain(core::iter::once(2))
            .collect::<std::vec::Vec<_>>()
    );
}

#[test]
fn rx_replay_is_per_tid_and_fenced_across_pairwise_key_reinstall() {
    let peer = [3; 6];
    let mut hardware = Hardware::default();
    let gtk = RsnGtk::new(1, true, [0x55; 16]).unwrap();
    let mut pairwise = ApPairwiseKeyStorage::new();
    let mut security = ApSecurity::install_group(&mut hardware, &gtk, &mut pairwise).unwrap();
    let ptk = Pmk::derive(b"password", b"ssid").unwrap().derive_ptk(
        oer_ieee80211_mac::security::rsn::Akm::Psk,
        PtkContext {
            authenticator_address: [2; 6],
            supplicant_address: peer,
            authenticator_nonce: [4; 32],
            supplicant_nonce: [5; 32],
        },
    );
    security
        .install_pairwise(&mut hardware, peer, 1, &ptk)
        .unwrap();
    let first_generation = security.bind_pairwise(peer, 1).unwrap();
    let pn1 = CcmpPacketNumber::new(1).unwrap();
    let pn4 = CcmpPacketNumber::new(4).unwrap();
    let pn5 = CcmpPacketNumber::new(5).unwrap();

    let first = security
        .prepare_bound_pairwise_rx(first_generation, CcmpReplayLane::NonQos, pn4)
        .unwrap();
    security.commit_bound_pairwise_rx(first).unwrap();
    assert_eq!(
        security.prepare_bound_pairwise_rx(first_generation, CcmpReplayLane::NonQos, pn4,),
        Err(ApSecurityError::Replay(CcmpReplayError::Replayed {
            packet_number: pn4,
            highest: pn4,
        })),
        "a different 802.11 sequence cannot make a reused PN admissible"
    );

    for lane in [CcmpReplayLane::Tid(1), CcmpReplayLane::Tid(7)] {
        security
            .commit_bound_pairwise_rx_immediate(first_generation, lane, pn1)
            .unwrap();
        assert_eq!(
            security.commit_bound_pairwise_rx_immediate(first_generation, lane, pn1),
            Err(ApSecurityError::Replay(CcmpReplayError::Replayed {
                packet_number: pn1,
                highest: pn1,
            })),
            "ordinary replay admission advances exactly the selected lane"
        );
    }

    let prepared_before_clear = security
        .prepare_bound_pairwise_rx(first_generation, CcmpReplayLane::NonQos, pn5)
        .unwrap();
    security.clear_peer(&mut hardware, peer).unwrap();
    security
        .install_pairwise(&mut hardware, peer, 1, &ptk)
        .unwrap();
    let replacement_generation = security.bind_pairwise(peer, 1).unwrap();
    assert_ne!(replacement_generation, first_generation);
    assert_eq!(
        security.commit_bound_pairwise_rx(prepared_before_clear),
        Err(ApSecurityError::WrongPeer),
        "a candidate from the cleared key cannot advance its replacement"
    );
    assert_eq!(
        security.prepare_bound_pairwise_rx(first_generation, CcmpReplayLane::NonQos, pn5,),
        Err(ApSecurityError::WrongPeer),
    );
    assert_eq!(
        security.commit_bound_pairwise_rx_immediate(first_generation, CcmpReplayLane::Tid(3), pn5,),
        Err(ApSecurityError::WrongPeer),
        "the immediate path cannot mutate a replacement key through a stale binding"
    );

    let replacement = security
        .prepare_bound_pairwise_rx(replacement_generation, CcmpReplayLane::NonQos, pn1)
        .unwrap();
    security.commit_bound_pairwise_rx(replacement).unwrap();
}

/// Station selection over this access point's advertisement and access
/// point admission of the resulting request negotiate the same security,
/// for every pair of policies.
#[test]
fn station_selection_and_access_point_admission_meet_at_one_association() {
    use oer_ieee80211_ap::service::admit_association_security;
    use oer_ieee80211_mac::{
        ap::ApAssociationSecurityObservation,
        beacon::{AP_BEACON_CAPACITY, write_ht_beacon},
        channel::WifiChannel,
        protection::ApBssProtection,
        scan::parse_management,
        security::{
            ApSecurityPolicy, AssociationAkm, AssociationSecurity, GroupManagementCipher,
            LinkProtection, RsnAssociation, SaePwe, StaSecurityPolicy,
        },
        sequence::SequenceNumber,
        ssid::WifiSsid,
        station::select_association_rsn,
    };

    const SAE: AssociationSecurity = AssociationSecurity::Rsn(RsnAssociation {
        akm: AssociationAkm::Sae(SaePwe::HashToElement),
        management: Some(GroupManagementCipher::BipCmac128),
        pmkid: None,
    });
    const PSK: AssociationSecurity = AssociationSecurity::Rsn(RsnAssociation {
        akm: AssociationAkm::Psk,
        management: None,
        pmkid: None,
    });
    let table = [
        (
            StaSecurityPolicy::Open,
            ApSecurityPolicy::Open,
            Some(AssociationSecurity::Open),
        ),
        (
            StaSecurityPolicy::Open,
            ApSecurityPolicy::Wpa2Personal,
            None,
        ),
        (
            StaSecurityPolicy::Open,
            ApSecurityPolicy::Wpa3Personal,
            None,
        ),
        (
            StaSecurityPolicy::Wpa2Personal,
            ApSecurityPolicy::Open,
            None,
        ),
        (
            StaSecurityPolicy::Wpa2Personal,
            ApSecurityPolicy::Wpa2Personal,
            Some(PSK),
        ),
        (
            StaSecurityPolicy::Wpa2Personal,
            ApSecurityPolicy::Wpa3Personal,
            Some(SAE),
        ),
        (
            StaSecurityPolicy::Wpa3Personal,
            ApSecurityPolicy::Open,
            None,
        ),
        (
            StaSecurityPolicy::Wpa3Personal,
            ApSecurityPolicy::Wpa2Personal,
            None,
        ),
        (
            StaSecurityPolicy::Wpa3Personal,
            ApSecurityPolicy::Wpa3Personal,
            Some(SAE),
        ),
    ];
    let access_point = [0x02, 0, 0, 0, 0, 1];
    let ssid = WifiSsid::new(b"open-radio-ap").unwrap();
    for (station, offer, expected) in table {
        let mut beacon = [0; AP_BEACON_CAPACITY];
        let length = write_ht_beacon(
            &crate::profile::ADVERTISEMENT,
            &mut beacon,
            access_point,
            &ssid,
            oer_ieee80211_mac::channel::Channel::from_wifi_channel(WifiChannel::mhz20(6).unwrap()),
            100,
            1,
            SequenceNumber::new(1).unwrap(),
            offer,
            ApBssProtection::default(),
        )
        .unwrap();
        let record = parse_management(&beacon[..length], 6, -40).unwrap();
        let Ok(selected) = select_association_rsn(&record, station) else {
            assert_eq!(expected, None, "{station:?} rejected {offer:?}");
            continue;
        };
        let elements = selected.as_bytes();
        let rsne_length = elements.get(1).map_or(0, |length| usize::from(*length) + 2);
        let (rsne, rsnxe) = elements.split_at(rsne_length);
        let request = ApAssociationSecurityObservation {
            privacy: selected.security().link_protection() == LinkProtection::Ccmp,
            rsn_ie: (!rsne.is_empty()).then_some(rsne),
            rsn_ie_count: u8::from(!rsne.is_empty()),
            rsnxe: (!rsnxe.is_empty()).then_some(rsnxe),
            rsnxe_count: u8::from(!rsnxe.is_empty()),
            legacy_wpa_present: false,
            malformed_elements: false,
        };
        let admitted = admit_association_security(offer, request);
        assert_eq!(admitted, expected, "{station:?} against {offer:?}");
        assert_eq!(
            admitted,
            Some(selected.security()),
            "{station:?} against {offer:?}"
        );
    }
}
