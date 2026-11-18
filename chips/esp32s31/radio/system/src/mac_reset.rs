//! The actor's step of a tracked Wi-Fi member's `reset_mac`: the MAC's
//! block reset for the channel the actor tuned the member to.

use oer_chips_esp32s31_hal::radio::ieee80211::{client::WifiClocksOn, init::MacBlockReset};
use oer_protocols_ieee80211_wire::channel::Ghz2_4Channel;

/// The domain's operation that pulses the MAC's block and writes back what
/// the domain keeps inside it for the PHY's lowered channel width `cbw`.
pub(crate) trait WifiMacBlock {
    fn reset_wifi_mac_block(&mut self, clocks: &WifiClocksOn, cbw: u8) -> MacBlockReset;
}

#[cfg(target_arch = "riscv32")]
impl<B: oer_chips_esp32s31_hal::power::PlatformClockBackend> WifiMacBlock
    for oer_chips_esp32s31_hal::radio::domain::RadioDomain<
        B,
        oer_chips_esp32s31_radio_phy::concurrent::ConcurrentPhy,
    >
{
    fn reset_wifi_mac_block(&mut self, clocks: &WifiClocksOn, cbw: u8) -> MacBlockReset {
        oer_chips_esp32s31_hal::radio::domain::RadioDomain::reset_wifi_mac_block(self, clocks, cbw)
    }
}

/// Reset the block for `tuned`, the channel the actor last tuned the member
/// to, lowered as the channel's selection lowered it.
pub(crate) fn reset_tuned(
    block: &mut impl WifiMacBlock,
    clocks: &WifiClocksOn,
    tuned: Ghz2_4Channel,
) -> MacBlockReset {
    block.reset_wifi_mac_block(clocks, crate::channel::lower_ghz2_4_channel(tuned).cbw)
}

#[cfg(test)]
mod tests;
