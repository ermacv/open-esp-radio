use super::*;
use oer_ieee80211_mac::management::MAX_SSID_LEN;

#[test]
fn ssid_is_binary_and_length_checked() {
    let ssid = WifiSsid::new(&[0xff, 0, b'a']).unwrap();
    assert_eq!(ssid.as_bytes(), &[0xff, 0, b'a']);
    assert_eq!(WifiSsid::new(&[]), Err(WifiSsidError::Empty));
    assert!(matches!(
        WifiSsid::new(&[0; MAX_SSID_LEN + 1]),
        Err(WifiSsidError::TooLong { .. })
    ));
}

#[test]
fn scan_channels_are_bounded_and_deduplicated() {
    let channels = StationScanChannels::from_primary_channels(&[1, 6, 6, 11]).unwrap();
    assert_eq!(channels.count(), 3);
    assert!(channels.contains(1));
    assert!(channels.contains(6));
    assert!(channels.contains(11));
    assert!(!channels.contains(14));
    assert_eq!(
        channels.primary_channels().collect::<std::vec::Vec<_>>(),
        [1, 6, 11]
    );
    assert_eq!(channels.primary_channels().len(), 3);
    assert_eq!(
        StationScanChannels::from_primary_channels(&[0]),
        Err(StationScanChannelsError::InvalidPrimary(0))
    );
}

#[test]
fn discovery_keeps_protocol_policy_chip_independent() {
    let ssid = WifiSsid::new(b"portable-station").unwrap();
    let scan = StationScanPolicy::new(
        StationScanChannels::from_primary_channels(&[1, 6, 11]).unwrap(),
        NonZeroU16::new(40).unwrap(),
        Preference::Automatic,
    );
    let discovery = StationDiscovery::new(ssid, scan);
    assert_eq!(discovery.ssid(), ssid);
    assert_eq!(discovery.scan(), scan);
}

#[test]
fn preferred_scan_order_preserves_the_exact_selected_set() {
    let channels = StationScanChannels::from_primary_channels(&[1, 6, 11, 14]).unwrap();
    assert_eq!(
        channels
            .primary_channels_preferred(Some(11))
            .collect::<std::vec::Vec<_>>(),
        [11, 1, 6, 14]
    );
    assert_eq!(
        channels
            .primary_channels_preferred(Some(9))
            .collect::<std::vec::Vec<_>>(),
        [1, 6, 11, 14]
    );
}

#[test]
fn scan_iterator_lengths_and_order_hold_for_every_channel_set() {
    for mask in 1..=ALL_2_4_GHZ_CHANNEL_BITS {
        let selected: std::vec::Vec<u8> = (1..=14)
            .filter(|channel| mask & (1 << (channel - 1)) != 0)
            .collect();
        let channels = StationScanChannels::from_primary_channels(&selected).unwrap();
        assert_eq!(usize::from(channels.count()), selected.len());
        assert!(channels.primary_channels().eq(selected.iter().copied()));
        for preference in [
            None,
            Some(0),
            Some(1),
            Some(7),
            Some(14),
            Some(15),
            Some(255),
        ] {
            let mut expected = selected.clone();
            if let Some(preference) = preference
                && let Some(index) = expected.iter().position(|channel| *channel == preference)
            {
                expected.remove(index);
                expected.insert(0, preference);
            }
            let mut iter = channels.primary_channels_preferred(preference);
            for (index, expected) in expected.iter().copied().enumerate() {
                let remaining = selected.len() - index;
                assert_eq!(iter.size_hint(), (remaining, Some(remaining)));
                assert_eq!(iter.len(), remaining);
                assert_eq!(iter.next(), Some(expected));
            }
            assert_eq!(iter.size_hint(), (0, Some(0)));
            assert_eq!(iter.next(), None);
            assert_eq!(iter.next(), None);
        }
    }
}

#[test]
fn every_out_of_domain_channel_is_refused_before_a_bit_shift() {
    let channels = StationScanChannels::CHANNELS_1_TO_14;
    for invalid in core::iter::once(0).chain(15..=u8::MAX) {
        assert!(!channels.contains(invalid));
        assert_eq!(
            StationScanChannels::from_primary_channels(&[1, invalid]),
            Err(StationScanChannelsError::InvalidPrimary(invalid)),
        );
    }
}
