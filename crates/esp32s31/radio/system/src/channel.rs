//! ESP32-S31 lowering of portable Wi-Fi channel definitions.

use oer_ieee80211_mac::channel::{WifiChannel, WifiChannelWidth};

/// Exact arguments accepted by the recovered ESP32-S31 PHY channel root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PhyChannel {
    pub(crate) channel_or_frequency: u16,
    pub(crate) cbw: u8,
}

pub(crate) const fn lower_wifi_channel(channel: WifiChannel) -> PhyChannel {
    match channel.width() {
        WifiChannelWidth::Mhz20 => PhyChannel {
            channel_or_frequency: channel.primary() as u16,
            cbw: 0,
        },
        WifiChannelWidth::Mhz40Above => PhyChannel {
            channel_or_frequency: channel.center_frequency_mhz(),
            cbw: 2,
        },
        WifiChannelWidth::Mhz40Below => PhyChannel {
            channel_or_frequency: channel.center_frequency_mhz(),
            cbw: 3,
        },
    }
}

#[cfg(test)]
mod tests;
