//! Channel selection while the client owns stopped MAC, DMA and IRQ routes.
//!
//! The role performs stop/restart at its ownership boundary; the actor owns
//! the shared PHY transaction and calibration, as in initial selection.

use oer_esp32s31_hal::{domain::RadioDomain, ieee80211::client::WifiClocked};
use oer_esp32s31_phy::{
    ConcurrentWifiChannelError, PhyTargetObserver, RomShortDelay, concurrent::ConcurrentPhy,
    select_concurrent_wifi_channel,
};
use oer_ieee80211_wire::channel::WifiChannel;
use oer_radio_regulatory::Permitted;

pub(crate) async fn select_wifi_channel<O: PhyTargetObserver>(
    timer: &impl oer_time::Timer,
    domain: &mut RadioDomain<ConcurrentPhy>,
    wifi: &mut WifiClocked,
    channel: Permitted<WifiChannel>,
    observer: &mut O,
) -> Result<(), ConcurrentWifiChannelError> {
    let channel = crate::channel::lower_wifi_channel(channel.channel());
    let mut platform = ();
    let (mut hardware, phy) = wifi.channel_hal_with_attachment(&mut platform, domain);
    select_concurrent_wifi_channel::<RomShortDelay, _, _>(
        timer,
        phy,
        channel.channel_or_frequency,
        channel.cbw,
        &mut hardware,
        observer,
    )
    .await
}
