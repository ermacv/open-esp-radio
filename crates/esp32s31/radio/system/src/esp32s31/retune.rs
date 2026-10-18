//! Wi-Fi channel retune on the radio domain, outside the R0 client contract.
//!
//! The recovered retune order (`docs/architecture.md`, retune) stays here,
//! compiled, until `track` and the periodic service gain a retune request at
//! R1. The client lends its stopped register owner for one transaction.

use oer_esp32s31_hal::{domain::RadioDomain, owner::RadioRuntimeOwner};
use oer_esp32s31_phy::{
    ConcurrentWifiChannelError, PhyTargetObserver, RomShortDelay, concurrent::ConcurrentPhy,
    switch_concurrent_wifi_channel,
};
use oer_ieee80211_mac::channel::WifiChannel;

use crate::channel::lower_wifi_channel;

/// Retune an initialized Wi-Fi MAC while its role-specific DMA/IRQ service is
/// stopped and therefore owns no asynchronous access to these registers.
#[expect(dead_code, reason = "retune joins the client contract at R1")]
pub(crate) async fn switch_wifi_channel<O: PhyTargetObserver>(
    timer: &impl oer_time::Timer,
    domain: &mut RadioDomain<ConcurrentPhy>,
    radio: &mut RadioRuntimeOwner,
    channel: WifiChannel,
    observer: &mut O,
) -> Result<(), ConcurrentWifiChannelError> {
    let channel = lower_wifi_channel(channel);
    let mut platform = ();
    let (mut hardware, phy) = radio.channel_hal_with_attachment(&mut platform, domain);
    switch_concurrent_wifi_channel::<RomShortDelay, _, _>(
        timer,
        phy,
        channel.channel_or_frequency,
        channel.cbw,
        &mut hardware,
        observer,
    )
    .await
}

/// [`switch_wifi_channel`] for a register owner published in the radio's
/// arena (`registers`), as an installed lower-MAC port holds it. The port's
/// runner lends its register owner to the switch: the port is enabling, and
/// every port call and interrupt entry that would reach the registers is
/// refused or deferred until the switch returns.
#[expect(dead_code, reason = "retune joins the client contract at R1")]
pub(crate) async fn switch_published_wifi_channel<O: PhyTargetObserver>(
    timer: &impl oer_time::Timer,
    radio: &mut RadioDomain<ConcurrentPhy>,
    registers: &oer_esp32s31_hal::ieee80211::arena::RadioAccess<'_>,
    channel: WifiChannel,
    observer: &mut O,
) -> Result<(), PublishedWifiChannelError> {
    let channel = lower_wifi_channel(channel);
    let mut platform = ();
    let (mut hardware, phy) = registers
        .try_channel_hal_with_attachment(&mut platform, radio)
        .map_err(PublishedWifiChannelError::Registers)?;
    switch_concurrent_wifi_channel::<RomShortDelay, _, _>(
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PublishedWifiChannelError {
    /// The register arena refused the transaction.
    Registers(oer_esp32s31_hal::ieee80211::arena::RadioOwnerArenaError),
    /// The PHY refused the channel.
    Phy(ConcurrentWifiChannelError),
}
