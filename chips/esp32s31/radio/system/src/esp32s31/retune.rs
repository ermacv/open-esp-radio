//! Channel selection while the client owns stopped MAC, DMA and IRQ routes.
//!
//! The role performs stop/restart at its ownership boundary; the actor owns
//! the shared PHY transaction and calibration, as in initial selection.

use oer_esp32s31_hal::power::PlatformClockBackend;

use oer_esp32s31_hal::{domain::RadioDomain, ieee80211::client::WifiClocked};
use oer_esp32s31_phy::{
    ConcurrentWifiChannelError, PhyTargetObserver, RomShortDelay, concurrent::ConcurrentPhy,
    select_concurrent_wifi_channel,
};
use oer_ieee80211_wire::channel::WifiChannel;
use oer_radio_regulatory::Permitted;

pub(crate) fn select_wifi_channel<B: PlatformClockBackend, O: PhyTargetObserver>(
    timer: &impl oer_cooperate::CooperativeTimer,
    domain: RadioDomain<B, ConcurrentPhy>,
    wifi: &mut WifiClocked,
    channel: Permitted<WifiChannel>,
    observer: &mut O,
) -> impl core::future::Future<
    Output = (
        RadioDomain<B, ConcurrentPhy>,
        Result<(), ConcurrentWifiChannelError>,
    ),
> {
    let transaction = domain.detach(oer_esp32s31_hal::domain::transaction::Operation::Retune);
    async move {
        let channel = crate::channel::lower_wifi_channel(channel.channel());
        let mut platform = ();
        let hardware = wifi.channel_transaction(&mut platform, transaction);
        core::pin::pin!(select_concurrent_wifi_channel::<RomShortDelay, _, _, _>(
            timer,
            channel.channel_or_frequency,
            channel.cbw,
            hardware,
            observer,
        ))
        .await
    }
}
