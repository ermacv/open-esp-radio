//! Receive policy and runtime MAC stopping for an installed role.

pub use crate::{
    sniffer::MacSnifferHardware,
    sta_link_policy::{
        StaEspNowRxPolicyHardware, StaLinkRxPolicyHardware, StaNoiseFloorHardware,
        configure_sta_esp_now_receive_policy, configure_sta_link_receive_policy,
        configure_standalone_esp_now_receive_policy,
    },
};

/// Open common gates before BASE with a prepared memory/binding proof,
/// then release MAC RX only after the published ring's walker is live.
pub trait MacRuntimeStartHardware {
    fn prepare_mac_runtime<'a, const COUNT: usize>(
        &mut self,
        ring: oer_esp32s31_hal::ieee80211::dma::rx_ring::RxRingPrepared<'a, COUNT>,
    ) -> oer_esp32s31_hal::ieee80211::dma::rx_ring::RxRingCoreReady<'a, COUNT>;
    fn start_mac_runtime<const COUNT: usize>(&mut self, ring: &crate::rx::RxRingLive<'_, COUNT>);
}

impl MacRuntimeStartHardware for oer_esp32s31_hal::ieee80211::mac::WifiMacHal<'_> {
    fn prepare_mac_runtime<'a, const COUNT: usize>(
        &mut self,
        ring: oer_esp32s31_hal::ieee80211::dma::rx_ring::RxRingPrepared<'a, COUNT>,
    ) -> oer_esp32s31_hal::ieee80211::dma::rx_ring::RxRingCoreReady<'a, COUNT> {
        Self::prepare_mac_runtime(self, ring)
    }

    fn start_mac_runtime<const COUNT: usize>(&mut self, ring: &crate::rx::RxRingLive<'_, COUNT>) {
        Self::start_mac_runtime(self, ring);
    }
}

impl MacRuntimeStartHardware for oer_esp32s31_hal::owner::RadioAccess {
    fn prepare_mac_runtime<'a, const COUNT: usize>(
        &mut self,
        ring: oer_esp32s31_hal::ieee80211::dma::rx_ring::RxRingPrepared<'a, COUNT>,
    ) -> oer_esp32s31_hal::ieee80211::dma::rx_ring::RxRingCoreReady<'a, COUNT> {
        self.wifi_mac_hal().prepare_mac_runtime(ring)
    }

    fn start_mac_runtime<const COUNT: usize>(&mut self, ring: &crate::rx::RxRingLive<'_, COUNT>) {
        self.wifi_mac_hal().start_mac_runtime(ring);
    }
}

/// Vendor-proven outer MAC quiescence used while no RX descriptor epoch is
/// allowed to exist.
///
/// This is the `WIFI_PS_NONE` stop-request transaction from complete
/// `hal_mac_deinit`/`hal_mac_init`, not the four undocumented global MAC
/// disable gates. A caller must request stop while its RX walker is still
/// live, wait until [`Self::mac_runtime_active_state`] reaches zero, and only
/// then withdraw DMA credits. Resume is legal only after a replacement ring
/// is live again.
pub trait MacRuntimeStopHardware {
    fn request_mac_runtime_stop(&mut self);
    fn mac_runtime_active_state(&mut self) -> u8;
    fn resume_mac_runtime(&mut self);
}

impl MacRuntimeStopHardware for oer_esp32s31_hal::ieee80211::mac::WifiMacHal<'_> {
    fn request_mac_runtime_stop(&mut self) {
        self.request_channel_stop();
    }

    fn mac_runtime_active_state(&mut self) -> u8 {
        self.channel_active_state()
    }

    fn resume_mac_runtime(&mut self) {
        let _ = self.restart_after_channel_switch();
    }
}

impl MacRuntimeStopHardware for oer_esp32s31_hal::owner::RadioAccess {
    fn request_mac_runtime_stop(&mut self) {
        self.wifi_mac_hal().request_channel_stop();
    }

    fn mac_runtime_active_state(&mut self) -> u8 {
        self.wifi_mac_hal().channel_active_state()
    }

    fn resume_mac_runtime(&mut self) {
        let _ = self.wifi_mac_hal().restart_after_channel_switch();
    }
}

/// Select the standalone/scan promiscuous receive policy after common init.
///
/// The ownership-bound transaction keeps the hardware-qualified open policy,
/// complete vendor sniffer leaf, misc-class update and device fence together.
/// It leaves queue 0..2 defaults and the MAC runtime stop request intact.
pub fn activate_promiscuous_receive<M: MacSnifferHardware>(mmio: &mut M) {
    mmio.configure_open_promiscuous_receive();
}

/// Restore queue three's normal filters before its RX DMA epoch stops.
pub fn deactivate_promiscuous_receive<M: MacSnifferHardware>(mmio: &mut M) {
    mmio.disable_open_promiscuous_receive();
}
