use super::*;

#[test]
fn channel_fourteen_has_its_nonuniform_frequency() {
    let channel = WifiChannel::mhz20(14).unwrap();
    assert_eq!(channel.primary_frequency_mhz(), 2_484);
    assert_eq!(channel.center_frequency_mhz(), 2_484);
}

#[test]
fn forty_megahertz_geometry_is_bounded_to_real_secondary_channels() {
    let above = WifiChannel::new_2_4_ghz(9, WifiChannelWidth::Mhz40Above).unwrap();
    let below = WifiChannel::new_2_4_ghz(5, WifiChannelWidth::Mhz40Below).unwrap();
    assert_eq!(above.center_frequency_mhz(), 2_462);
    assert_eq!(below.center_frequency_mhz(), 2_422);
    assert_eq!(
        WifiChannel::new_2_4_ghz(10, WifiChannelWidth::Mhz40Above),
        Err(WifiChannelError::InvalidSecondary {
            primary: 10,
            width: WifiChannelWidth::Mhz40Above,
        })
    );
    assert_eq!(
        WifiChannel::new_2_4_ghz(4, WifiChannelWidth::Mhz40Below),
        Err(WifiChannelError::InvalidSecondary {
            primary: 4,
            width: WifiChannelWidth::Mhz40Below,
        })
    );
}

#[test]
fn every_2_4_ghz_channel_converts_both_ways() {
    for primary in 1..=14 {
        for width in [
            ChannelWidth::Mhz20,
            ChannelWidth::Mhz40Above,
            ChannelWidth::Mhz40Below,
        ] {
            let legacy = WifiChannel::new_2_4_ghz(primary, width);
            let channel = Channel::ghz2_4(primary, width);
            assert_eq!(legacy.is_ok(), channel.is_ok(), "{primary} {width:?}");
            if let (Ok(legacy), Ok(channel)) = (legacy, channel) {
                assert_eq!(Channel::from(legacy), channel);
                assert_eq!(WifiChannel::try_from(channel), Ok(legacy));
                assert_eq!(channel.band(), Band::Ghz2_4);
                assert_eq!(
                    channel.center_frequency_mhz(),
                    legacy.center_frequency_mhz()
                );
            }
        }
    }
    assert_eq!(
        Channel::ghz2_4(15, ChannelWidth::Mhz20),
        Err(ChannelError::InvalidNumber {
            band: Band::Ghz2_4,
            number: 15
        })
    );
}

#[test]
fn five_gigahertz_channels_follow_the_global_operating_classes() {
    let twenty: [u8; 28] = [
        36, 40, 44, 48, 52, 56, 60, 64, 100, 104, 108, 112, 116, 120, 124, 128, 132, 136, 140, 144,
        149, 153, 157, 161, 165, 169, 173, 177,
    ];
    for number in 0..=u8::MAX {
        let valid = twenty.contains(&number);
        assert_eq!(
            Channel::ghz5(number, ChannelWidth::Mhz20).is_ok(),
            valid,
            "{number}"
        );
    }
    for pair in twenty.chunks(2) {
        let (lower, upper) = (pair[0], pair[1]);
        let above = Channel::ghz5(lower, ChannelWidth::Mhz40Above).unwrap();
        let below = Channel::ghz5(upper, ChannelWidth::Mhz40Below).unwrap();
        assert_eq!(above.center_frequency_mhz(), below.center_frequency_mhz());
        assert!(Channel::ghz5(lower, ChannelWidth::Mhz40Below).is_err());
        assert!(Channel::ghz5(upper, ChannelWidth::Mhz40Above).is_err());
    }
    let channel = Channel::ghz5(36, ChannelWidth::Mhz40Above).unwrap();
    assert_eq!(channel.primary_frequency_mhz(), 5_180);
    assert_eq!(channel.center_frequency_mhz(), 5_190);
    assert_eq!(channel.wifi_channel(), None);
    assert!(WifiChannel::try_from(channel).is_err());
}
