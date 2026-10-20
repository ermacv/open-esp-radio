use crate::ap::profile::tests::TEST_ADVERTISEMENT;
use crate::sequence::seq;

use super::{
    AP_BEACON_CAPACITY, ApBeaconBuildError, ApBeaconProtectionError, dtim, stamp,
    update_bss_protection, write_ht_beacon, write_tim_partial_virtual_bitmap,
};
use crate::{
    channel::{WifiChannel, WifiChannelWidth},
    ht::HtDuplicateMcs32,
    protection::{ApBssProtection, ErpProtection, HtOperationProtection, HtProtectionMode},
    scan::parse_management,
    security::{
        AP_SAE_H2E_RSNX_ELEMENT, AP_WPA2_PERSONAL_RSN_ELEMENT, AP_WPA3_PERSONAL_RSN_ELEMENT,
        ApSecurityPolicy, AssociationAkm, SaePwe, StaSecurityPolicy,
    },
    ssid::WifiSsid,
    station::select_association_rsn,
};

fn beacon() -> [u8; 44] {
    let mut bytes = [0_u8; 44];
    bytes[..2].copy_from_slice(&0x0080_u16.to_le_bytes());
    bytes[32..34].copy_from_slice(&100_u16.to_le_bytes());
    // SSID with zero-length payload, followed by a complete TIM.
    bytes[36] = 0;
    bytes[37] = 0;
    bytes[38] = 5;
    bytes[39] = 4;
    bytes[40] = 1;
    bytes[41] = 2;
    bytes
}

#[test]
fn executor_tsf_drives_dtim_and_group_indication() {
    let mut bytes = beacon();

    assert_eq!(stamp(&mut bytes, 0, true), Some((1, 2)));
    assert_eq!(dtim(&bytes), Some((38, 1, 2)));
    assert_eq!(bytes[42] & 1, 0);

    assert_eq!(stamp(&mut bytes, 100 * 1_024, true), Some((0, 2)));
    assert_eq!(dtim(&bytes), Some((38, 0, 2)));
    assert_eq!(bytes[42] & 1, 1);

    assert_eq!(stamp(&mut bytes, 3 * 100 * 1_024, false), Some((0, 2)));
    assert_eq!(bytes[42] & 1, 0);
}

#[test]
fn a_wpa3_beacon_offers_sae_with_protected_management_frames() {
    let ap = [0x02, 0, 0, 0, 0, 1];
    let ssid = WifiSsid::new(b"open-radio-ap").unwrap();
    let mut bytes = [0; AP_BEACON_CAPACITY];
    let len = write_ht_beacon(
        &TEST_ADVERTISEMENT,
        &mut bytes,
        ap,
        &ssid,
        crate::channel::Channel::from_wifi_channel(WifiChannel::mhz20(6).unwrap()),
        100,
        2,
        seq(1),
        ApSecurityPolicy::Wpa3Personal,
        ApBssProtection::default(),
    )
    .unwrap();
    let beacon = &bytes[..len];
    assert_ne!(u16::from_le_bytes([beacon[34], beacon[35]]) & 0x0010, 0);
    assert!(
        beacon
            .windows(AP_WPA3_PERSONAL_RSN_ELEMENT.len())
            .any(|window| window == AP_WPA3_PERSONAL_RSN_ELEMENT)
    );
    assert!(beacon.ends_with(&AP_SAE_H2E_RSNX_ELEMENT));

    let record = parse_management(beacon, 6, -40).unwrap();
    let selected = select_association_rsn(&record, StaSecurityPolicy::Wpa3Personal).unwrap();
    assert_eq!(
        selected.negotiated_akm(),
        AssociationAkm::Sae(SaePwe::HashToElement)
    );
    assert!(selected.security().protects_management());
    assert_eq!(
        select_association_rsn(&record, StaSecurityPolicy::Wpa2Personal)
            .unwrap()
            .negotiated_akm(),
        AssociationAkm::Sae(SaePwe::HashToElement)
    );
}

