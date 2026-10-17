//! Ownership boundary for bounded complete `hal_he_init` transactions.

use oer_esp32s31_hal::ieee80211::mac::WifiMacColdHal;
use oer_esp32s31_hal::types::{MAC_TX_POWER_RATE_COUNT, MacTxPowerPair, MacTxPowerTable};

pub trait MacColdHeHardware {
    fn initialize_he_prefix(&mut self);
    fn initialize_tx_power(&mut self, table: &MacTxPowerTable);
    fn initialize_he_suffix(&mut self);
}

/// Calibrated two-byte target-power producer owned by the PHY/platform layer.
///
/// This replaces the direct MAC dependency on ROM `phy_get_max_pwr` and on the
/// vendor global `s_phy_get_max_pwr`. The PHY supplies an owned Rust profile;
/// the MAC owns the resulting fixed table and never borrows PHY calibration
/// state.
pub trait MacTxPowerSource {
    fn mac_tx_power_pair(&mut self, rate: u8) -> MacTxPowerPair;
}

pub fn query_tx_power_table(source: &mut impl MacTxPowerSource) -> MacTxPowerTable {
    let mut entries = [MacTxPowerPair::ZERO; MAC_TX_POWER_RATE_COUNT];
    for (rate, entry) in entries.iter_mut().enumerate() {
        *entry = source.mac_tx_power_pair(rate as u8);
    }
    MacTxPowerTable::new(entries)
}

/// Preserve the unconditional PHY-query traversal in `dbg_read_tx_power`.
///
/// SOURCE: complete pinned `libpp.a[hal_debug.o]` body recorded as
/// `BLOB_LIBPP_DBG_READ_TX_POWER`. Its results are discarded. The rev0 ROM
/// implementation also reasserts the same WORD_7428_OPAQUE fields already
/// owned by open PHY initialization; those redundant RMWs are not part of this
/// PHY-data query boundary.
pub fn run_tx_power_diagnostic_queries(source: &mut impl MacTxPowerSource) {
    for rate in 0..26 {
        if rate != 4 {
            let _ = source.mac_tx_power_pair(rate);
        }
    }
}

impl MacColdHeHardware for WifiMacColdHal<'_> {
    fn initialize_he_prefix(&mut self) {
        WifiMacColdHal::initialize_he_prefix(self);
    }

    fn initialize_tx_power(&mut self, table: &MacTxPowerTable) {
        WifiMacColdHal::initialize_tx_power(self, table);
    }

    fn initialize_he_suffix(&mut self) {
        WifiMacColdHal::initialize_he_suffix(self);
    }
}
