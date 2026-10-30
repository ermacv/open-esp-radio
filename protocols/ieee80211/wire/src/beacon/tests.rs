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
    security::{BssSecurityElements, test_elements},
    ssid::WifiSsid,
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
fn a_wpa3_beacon_carries_its_rsne_and_rsnxe_to_the_scan_record() {
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
        test_elements::WPA3_PERSONAL,
        ApBssProtection::default(),
    )
    .unwrap();
    let beacon = &bytes[..len];
    assert_ne!(u16::from_le_bytes([beacon[34], beacon[35]]) & 0x0010, 0);
    assert!(
        beacon
            .windows(test_elements::WPA3_PERSONAL_RSNE.len())
            .any(|window| window == test_elements::WPA3_PERSONAL_RSNE)
    );
    assert!(beacon.ends_with(&test_elements::SAE_H2E_RSNXE));

    let record = parse_management(beacon, 6).unwrap();
    assert!(record.privacy);
    assert_eq!(record.rsn_ie_bytes(), test_elements::WPA3_PERSONAL_RSNE);
    assert_eq!(record.rsnxe_bytes(), test_elements::SAE_H2E_RSNXE);
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
        test_elements::WPA2_PERSONAL,
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
            .any(|window| window == test_elements::WPA2_PERSONAL_RSNE)
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
        test_elements::WPA2_PERSONAL,
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
            test_elements::WPA2_PERSONAL,
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
        BssSecurityElements::OPEN,
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
            test_elements::WPA2_PERSONAL,
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
        test_elements::WPA2_PERSONAL,
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
    let parsed = parse_management(frame, 36).unwrap();
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

#[test]
fn protection_update_refuses_truncated_elements_before_mutating_the_frame() {
    let mut frame = [0; 66];
    frame[..2].copy_from_slice(&[0x80, 0]);
    frame[36..39].copy_from_slice(&[42, 1, 0]);
    frame[39..63].copy_from_slice(&[
        61, 22, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ]);
    frame[63..].copy_from_slice(&[1, 255, 0]);
    for length in [0, 35, 37, 38, 40, 62, 64, 65, 66] {
        let before = frame;
        assert_eq!(
            update_bss_protection(&mut frame[..length], ApBssProtection::default()),
            Err(ApBeaconProtectionError::MalformedElement)
        );
        assert_eq!(frame, before);
    }
}

#[test]
fn tim_updates_refuse_hostile_lengths_and_truncated_frames_without_moving_bytes() {
    let bitmap = super::TimVirtualBitmap::<2>::try_new().unwrap();
    let partial = bitmap.partial();
    for length in [0, 35, 37, 38, 39, 40, 43, 45, usize::MAX] {
        let mut storage = beacon();
        let before = storage;
        assert_eq!(
            write_tim_partial_virtual_bitmap(&mut storage, length, partial),
            None
        );
        assert_eq!(storage, before);
    }
}

#[test]
fn template_edits_insert_count_and_remove_elements_after_the_tim() {
    use super::{
        BeaconTemplateError, insert_after_tim, remove_elements, write_channel_switch_count,
    };
    let mut bytes = [0; AP_BEACON_CAPACITY];
    let len = write_ht_beacon(
        &TEST_ADVERTISEMENT,
        &mut bytes,
        [0x02, 0, 0, 0, 0, 1],
        &WifiSsid::new(b"ap").unwrap(),
        crate::channel::Channel::from_wifi_channel(WifiChannel::mhz20(6).unwrap()),
        100,
        2,
        seq(1),
        BssSecurityElements::OPEN,
        ApBssProtection::default(),
    )
    .unwrap();
    let original = bytes;
    let (tim, _, _) = dtim(&bytes[..len]).unwrap();
    let after_tim = tim + 2 + usize::from(bytes[tim + 1]);

    // A Channel Switch Announcement: mode, new channel, count.
    let announcement = [37, 3, 1, 11, 5];
    let inserted = insert_after_tim(&mut bytes, len, &announcement).unwrap();
    assert_eq!(inserted, len + announcement.len());
    assert_eq!(&bytes[after_tim..after_tim + 5], &announcement);
    assert_eq!(&bytes[after_tim + 5..inserted], &original[after_tim..len]);

    write_channel_switch_count(&mut bytes, inserted, 2).unwrap();
    assert_eq!(bytes[after_tim + 4], 2);

    let removed = remove_elements(&mut bytes, inserted, &[37]).unwrap();
    assert_eq!(removed, len);
    assert_eq!(bytes, original);
    assert_eq!(write_channel_switch_count(&mut bytes, removed, 2), None);

    // A full template and one without a TIM are refused unchanged.
    assert_eq!(
        insert_after_tim(&mut bytes[..len + 2], len, &announcement),
        Err(BeaconTemplateError::Full)
    );
    let mut no_tim = [0_u8; 40];
    assert_eq!(
        insert_after_tim(&mut no_tim, 36, &announcement),
        Err(BeaconTemplateError::Malformed)
    );
}