#[test]
fn builds_the_bounded_wpa2_ht20_beacon() {
    let ap = [0x02, 0, 0, 0, 0, 1];
    let ssid = WifiSsid::new(b"open-radio-ap").unwrap();
    let mut bytes = [0; AP_BEACON_CAPACITY];
    let len = write_ht_beacon(
        &TEST_ADVERTISEMENT,
        &mut bytes,
        ap,
        &ssid,
        crate::channel::Channel::from_wifi_channel(WifiChannel::mhz20(6).unwrap()),
        100,
        2,
        seq(0x0abc),
        ApSecurityPolicy::Wpa2Personal,
        ApBssProtection::default(),
    )
    .unwrap();

    assert_eq!(&bytes[..2], &0x0080_u16.to_le_bytes());
    assert_eq!(&bytes[4..10], &[0xff; 6]);
    assert_eq!(&bytes[10..16], &ap);
    assert_eq!(&bytes[16..22], &ap);
    assert_eq!(&bytes[22..24], &0xabc0_u16.to_le_bytes());
    assert_eq!(dtim(&bytes[..len]), Some((64, 1, 2)));
    assert!(bytes[..len].windows(3).any(|window| window == [42, 1, 0]));
    assert!(
        bytes[..len]
            .windows(22)
            .any(|window| window == AP_WPA2_PERSONAL_RSN_ELEMENT)
    );
    assert!(bytes[..len].windows(3).any(|window| window == [3, 1, 6]));
    assert!(bytes[..len].windows(2).any(|window| window == [45, 26]));
    assert!(bytes[..len].windows(3).any(|window| window == [61, 22, 6]));
}

#[test]
fn ht40_beacon_advertises_the_validated_secondary_channel() {
    let ssid = WifiSsid::new(b"open-radio-ap").unwrap();
    let mut bytes = [0; AP_BEACON_CAPACITY];
    let channel = WifiChannel::new_2_4_ghz(6, WifiChannelWidth::Mhz40Above).unwrap();
    let len = write_ht_beacon(
        &TEST_ADVERTISEMENT,
        &mut bytes,
        [2; 6],
        &ssid,
        crate::channel::Channel::from_wifi_channel(channel),
        100,
        2,
        seq(0),
        ApSecurityPolicy::Wpa2Personal,
        ApBssProtection::default(),
    )
    .unwrap();
    assert!(
        bytes[..len]
            .windows(4)
            .any(|window| window == [45, 26, 0x6e, 0x10])
    );
    assert!(
        bytes[..len]
            .windows(4)
            .any(|window| window == [61, 22, 6, 0x05])
    );
    let ht_capability = bytes[..len]
        .windows(2)
        .position(|window| window == [45, 26])
        .expect("the HT40 beacon includes HT Capabilities");
    assert_eq!(
        bytes[ht_capability + HtDuplicateMcs32::CAPABILITY_IE_BYTE]
            & HtDuplicateMcs32::CAPABILITY_IE_MASK,
        0,
        "the AP must not advertise unqualified local MCS32 reception"
    );
    assert_eq!(
        bytes[ht_capability + 17],
        0x01,
        "the AP must advertise only the implemented equal MCS0..MCS7 sets"
    );
}

#[test]
fn rejects_unrepresentable_beacon_policy_before_mutation() {
    let ssid = WifiSsid::new(b"ap").unwrap();
    let mut bytes = [0xaa; AP_BEACON_CAPACITY];
    assert_eq!(
        write_ht_beacon(
            &TEST_ADVERTISEMENT,
            &mut bytes,
            [0; 6],
            &ssid,
            crate::channel::Channel::from_wifi_channel(WifiChannel::mhz20(14).unwrap()),
            100,
            2,
            seq(0),
            ApSecurityPolicy::Wpa2Personal,
            ApBssProtection::default(),
        ),
        Err(ApBeaconBuildError::InvalidPrimaryChannel)
    );
    assert!(bytes.iter().all(|byte| *byte == 0xaa));
}

