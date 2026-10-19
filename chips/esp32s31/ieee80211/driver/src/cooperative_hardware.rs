//! Driver-local capabilities implemented by the unique task-side radio owner.
//!
//! Each hardware transaction borrows the owner mutably for that call. A TX
//! wait retains its operation token and returns the register borrow before
//! RX is serviced; no RefCell arena or copyable MMIO handle is involved.
//!
//! SOURCE: complete `libpp[wdev.o]::wDev_ProcessFiq` handles RX-success
//! bit `0x4000` before TX-complete bit `0x80`.

use crate::station_tsf::{StationTsfHardware, StationTsfWrite};
use oer_esp32s31_hal::owner::RadioAccess;

impl StationTsfHardware for RadioAccess {
    fn station_tsf(&mut self) -> u64 {
        self.wifi_mac_hal().station_tsf()
    }

    fn set_station_tsf(&mut self, _: StationTsfWrite, value: u64) {
        RadioAccess::set_station_tsf(self, value);
    }
}

impl crate::lower_mac::StationTbttHardware for RadioAccess {
    fn start_station_tbtt(&mut self, schedule: oer_esp32s31_hal::types::StaTbttSchedule) {
        self.station_wake_hal().start_station_tbtt(schedule);
    }

    fn stop_station_tbtt(&mut self) {
        self.station_wake_hal().stop_station_tbtt();
    }
}

impl crate::lower_mac::RxBeaconPriorityHardware for RadioAccess {
    fn set_rx_beacon_pti(&mut self, pti: oer_esp32s31_hal::types::MacPti) {
        self.wifi_mac_hal().set_rx_beacon_pti(pti, pti);
    }

    fn clear_rx_beacon_pti(&mut self) {
        self.wifi_mac_hal().clear_rx_beacon_pti();
    }
}

impl crate::lower_mac::TxGateHardware for RadioAccess {
    fn set_power_save_tx_block(&mut self, blocked: bool) {
        self.wifi_mac_hal().set_power_save_tx_block(blocked);
    }
}
