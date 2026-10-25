//! The MAC owners over host memory, for the tests of their operations.
//!
//! A test fills each block's memory, runs owner operations and compares the
//! memory before and after, or reads it back through the generated parts'
//! typed readers, so no test names a register address, mask or shift.

use std::{vec, vec::Vec};

use oer_esp32s31_pac::{
    bt_v3_2_baseband::{self, BtV3_2Baseband},
    ieee802154_mac::{self, Ieee802154Mac},
    modem_etm::{self, ModemEtm},
};
use oer_mmio::Host;

use super::{Ieee802154InterruptSetupOwner, Ieee802154TaskOwner};

/// The task owner over host memory.
pub(crate) type HostTask<'a> = Ieee802154TaskOwner<Host<'a>, Host<'a>, Host<'a>>;

/// The inactive interrupt owner over host memory.
pub(crate) type HostSetup<'a> = Ieee802154InterruptSetupOwner<Host<'a>>;

/// The memory of the three blocks the IEEE 802.15.4 partition reaches.
pub(crate) struct HostMemory {
    pub(crate) mac: Vec<u32>,
    pub(crate) etm: Vec<u32>,
    pub(crate) baseband: Vec<u32>,
}

impl HostMemory {
    /// Every word of every block `fill`.
    pub(crate) fn filled(fill: u32) -> Self {
        Self {
            mac: vec![fill; ieee802154_mac::SIZE.div_ceil(4)],
            etm: vec![fill; modem_etm::SIZE.div_ceil(4)],
            baseband: vec![fill; bt_v3_2_baseband::SIZE.div_ceil(4)],
        }
    }

    /// The task owner and the inactive interrupt owner over this memory, as
    /// the partition's split gives them.
    pub(crate) fn owners(&mut self) -> (HostTask<'_>, HostSetup<'_>) {
        let (task, interrupt) = Ieee802154Mac::host(&mut self.mac)
            .expect("memory holds the MAC block")
            .split();
        let (etm, _, _) = ModemEtm::host(&mut self.etm)
            .expect("memory holds the ETM block")
            .divide();
        let (_, rx_info) = BtV3_2Baseband::host(&mut self.baseband)
            .expect("memory holds the baseband block")
            .split_rx_info();
        (
            Ieee802154TaskOwner::from_parts(task, etm, rx_info),
            Ieee802154InterruptSetupOwner::from_part(interrupt),
        )
    }
}

/// The new value of each word that differs between two images of a block,
/// in memory order.
pub(crate) fn changed(before: &[u32], after: &[u32]) -> Vec<u32> {
    before
        .iter()
        .zip(after)
        .filter(|(before, after)| before != after)
        .map(|(_, after)| *after)
        .collect()
}
