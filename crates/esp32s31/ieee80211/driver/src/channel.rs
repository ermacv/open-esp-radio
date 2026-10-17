//! ESP32-S31 lowering of portable Wi-Fi channel definitions.

#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::{owner::RadioRuntimeOwner, shared_radio::SharedRadioLease};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_phy::{
    ConcurrentWifiChannelError, PhyShortDelay, PhyTargetObserver, concurrent::ConcurrentPhy,
    switch_concurrent_wifi_channel,
};

use oer_ieee80211_mac::channel::{WifiChannel, WifiChannelWidth};

/// Exact arguments accepted by the recovered ESP32-S31 PHY channel root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyChannel {
    pub channel_or_frequency: u16,
    pub cbw: u8,
}

pub const fn lower_wifi_channel(channel: WifiChannel) -> PhyChannel {
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

/// Retune an initialized Wi-Fi MAC while its role-specific DMA/IRQ service is
/// stopped and therefore owns no asynchronous access to these registers.
///
/// The shared PHY domain is retuned under the arbiter lease; `platform` is
/// the PHY platform token of the shared radio's owner.
#[cfg(target_arch = "riscv32")]
pub async fn switch_esp32s31_wifi_channel<D: PhyShortDelay, P, O: PhyTargetObserver>(
    timer: &impl oer_time::Timer,
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    platform: &mut P,
    radio: &mut RadioRuntimeOwner,
    channel: WifiChannel,
    observer: &mut O,
) -> Result<(), ConcurrentWifiChannelError> {
    let channel = lower_wifi_channel(channel);
    let (mut hardware, phy) = radio.channel_hal_with_attachment(platform, lease);
    switch_concurrent_wifi_channel::<D, _, _>(
        timer,
        phy,
        channel.channel_or_frequency,
        channel.cbw,
        &mut hardware,
        observer,
    )
    .await
}

/// [`switch_esp32s31_wifi_channel`] for a register owner published in the
/// radio's arena (`registers`), as an installed lower-MAC port holds it.
/// The port's runner lends its register owner to the switch: the port is
/// enabling, and every port call and interrupt entry that would reach the
/// registers is refused or deferred until the switch returns.
#[cfg(target_arch = "riscv32")]
pub async fn switch_published_esp32s31_wifi_channel<D: PhyShortDelay, P, O: PhyTargetObserver>(
    timer: &impl oer_time::Timer,
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    platform: &mut P,
    registers: &oer_esp32s31_hal::ieee80211::arena::RadioAccess<'_>,
    channel: WifiChannel,
    observer: &mut O,
) -> Result<(), PublishedWifiChannelError> {
    let channel = lower_wifi_channel(channel);
    let (mut hardware, phy) = registers
        .try_channel_hal_with_attachment(platform, lease)
        .map_err(PublishedWifiChannelError::Registers)?;
    switch_concurrent_wifi_channel::<D, _, _>(
        timer,
        phy,
        channel.channel_or_frequency,
        channel.cbw,
        &mut hardware,
        observer,
    )
    .await
    .map_err(PublishedWifiChannelError::Phy)
}

/// Why a retune through the published register owner failed.
#[cfg(target_arch = "riscv32")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublishedWifiChannelError {
    /// The register arena refused the transaction.
    Registers(oer_esp32s31_hal::ieee80211::arena::RadioOwnerArenaError),
    /// The PHY refused the channel.
    Phy(ConcurrentWifiChannelError),
}

#[cfg(test)]
mod tests;
