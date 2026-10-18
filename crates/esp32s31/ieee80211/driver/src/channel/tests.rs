use super::*;

#[test]
fn lowering_contains_the_chip_encoding_in_the_chip_crate() {
    assert_eq!(
        lower_wifi_channel(WifiChannel::mhz20(6).unwrap()),
        PhyChannel {
            channel_or_frequency: 6,
            cbw: 0,
        }
    );
    assert_eq!(
        lower_wifi_channel(WifiChannel::new_2_4_ghz(6, WifiChannelWidth::Mhz40Above).unwrap()),
        PhyChannel {
            channel_or_frequency: 2_447,
            cbw: 2,
        }
    );
}
