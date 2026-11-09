//! Channel selection while the client owns stopped MAC, DMA and IRQ routes.
//!
//! The role performs stop/restart at its ownership boundary; the actor owns
//! the shared PHY transaction and calibration, as in initial selection.

use oer_chips_esp32s31_hal::power::PlatformClockBackend;

use oer_chips_esp32s31_hal::{radio::domain::RadioDomain, radio::ieee80211::client::WifiClocked};
use oer_chips_esp32s31_radio_phy::{
    ConcurrentWifiChannelError, PhyTargetObserver, RomShortDelay, concurrent::ConcurrentPhy,
    select_concurrent_wifi_channel,
};
use oer_protocols_ieee80211_wire::channel::Ghz2_4Channel;
use oer_protocols_radio_regulatory::Permitted;

pub(crate) fn select_wifi_channel<B: PlatformClockBackend, O: PhyTargetObserver>(
    timer: &impl oer_portable_cooperate::CooperativeTimer,
    domain: RadioDomain<B, ConcurrentPhy>,
    wifi: &mut WifiClocked,
    channel: Permitted<oer_protocols_ieee80211_contracts_port::RegulatedChannel<Ghz2_4Channel>>,
    observer: &mut O,
) -> impl core::future::Future<
    Output = (
        RadioDomain<B, ConcurrentPhy>,
        Result<(), ConcurrentWifiChannelError>,
    ),
> {
    let transaction =
        domain.detach(oer_chips_esp32s31_hal::radio::domain::transaction::Operation::Retune);
    async move {
        let channel = crate::channel::lower_ghz2_4_channel(channel.channel().channel());
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
