//! Typed snapshots of the complete Wi-Fi MAC TX statistics dump.

use crate::ieee80211::registers::{MacPlacement, WifiRadioRegisters};

/// Value-only image of the four words printed by the vendor
/// `dbg_lmac_hw_statis_dump` routine.
///
/// The abbreviated `track` and `trcts` labels are deliberately retained: the
/// reviewed blob proves the register identity, but not a longer semantic name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacTxStatisticsSnapshot {
    pub tx_rts: u32,
    pub tx_cts: u32,
    pub track: u32,
    pub trcts: u32,
}

impl MacTxStatisticsSnapshot {
    /// Return wrapping increments since an earlier hardware snapshot.
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            tx_rts: self.tx_rts.wrapping_sub(earlier.tx_rts),
            tx_cts: self.tx_cts.wrapping_sub(earlier.tx_cts),
            track: self.track.wrapping_sub(earlier.track),
            trcts: self.trcts.wrapping_sub(earlier.trcts),
        }
    }
}

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Read the complete vendor-labelled TX statistics bank.
    ///
    /// SOURCE: complete `libpp[pp_debug.o]::dbg_lmac_hw_statis_dump`.
    pub fn tx_statistics_snapshot(&self) -> MacTxStatisticsSnapshot {
        let registers = &self.mac.wifi_mac_tx_statistics;
        MacTxStatisticsSnapshot {
            tx_rts: registers.read_tx_rts().value(),
            tx_cts: registers.read_tx_cts().value(),
            track: registers.read_track().value(),
            trcts: registers.read_trcts().value(),
        }
    }
}

#[cfg(test)]
mod tests;
