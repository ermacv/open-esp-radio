//! ESP32-S31 lowering of portable Wi-Fi channel definitions.

use oer_ieee80211_mac::{
    channel::{WifiChannel, WifiChannelWidth},
    regulatory::LegalChannel,
};

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

/// The PHY's runtime TX power limit, in quarter-dBm, for `channel`'s
/// ceiling; a ceiling above the limit's range saturates, which the
/// calibrated maximum bounds anyway.
///
/// The ceiling is EIRP, and the PHY target is the power at the antenna
/// port: this applies it with no antenna gain subtracted, so a board with
/// a gain above 0 dBi radiates up to that gain more. The board's antenna
/// gain is not a recovered value yet.
pub(crate) const fn tx_ceiling_quarter_dbm(channel: &LegalChannel) -> i8 {
    channel.max_eirp_dbm().saturating_mul(4)
}

#[cfg(test)]
mod tests;
