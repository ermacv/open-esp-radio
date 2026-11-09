use super::*;

#[test]
fn lowering_contains_the_chip_encoding_in_the_chip_crate() {
    assert_eq!(
        lower_ghz2_4_channel(Ghz2_4Channel::mhz20(6).unwrap()),
        PhyChannel {
            channel_or_frequency: 6,
            cbw: 0,
        }
    );
    assert_eq!(
        lower_ghz2_4_channel(Ghz2_4Channel::new(6, ChannelWidth::Mhz40Above).unwrap()),
        PhyChannel {
            channel_or_frequency: 2_447,
            cbw: 2,
        }
    );
}
