//! The MAC owners over host memory, for the tests of their operations.
//!
//! A test fills each block's memory, runs owner operations and compares the
//! memory before and after, or reads it back through the generated parts'
//! typed readers, so no test names a register address, mask or shift.

use std::{vec, vec::Vec};

use oer_esp32s31_pac::{
    bt_bb::BtV32BasebandRxInfo,
    ieee802154::ieee802154_host,
    instances::{bt_bb, ieee802154, modem_etm},
    modem_etm::modem_etm_host,
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
            mac: vec![fill; ieee802154::SIZE.div_ceil(4)],
            etm: vec![fill; modem_etm::SIZE.div_ceil(4)],
            baseband: vec![fill; bt_bb::SIZE.div_ceil(4)],
        }
    }

    /// The task owner and the inactive interrupt owner over this memory, as
    /// the partition's split gives them.
    pub(crate) fn owners(&mut self) -> (HostTask<'_>, HostSetup<'_>) {
        let (task, interrupt) = ieee802154_host(&mut self.mac).expect("memory holds the MAC block");
        let (etm, _, _) = modem_etm_host(&mut self.etm).expect("memory holds the ETM block");
        let rx_info =
            BtV32BasebandRxInfo::host(&mut self.baseband).expect("memory holds the baseband block");
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

/// A hart whose matrix routes nothing and whose lines are all disabled, for
/// the router's proofs in tests: its writes change nothing it reports.
pub(crate) struct NoRoutes;

impl oer_espressif_interrupt_table::Controller for NoRoutes {
    const SOURCES: u16 = 256;
    const LINES: u8 = crate::platform::interrupt_controller::facts::LINES.count;
    const LEVEL_BITS: oer_riscv::clic::LevelBits = oer_riscv::clic::LevelBits::constant::<
        { crate::platform::interrupt_controller::facts::LEVEL_BITS },
    >();
    fn verify_configuration(&self) -> Result<(), oer_espressif_interrupt_table::Error> {
        Ok(())
    }
    fn hart(&self) -> u8 {
        0
    }
    fn route(
        &mut self,
        _source: u16,
        _line: u8,
    ) -> Result<(), oer_espressif_interrupt_table::Error> {
        Ok(())
    }
    fn silence(&mut self, _source: u16) -> Result<(), oer_espressif_interrupt_table::Error> {
        Ok(())
    }
    fn routed(&self, _source: u16) -> Option<u8> {
        None
    }
    fn set_line(
        &mut self,
        _line: u8,
        _state: oer_riscv::clic::LineState,
    ) -> Result<(), oer_espressif_interrupt_table::Error> {
        Ok(())
    }
    fn line(&self, _line: u8) -> oer_riscv::clic::LineState {
        oer_riscv::clic::LineState::DISABLED
    }
}

/// The router's proof that no line receives the MAC's source, over a table
/// without entries.
pub(crate) fn silent<'a>()
-> oer_espressif_interrupt_table::Silent<super::Ieee802154InterruptOwner<Host<'a>>> {
    oer_espressif_interrupt_table::Router::install(NoRoutes, &[])
        .and_then(|router| router.silent())
        .expect("a table without entries leaves the MAC's source silent")
}
