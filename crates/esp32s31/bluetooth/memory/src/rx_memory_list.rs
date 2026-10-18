//! Semantic routing onto the two active ESP32-S31 RX memory lists.

#![forbid(unsafe_code)]

use oer_esp32s31_hal::types::BluetoothMemoryListSelector;

/// One of the two global RX list classes activated by the current S31 memory
/// manager.
///
/// Complete `update_global_rxlink` bodies inspect scheduler-item byte `+0x4d`:
/// scan kind two selects list one and every other active kind selects list two.
/// Selector three remains outside this semantic type because no current caller
/// publishes it to hardware after reset.
///
/// The DTM allocator writes non-scanner kind five but also selects the memory-
/// manager bypass that prevents this global-insertion function from running.
/// DTM therefore has no selector binding in this API: its private RX graph
/// needs a separately proven hardware-publication path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxMemoryListClass {
    /// Scanner scheduler items use positional selector one.
    Scanning,
    /// Non-scanner items admitted to normal global insertion use selector two.
    NonScanning,
}

impl RxMemoryListClass {
    const LINK_STATE_CLASS_MASK: u32 = 0x7000_0000;

    /// Select this class in link-state word `+0x20` bits 30:28, as
    /// `r_ble_lll_mmgmt_update_global_rxlink` does for every role that
    /// receives through a global list.
    pub(crate) const fn select_in_link_state(self, word: u32) -> u32 {
        let class = match self {
            Self::Scanning => 1,
            Self::NonScanning => 2,
        };
        (word & !Self::LINK_STATE_CLASS_MASK) | (class << 28)
    }

    /// The exact active selector chosen by the complete memory-manager body.
    pub const fn selector(self) -> BluetoothMemoryListSelector {
        match self {
            Self::Scanning => BluetoothMemoryListSelector::One,
            Self::NonScanning => BluetoothMemoryListSelector::Two,
        }
    }
}

#[cfg(test)]
mod tests;
