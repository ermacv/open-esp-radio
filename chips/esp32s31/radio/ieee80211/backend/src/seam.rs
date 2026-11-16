//! SCRATCH: the hardware seam the hardware code implements.

use oer_chips_esp32s31_hal::types::{MacHe20Association, MacHeBssColor, MacPti, StaTbttSchedule};

/// The station TBTT schedule of the MAC's interface-zero timer, whose event
/// fires on the power interrupt (`MacPowerInterruptObservation::sta_tbtt`).
pub trait StationTbttHardware {
    fn start_station_tbtt(&mut self, schedule: StaTbttSchedule);
    fn stop_station_tbtt(&mut self);
}

/// The power-save block of every ordinary TX queue.
pub trait TxGateHardware {
    fn set_power_save_tx_block(&mut self, blocked: bool);
}

/// The beacon receive priority registers (`hal_set_rx_beacon_pti`,
/// `hal_clear_rx_beacon_pti`).
pub trait RxBeaconPriorityHardware {
    /// Receive beacons at `pti`, the beacon and the shared priority alike.
    fn set_rx_beacon_pti(&mut self, pti: MacPti);
    /// Withdraw the beacon receive priority request.
    fn clear_rx_beacon_pti(&mut self);
}

/// The station interface's HE association registers
/// (`libnet80211.a[wl_cnx.o]::cnx_connect_to_bss` and `ic_set_sta`).
pub trait StaHeHardware {
    /// Begin an HE BSS's connection before authentication: HE BSSID
    /// matching, the color bitmap cleared, the scanned BSS color.
    fn connect_he20_bss(&mut self, color: MacHeBssColor, enabled: bool, partial: bool);
    /// Install the interface-zero HE20 association in the vendor's order.
    fn install_he20_association(&mut self, association: MacHe20Association);
    /// Remove the interface-zero HE association.
    fn clear_he20_association(&mut self);
}

