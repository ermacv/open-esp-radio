use super::*;

fn channel(band: Band, number: u8, width: ChannelWidth) -> Channel {
    Channel::new(band, number, width).unwrap()
}

#[test]
fn an_announcement_round_trips_through_its_elements_and_its_action() {
    for (target, mode, count) in [
        (
            channel(Band::Ghz2_4, 11, ChannelWidth::Mhz20),
            ChannelSwitchMode::Continue,
            5,
        ),
        (
            channel(Band::Ghz2_4, 1, ChannelWidth::Mhz40Above),
            ChannelSwitchMode::StopTransmitting,
            1,
        ),
        (
            channel(Band::Ghz5, 48, ChannelWidth::Mhz40Below),
            ChannelSwitchMode::Continue,
            0,
        ),
    ] {
        let announcement = ChannelSwitch::to(target, mode, count);
        let mut elements = [0_u8; 8];
        let length = announcement.encode_elements(&mut elements).unwrap();
        let parsed = parse_channel_switch(&elements[..length]).unwrap().unwrap();
        assert_eq!(parsed, announcement);
        assert_eq!(parsed.target(target.band()), Ok(target));

        let mut action = [0_u8; 10];
        let length = announcement.encode_action(&mut action).unwrap();
        assert_eq!(&action[..2], &[0, 4]);
        assert_eq!(
            parse_channel_switch_action(&action[..length]),
            Ok(Some(announcement))
        );
    }
    // Too short a buffer encodes nothing.
    let announcement = ChannelSwitch::to(
        channel(Band::Ghz2_4, 1, ChannelWidth::Mhz40Above),
        ChannelSwitchMode::Continue,
        3,
    );
    assert_eq!(announcement.encode_elements(&mut [0; 7]), None);
}

#[test]
fn an_extended_announcement_names_its_band_and_width_and_takes_precedence() {
    // A Channel Switch Announcement to 6, then an extended one to 5 GHz
    // channel 40 of class 117 (40 MHz, secondary below).
    let elements = [37, 3, 0, 6, 9, 60, 4, 1, 117, 40, 2, 7, 1, 1];
    let announcement = parse_channel_switch(&elements).unwrap().unwrap();
    assert_eq!(announcement.mode, ChannelSwitchMode::StopTransmitting);
    assert_eq!(announcement.count, 2);
    assert_eq!(
        announcement.target(Band::Ghz2_4),
        Ok(channel(Band::Ghz5, 40, ChannelWidth::Mhz40Below))
    );
    // The Public action form.
    assert_eq!(
        parse_channel_switch_action(&[4, 4, 0, 81, 13, 3]),
        Ok(Some(ChannelSwitch {
            mode: ChannelSwitchMode::Continue,
            channel_number: 13,
            count: 3,
            operating_class: Some(81),
            secondary: SecondaryChannelOffset::None,
        }))
    );
}

#[test]
fn a_plain_announcement_stays_in_its_band_at_20_mhz_without_an_offset() {
    let announcement = parse_channel_switch(&[0, 2, b'h', b'i', 37, 3, 0, 36, 4])
        .unwrap()
        .unwrap();
    assert_eq!(
        announcement.target(Band::Ghz5),
        Ok(channel(Band::Ghz5, 36, ChannelWidth::Mhz20))
    );
    assert_eq!(parse_channel_switch(&[0, 2, b'h', b'i']), Ok(None));
    assert_eq!(parse_channel_switch_action(&[3, 0, 1, 2]), Ok(None));
}

#[test]
fn malformed_and_unrepresentable_announcements_are_refused() {
    // Wrong length, reserved mode, reserved offset, truncated element.
    for elements in [
        &[37, 2, 0, 6][..],
        &[37, 3, 2, 6, 1],
        &[37, 3, 0, 6, 1, 62, 1, 2],
        &[37, 3, 0],
    ] {
        assert_eq!(
            parse_channel_switch(elements),
            Err(ChannelSwitchError::Malformed)
        );
    }
    // An 80 MHz operating class, and a channel the band does not have.
    let wide = parse_channel_switch(&[60, 4, 0, 128, 42, 1])
        .unwrap()
        .unwrap();
    assert_eq!(
        wide.target(Band::Ghz5),
        Err(ChannelSwitchError::UnsupportedOperatingClass(128))
    );
    let nowhere = parse_channel_switch(&[37, 3, 0, 15, 1]).unwrap().unwrap();
    assert!(matches!(
        nowhere.target(Band::Ghz2_4),
        Err(ChannelSwitchError::Channel(_))
    ));
}

