//! Driver-local capabilities implemented by the unique task-side radio owner.
//!
//! Each hardware transaction borrows the owner mutably for that call. A TX
//! wait retains its operation token and returns the register borrow before
//! RX is serviced; no RefCell arena or copyable MMIO handle is involved.
//!
//! SOURCE: complete `libpp[wdev.o]::wDev_ProcessFiq` handles RX-success
//! bit `0x4000` before TX-complete bit `0x80`.

use crate::{
    mac::{
        init::{MacRuntimeStartHardware, MacRuntimeStopHardware, MacSnifferHardware},
        rx::{RxBufferFullCounter, RxDma},
    },
    station_tsf::{StationTsfHardware, StationTsfWrite},
};
use oer_chips_esp32s31_hal::{
    radio::ieee80211::{dma::rx_dma::WifiRxDmaEngine, mac::WifiMacHal},
    system::owner::RadioAccess,
};

/// The receive session's two views of the one register owner: the MAC, and
/// the MAC beside the RX walker. Each view borrows the owner for its call.
/// The chip's owner is [`RadioAccess`].
pub trait ReceiveRegisters {
    /// The MAC, as one transaction borrows it.
    type Mac<'a>: MacRuntimeStartHardware
        + MacRuntimeStopHardware
        + MacSnifferHardware
        + RxBufferFullCounter
    where
        Self: 'a;
    /// The RX descriptor walker.
    type RxDma: RxDma;
    fn mac(&mut self) -> Self::Mac<'_>;
    fn split_rx_dma(&mut self) -> (Self::Mac<'_>, &mut Self::RxDma);
    /// The low word of the MAC's local clock.
    fn mac_local_time(&mut self) -> u32;
    /// Sample the MAC's statistics for a trace build's counters.
    #[cfg(oer_trace)]
    fn sample_mac_counts(&mut self, now_us: u32);
    /// A trace build's stall snapshot of the MAC's statistics: the bank
    /// read now and the counters' last periodic sample. Only reads; `None`
    /// where the owner has no bank or no sample yet.
    #[cfg(oer_trace)]
    fn mac_banks(
        &mut self,
    ) -> (
        Option<crate::rx::counters::mac::MacBank>,
        Option<crate::rx::counters::mac::MacSample>,
    );
}

impl ReceiveRegisters for RadioAccess {
    type Mac<'a> = WifiMacHal<'a>;
    type RxDma = WifiRxDmaEngine;
    fn mac(&mut self) -> WifiMacHal<'_> {
        self.wifi_mac_hal()
    }
    fn split_rx_dma(&mut self) -> (WifiMacHal<'_>, &mut WifiRxDmaEngine) {
        RadioAccess::split_rx_dma(self)
    }
    fn mac_local_time(&mut self) -> u32 {
        self.wifi_mac_hal().mac_local_time()
    }
    #[cfg(oer_trace)]
    fn sample_mac_counts(&mut self, now_us: u32) {
        crate::rx::counters::mac::MAC_COUNTS.sample(&self.wifi_mac_hal(), now_us);
    }
    #[cfg(oer_trace)]
    fn mac_banks(
        &mut self,
    ) -> (
        Option<crate::rx::counters::mac::MacBank>,
        Option<crate::rx::counters::mac::MacSample>,
    ) {
        use crate::rx::counters::mac;
        (
            Some(mac::bank(&self.wifi_mac_hal())),
            mac::MAC_COUNTS.last_sample(),
        )
    }
}

impl StationTsfHardware for RadioAccess {
    fn station_tsf(&mut self) -> u64 {
        self.wifi_mac_hal().station_tsf()
    }

    fn set_station_tsf(&mut self, _: StationTsfWrite, value: u64) {
        RadioAccess::set_station_tsf(self, value);
    }
}

impl crate::lower_mac::StationTbttHardware for RadioAccess {
    fn start_station_tbtt(&mut self, schedule: oer_chips_esp32s31_hal::types::StaTbttSchedule) {
        self.station_wake_hal().start_station_tbtt(schedule);
    }

    fn stop_station_tbtt(&mut self) {
        self.station_wake_hal().stop_station_tbtt();
    }
}

impl crate::lower_mac::RxBeaconPriorityHardware for RadioAccess {
    fn set_rx_beacon_pti(&mut self, pti: oer_chips_esp32s31_hal::types::MacPti) {
        self.wifi_mac_hal().set_rx_beacon_pti(pti, pti);
    }

    fn clear_rx_beacon_pti(&mut self) {
        self.wifi_mac_hal().clear_rx_beacon_pti();
    }
}

impl crate::lower_mac::StaHeHardware for RadioAccess {
    fn install_he20_association(
        &mut self,
        association: oer_chips_esp32s31_hal::types::MacHe20Association,
    ) {
        self.wifi_mac_hal().install_he20_association(association);
    }

    fn clear_he20_association(&mut self) {
        self.wifi_mac_hal().clear_he20_association();
    }
}

impl crate::lower_mac::TxGateHardware for RadioAccess {
    fn set_power_save_tx_block(&mut self, blocked: bool) {
        self.wifi_mac_hal().set_power_save_tx_block(blocked);
    }
}
