//! Exclusive channel owner for bounded standalone ESP-NOW excursions.

use core::future::Future;

use oer_ieee80211_mac::channel::WifiChannel;

/// Retune capability lent to the standalone scheduler.
///
/// The scheduler calls this only after it has stopped ordinary TX, RX DMA and
/// the MAC interrupt route. A successful implementation must update its
/// `current_channel` observation atomically with the completed PHY transition.
/// An error leaves the physical channel unknown and forces the service into
/// sticky quarantine.
pub trait StandaloneEspNowChannelControl<H> {
    type Error;

    fn current_channel(&self) -> WifiChannel;

    fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut H,
        channel: WifiChannel,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a;
}

/// Production S31 channel capability for one standalone ESP-NOW runner.
///
/// It borrows the role-neutral runtime context for the channel observation and
/// leases the shared radio for exactly one retune. It is intentionally supplied
/// only to the opt-in off-channel run method; connected ESP-NOW has no path to
/// construct or consume it. Its retunes wait on the radio's timer `D`.
#[cfg(target_arch = "riscv32")]
pub struct StandaloneEspNowPhyChannelControl<'context, 'observer, Membership, P, C, D, O> {
    context: &'context mut oer_esp32s31_ieee80211::runtime::WifiRuntimeContext<Membership>,
    radio: &'context oer_esp32s31_radio_runtime::RadioSystem<P, C, D>,
    observer: &'observer mut O,
}

#[cfg(target_arch = "riscv32")]
impl<'context, 'observer, Membership, P, C, D, O>
    StandaloneEspNowPhyChannelControl<'context, 'observer, Membership, P, C, D, O>
{
    pub fn new(
        context: &'context mut oer_esp32s31_ieee80211::runtime::WifiRuntimeContext<Membership>,
        radio: &'context oer_esp32s31_radio_runtime::RadioSystem<P, C, D>,
        observer: &'observer mut O,
    ) -> Self {
        Self {
            context,
            radio,
            observer,
        }
    }

    pub const fn current_channel(&self) -> WifiChannel {
        self.context.current_channel()
    }
}

#[cfg(target_arch = "riscv32")]
impl<Membership, P, C, D, O>
    StandaloneEspNowChannelControl<oer_esp32s31_hal::owner::RadioRuntimeOwner>
    for StandaloneEspNowPhyChannelControl<'_, '_, Membership, P, C, D, O>
where
    C: oer_esp32s31_hal::shared_radio::PlatformClockProvider,
    D: oer_time::Timer,
    O: oer_esp32s31_phy::PhyTargetObserver,
{
    type Error = oer_esp32s31_phy::ConcurrentWifiChannelError;

    fn current_channel(&self) -> WifiChannel {
        Self::current_channel(self)
    }

    async fn switch_channel<'a>(
        &'a mut self,
        hardware: &'a mut oer_esp32s31_hal::owner::RadioRuntimeOwner,
        channel: WifiChannel,
    ) -> Result<(), Self::Error> {
        let mut guard = self.radio.lock().await;
        let (lease, platform, _) = guard.parts();
        oer_esp32s31_ieee80211::switch_esp32s31_wifi_channel::<oer_esp32s31_phy::RomShortDelay, _, _>(
            self.radio.timer(),
            lease,
            platform,
            hardware,
            channel,
            self.observer,
        )
        .await?;
        self.context.set_current_channel(channel);
        Ok(())
    }
}
