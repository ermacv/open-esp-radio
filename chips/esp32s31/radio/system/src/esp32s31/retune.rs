//! Wi-Fi channel retune on the radio domain, which no client request reaches yet.
//!
//! The recovered retune order (`docs/architecture.md`, retune). The client
//! transfers its stopped register owner into the complete transaction.

use oer_esp32s31_hal::{domain::RadioDomain, owner::RadioAccess};
use oer_esp32s31_phy::{
    ConcurrentWifiChannelError, PhyTargetObserver, RomShortDelay, concurrent::ConcurrentPhy,
    switch_concurrent_wifi_channel,
};
use oer_ieee80211_mac::channel::WifiChannel;
use oer_radio_regulatory::Permitted;

use crate::channel::lower_wifi_channel;

/// Retune an initialized Wi-Fi MAC while its role-specific DMA/IRQ service is
/// stopped and therefore owns no asynchronous access to these registers.
#[expect(dead_code, reason = "no client request retunes yet")]
pub(crate) async fn switch_wifi_channel<O: PhyTargetObserver>(
    timer: &impl oer_time::Timer,
    domain: &mut RadioDomain<ConcurrentPhy>,
    mut radio: RadioAccess,
    channel: Permitted<WifiChannel>,
    observer: &mut O,
) -> (RadioAccess, Result<(), ConcurrentWifiChannelError>) {
    let channel = lower_wifi_channel(channel.channel());
    let mut platform = ();
    let (mut hardware, phy) = radio.channel_hal_with_attachment(&mut platform, domain);
    let result = switch_concurrent_wifi_channel::<RomShortDelay, _, _>(
        timer,
        phy,
        channel.channel_or_frequency,
        channel.cbw,
        &mut hardware,
        observer,
    )
    .await;
    (radio, result)
}
