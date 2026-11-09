//! ESP32-S31 lowering of portable Wi-Fi channel definitions.

use oer_protocols_ieee80211_wire::channel::{ChannelWidth, Ghz2_4Channel};

/// Exact arguments accepted by the recovered ESP32-S31 PHY channel root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PhyChannel {
    pub(crate) channel_or_frequency: u16,
    pub(crate) cbw: u8,
}

pub(crate) const fn lower_ghz2_4_channel(channel: Ghz2_4Channel) -> PhyChannel {
    match channel.width() {
        ChannelWidth::Mhz20 => PhyChannel {
            channel_or_frequency: channel.primary() as u16,
            cbw: 0,
        },
        ChannelWidth::Mhz40Above => PhyChannel {
            channel_or_frequency: channel.center_frequency_mhz(),
            cbw: 2,
        },
        ChannelWidth::Mhz40Below => PhyChannel {
            channel_or_frequency: channel.center_frequency_mhz(),
            cbw: 3,
        },
    }
}

#[cfg(test)]
mod tests;