#[test]
fn an_extended_announcement_round_trips_with_its_operating_class() {
    for (target, class) in [
        (channel(Band::Ghz5, 36, ChannelWidth::Mhz20), 115),
        (channel(Band::Ghz5, 161, ChannelWidth::Mhz40Below), 127),
        (channel(Band::Ghz5, 100, ChannelWidth::Mhz40Above), 122),
        (channel(Band::Ghz2_4, 6, ChannelWidth::Mhz40Below), 84),
    ] {
        let announcement = ChannelSwitch::extended_to(target, ChannelSwitchMode::Continue, 4);
        assert_eq!(announcement.operating_class, Some(class));
        let mut elements = [0_u8; 6];
        let length = announcement.encode_elements(&mut elements).unwrap();
        assert_eq!(elements[..2], [60, 4]);
        let parsed = parse_channel_switch(&elements[..length]).unwrap().unwrap();
        // The other band's channel is named by the class, not the band the
        // station is on.
        assert_eq!(parsed.target(Band::Ghz2_4), Ok(target));
        assert_eq!(parsed.target(Band::Ghz5), Ok(target));
    }
}

fn read_announcement(announcement: Option<ChannelSwitch>) {
    if let Some(announcement) = announcement {
        let _ = announcement.target(Band::Ghz2_4);
        let _ = announcement.target(Band::Ghz5);
        let _ = announcement.encode_elements(&mut [0; 8]);
        let _ = announcement.encode_action(&mut [0; 10]);
    }
}

fn channel_switch_seeds() -> std::vec::Vec<std::vec::Vec<u8>> {
    std::vec![
        std::vec![37, 3, 0, 6, 9, 60, 4, 1, 117, 40, 2, 7, 1, 1],
        std::vec![0, 2, b'h', b'i', 37, 3, 0, 36, 4],
        std::vec![37, 3, 1, 11, 4, 62, 1, 3],
        std::vec![60, 4, 0, 128, 42, 1],
        std::vec![0, 4, 37, 3, 0, 1, 2],
        std::vec![4, 4, 0, 81, 13, 3],
        std::vec![4, 4, 0, 83, 1, 3, 62, 1, 1],
    ]
}

fn parse_channel_switch_never_panics(count: usize) {
    let seeds = channel_switch_seeds();
    let seeds: std::vec::Vec<&[u8]> = seeds.iter().map(std::vec::Vec::as_slice).collect();
    crate::fuzz::inputs(&seeds, count, 64, |bytes| {
        read_announcement(parse_channel_switch(bytes).ok().flatten());
        read_announcement(parse_channel_switch_action(bytes).ok().flatten());
    });
}

#[test]
fn channel_switch_parsers_never_panic_on_received_bytes() {
    parse_channel_switch_never_panics(crate::fuzz::QUICK);
}

#[test]
#[ignore = "the full run"]
fn channel_switch_parsers_never_panic_on_received_bytes_full() {
    parse_channel_switch_never_panics(crate::fuzz::FULL);
}

#[test]
fn an_announcement_encodes_nothing_into_a_short_buffer() {
    let plain = ChannelSwitch::to(
        channel(Band::Ghz2_4, 6, ChannelWidth::Mhz20),
        ChannelSwitchMode::Continue,
        2,
    );
    assert_eq!(plain.encode_elements(&mut [0; 4]), None);
    assert_eq!(plain.encode_action(&mut [0; 1]), None);
    assert_eq!(plain.encode_action(&mut [0; 6]), None);
    let mut action = [0; 7];
    assert_eq!(plain.encode_action(&mut action), Some(7));
    assert_eq!(action, [0, 4, 37, 3, 0, 6, 2]);
    let extended = ChannelSwitch::extended_to(
        channel(Band::Ghz5, 36, ChannelWidth::Mhz20),
        ChannelSwitchMode::Continue,
        2,
    );
    assert_eq!(extended.encode_elements(&mut [0; 5]), None);
}
