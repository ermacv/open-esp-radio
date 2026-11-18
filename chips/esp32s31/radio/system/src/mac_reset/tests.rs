extern crate std;

use super::{WifiMacBlock, reset_tuned};
use oer_chips_esp32s31_hal::{
    radio::ieee80211::{
        client::{WifiClocked, WifiClocksOn, WifiCold},
        init::MacBlockReset,
    },
    system::root::RadioHardware,
};
use oer_protocols_ieee80211_wire::channel::{ChannelWidth, Ghz2_4Channel};
use std::vec::Vec;

/// The domain's operation, recording the width code it was handed.
struct Block(Vec<u8>);

impl WifiMacBlock for Block {
    fn reset_wifi_mac_block(&mut self, _clocks: &WifiClocksOn, cbw: u8) -> MacBlockReset {
        self.0.push(cbw);
        MacBlockReset::for_host_model()
    }
}

fn clocked() -> WifiClocked {
    let (_domain, partitions) = RadioHardware::for_host_test()
        .split::<crate::test_clocks::TestClockBackend, _>(
            (),
            oer_chips_esp32s31_hal::radio::domain::transaction::PhyIdentity::for_host_test(),
        )
        .expect("identity capacity");
    WifiClocked::for_host_test(WifiCold::from_partition(partitions.wifi))
}

/// The backend's step hands the domain the width the member's channel was
/// tuned at, lowered as the selection lowered it: a 40 MHz channel's code
/// is not 20 MHz's.
#[test]
fn the_reset_writes_back_the_width_of_the_channel_the_member_is_tuned_to() {
    let wifi = clocked();
    for (width, cbw) in [
        (ChannelWidth::Mhz20, 0),
        (ChannelWidth::Mhz40Above, 2),
        (ChannelWidth::Mhz40Below, 3),
    ] {
        let primary = if width == ChannelWidth::Mhz40Below {
            13
        } else {
            1
        };
        let tuned = Ghz2_4Channel::new(primary, width).expect("a 2.4 GHz channel");
        let mut block = Block(Vec::new());
        let MacBlockReset { .. } = reset_tuned(&mut block, wifi.clocks_on(), tuned);
        assert_eq!(block.0, [cbw], "{width:?}");
        assert_eq!(
            cbw,
            crate::channel::lower_ghz2_4_channel(tuned).cbw,
            "the selection's lowering"
        );
    }
}
