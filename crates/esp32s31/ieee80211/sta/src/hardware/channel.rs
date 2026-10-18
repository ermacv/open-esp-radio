//! Wi-Fi channel transactions used by ESP32-S31 station scan and reconnect.

use oer_esp32s31_hal::{
    ieee80211::arena::RadioAccess, owner::RadioRuntimeOwner, shared_radio::SharedRadioLease,
};

use oer_esp32s31_phy::{
    ConcurrentWifiChannelError, PhyTargetObserver, PhyTargetPortError, RomShortDelay,
    concurrent::ConcurrentPhy, select_concurrent_wifi_channel, switch_concurrent_wifi_channel,
};

/// One Wi-Fi channel transaction per call on the shared radio domain.
///
/// The arbiter lease is taken per transaction, as the vendor `phy_lock`
/// scope is, so another radio client can use the shared domain between the
/// channels of one scan. The caller passes the lease and the PHY platform
/// token it grants. The transactions wait on the image's monotonic `timer`
/// and complete short settles through the ROM delay.
pub struct ScanPhy<O, T> {
    observer: O,
    timer: T,
}

impl<O, T> ScanPhy<O, T>
where
    O: PhyTargetObserver,
    T: oer_time::Timer,
{
    pub const fn new(observer: O, timer: T) -> Self {
        Self { observer, timer }
    }

    /// Retune the shared domain while the caller still owns a cold, stopped
    /// MAC.
    pub async fn select_channel<P>(
        &mut self,
        lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
        platform: &mut P,
        channel_or_frequency: u16,
        cbw: u8,
        radio: &mut RadioRuntimeOwner,
    ) -> Result<(), ConcurrentWifiChannelError> {
        let (mut hardware, phy) = radio.channel_hal_with_attachment(platform, lease);
        select_concurrent_wifi_channel::<RomShortDelay, _, _>(
            &self.timer,
            phy,
            channel_or_frequency,
            cbw,
            &mut hardware,
            &mut self.observer,
        )
        .await
    }

    /// Stop the MAC, retune the shared domain and restore the qualified REGDMA
    /// link.
    // CAPABILITY: channel-selection-switch
    pub async fn switch_channel<P>(
        &mut self,
        lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
        platform: &mut P,
        channel_or_frequency: u16,
        cbw: u8,
        radio: &mut RadioRuntimeOwner,
    ) -> Result<(), ConcurrentWifiChannelError> {
        let (mut hardware, phy) = radio.channel_hal_with_attachment(platform, lease);
        switch_concurrent_wifi_channel::<RomShortDelay, _, _>(
            &self.timer,
            phy,
            channel_or_frequency,
            cbw,
            &mut hardware,
            &mut self.observer,
        )
        .await
    }

    /// Stop, retune and restart through the arena's serialized channel-only
    /// capability. No PAC owner or generic register borrow crosses this API.
    pub async fn switch_published_channel<P>(
        &mut self,
        lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
        platform: &mut P,
        channel_or_frequency: u16,
        cbw: u8,
        access: RadioAccess<'_>,
    ) -> Result<(), ConcurrentWifiChannelError> {
        let (mut hardware, phy) = access
            .try_channel_hal_with_attachment(platform, lease)
            .map_err(|_| {
                ConcurrentWifiChannelError::Failed(
                    PhyTargetPortError::HardwareCapabilityUnavailable,
                )
            })?;
        switch_concurrent_wifi_channel::<RomShortDelay, _, _>(
            &self.timer,
            phy,
            channel_or_frequency,
            cbw,
            &mut hardware,
            &mut self.observer,
        )
        .await
    }

    /// Return the observer after the scan transaction has stopped RX and
    /// selected its candidate.
    pub fn into_observer(self) -> O {
        self.observer
    }
}