#[test]
fn protection_update_rewrites_only_erp_and_ht_operation_after_tim_growth() {
    let mut bytes = [0; AP_BEACON_CAPACITY];
    let len = write_ht_beacon(
        &TEST_ADVERTISEMENT,
        &mut bytes,
        [2; 6],
        &WifiSsid::new(b"ap").unwrap(),
        crate::channel::Channel::from_wifi_channel(WifiChannel::mhz20(6).unwrap()),
        100,
        2,
        seq(0),
        ApSecurityPolicy::Open,
        ApBssProtection::default(),
    )
    .unwrap();
    let mut bitmap = crate::beacon::TimVirtualBitmap::<4>::try_new().unwrap();
    bitmap
        .set(crate::beacon::TimAssociationId::new(25).unwrap(), true)
        .unwrap();
    let len = write_tim_partial_virtual_bitmap(&mut bytes, len, bitmap.partial()).unwrap();
    let before = bytes;

    let protection = ApBssProtection {
        non_erp_present: true,
        erp: ErpProtection::new(true, true),
        ht: HtOperationProtection {
            mode: HtProtectionMode::NonHtMixed,
            non_greenfield_present: true,
        },
    };
    update_bss_protection(&mut bytes[..len], protection).unwrap();
    let erp = bytes[..len]
        .windows(2)
        .position(|window| window == [42, 1])
        .unwrap();
    let ht_operation = bytes[..len]
        .windows(3)
        .position(|window| window == [61, 22, 6])
        .unwrap();
    assert_eq!(bytes[erp + 2], 0x07);
    assert_eq!(bytes[ht_operation + 4], 0x07);
    for index in 0..len {
        let rewritten = index == erp + 2 || index == ht_operation + 4;
        assert_eq!(bytes[index] != before[index], rewritten, "byte {index}");
    }

    assert_eq!(
        update_bss_protection(&mut bytes[..erp], protection),
        Err(ApBeaconProtectionError::MissingProtectionElement)
    );
}

/// The authentication a personal selection negotiated.
trait NegotiatedAkm {
    fn negotiated_akm(&self) -> crate::security::AssociationAkm;
}

impl NegotiatedAkm for crate::station::SelectedRsn {
    fn negotiated_akm(&self) -> crate::security::AssociationAkm {
        match self.security() {
            crate::security::AssociationSecurity::Rsn(association) => association.akm,
            crate::security::AssociationSecurity::Open => panic!("an Open selection has no AKM"),
        }
    }
}

/// The test advertisement with the OFDM rates a 5 GHz BSS carries: 6, 12
/// and 24 Mbit/s basic.
const OFDM_ADVERTISEMENT: crate::ap::profile::Advertisement =
    crate::ap::profile::Advertisement::new(
        crate::ap::profile::LegacyRates::new(
            [0x8c, 0x12, 0x98, 0x24, 0xb0, 0x48, 0x60, 0x6c],
            [0x8c, 0x98, 0xb0, 0x6c],
        ),
        TEST_ADVERTISEMENT.ht,
        TEST_ADVERTISEMENT.wmm,
        0x0001,
    )
    .unwrap();

#[test]
fn a_5_ghz_beacon_carries_no_dsss_parameter_set_erp_or_dsss_rate() {
    use crate::channel::{Channel, ChannelWidth};

    let ap = [0x02, 0, 0, 0, 0, 1];
    let ssid = WifiSsid::new(b"open-radio-ap").unwrap();
    let channel = Channel::ghz5(36, ChannelWidth::Mhz40Above).unwrap();
    let mut bytes = [0; AP_BEACON_CAPACITY];
    // DSSS rates have no place in the band.
    assert_eq!(
        write_ht_beacon(
            &TEST_ADVERTISEMENT,
            &mut bytes,
            ap,
            &ssid,
            channel,
            100,
            1,
            seq(0),
            ApSecurityPolicy::Wpa2Personal,
            ApBssProtection::default(),
        ),
        Err(ApBeaconBuildError::DsssRateIn5Ghz)
    );
    let len = write_ht_beacon(
        &OFDM_ADVERTISEMENT,
        &mut bytes,
        ap,
        &ssid,
        channel,
        100,
        1,
        seq(0),
        ApSecurityPolicy::Wpa2Personal,
        ApBssProtection::default(),
    )
    .unwrap();
    let frame = &mut bytes[..len];
    let ids: [bool; 3] = [3, 42, 61].map(|id| {
        let mut offset = 36;
        let mut found = false;
        while offset + 2 <= frame.len() {
            found |= frame[offset] == id;
            offset += 2 + usize::from(frame[offset + 1]);
        }
        found
    });
    // No DSSS Parameter Set, no ERP; the HT Operation names channel 36 with
    // its secondary above.
    assert_eq!(ids, [false, false, true]);
    let parsed = parse_management(frame, 36, -40).unwrap();
    assert!(
        parsed
            .ht_operation_ie_bytes()
            .is_some_and(|ht| ht[2] == 36 && ht[3] & 0x03 == 1)
    );
    // The protection update needs no ERP element in the band.
    let protection = ApBssProtection {
        non_erp_present: false,
        erp: ErpProtection::new(false, false),
        ht: HtOperationProtection {
            mode: HtProtectionMode::NonHtMixed,
            non_greenfield_present: false,
        },
    };
    assert_eq!(update_bss_protection(frame, protection), Ok(()));
}
