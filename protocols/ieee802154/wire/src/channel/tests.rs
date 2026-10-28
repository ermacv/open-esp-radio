use super::*;

#[test]
fn only_channels_eleven_through_twenty_six_are_constructible() {
    for raw in u8::MIN..=u8::MAX {
        assert_eq!(Channel::new(raw).is_ok(), (11..=26).contains(&raw));
    }
    for (index, channel) in Channel::ALL.into_iter().enumerate() {
        assert_eq!(usize::from(channel.index()), index);
        assert_eq!(channel.center_frequency_mhz(), 2405 + index as u16 * 5);
    }
}

#[test]
fn channel_spans_two_megahertz_around_its_center() {
    use oer_radio_regulatory::RadioChannel as _;
    let span = Channel::new(26).unwrap().span();
    assert_eq!((span.low_khz(), span.high_khz()), (2_479_000, 2_481_000));
}
