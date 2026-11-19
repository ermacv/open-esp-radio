//! The MAC owners over host memory, for the tests of their operations.
//!
//! A test fills each block's memory, runs owner operations and compares the
//! memory before and after, or reads it back through the generated parts'
//! typed readers, so no test names a register address, mask or shift.

use std::{vec, vec::Vec};

use oer_chips_esp32s31_pac::{
    bt_bb::BtV32BasebandRxInfo,
    ieee802154::ieee802154_host,
    instances::{bt_bb, ieee802154, modem_etm},
    modem_etm::modem_etm_host,
};
use oer_portable_mmio::Host;

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

/// The numbers `0..N`, number `n` at `n`.
const fn numbers<const N: usize>() -> [u16; N] {
    let mut all = [0; N];
    let mut rest: &mut [u16] = &mut all;
    let mut number = 0;
    while let [slot, tail @ ..] = rest {
        *slot = number;
        rest = tail;
        number += 1;
    }
    all
}

/// The host stub's 256 sources.
const NO_ROUTE_SOURCES: [u16; 256] = numbers::<256>();
/// The host stub's lines, the chip's count of them.
const NO_ROUTE_LINES: [u16; crate::system::platform::interrupt_controller::facts::LINES.count
    as usize] = numbers();

impl oer_chips_family_platform_interrupt_table::Controller for NoRoutes {
    type Source = u16;
    type Line = u16;
    const SOURCES: &'static [u16] = &NO_ROUTE_SOURCES;
    const LINES: &'static [u16] = &NO_ROUTE_LINES;
    const LEVEL_BITS: oer_portable_riscv::clic::LevelBits =
        oer_portable_riscv::clic::LevelBits::constant::<
            { crate::system::platform::interrupt_controller::facts::LEVEL_BITS },
        >();
    fn verify_configuration(&self) -> Result<(), oer_chips_family_platform_interrupt_table::Error> {
        Ok(())
    }
    fn hart(&self) -> u8 {
        0
    }
    fn route(&mut self, _source: u16, _line: u16) {}
    fn silence(&mut self, _source: u16) {}
    fn routed(&self, _source: u16) -> Option<u8> {
        None
    }
    fn bind(&mut self, _line: u16, _control: oer_portable_riscv::clic::Control) {}
    fn bind_task(&mut self, _line: u16, _control: oer_portable_riscv::clic::Control) {}
    fn disable(&mut self, _line: u16) {}
    /// Every line reads disabled: a binding is never observed.
    fn check(
        &self,
        _line: u16,
        expected: oer_chips_family_platform_interrupt_table::LineExpectation,
    ) -> Result<(), oer_chips_family_platform_interrupt_table::LineObservation> {
        match expected {
            oer_chips_family_platform_interrupt_table::LineExpectation::Disabled => Ok(()),
            oer_chips_family_platform_interrupt_table::LineExpectation::Bound(_)
            | oer_chips_family_platform_interrupt_table::LineExpectation::Task(_) => {
                Err(oer_chips_family_platform_interrupt_table::LineObservation::of_word(0))
            }
        }
    }
}

/// The router's proof that no line receives the MAC's source, over a table
/// without entries.
pub(crate) fn silent<'a>()
-> oer_chips_family_platform_interrupt_table::Silent<super::Ieee802154InterruptOwner<Host<'a>>> {
    oer_chips_family_platform_interrupt_table::Router::<_, 0, _>::install(
        NoRoutes,
        oer_chips_family_platform_interrupt_table::NoBindings,
    )
    .and_then(|router| router.silent())
    .expect("a table without entries leaves the MAC's source silent")
}

/// Drive the startup atom contract on host owners, testing every phase.
pub(crate) fn arm<'a>(
    setup: HostSetup<'a>,
    task: HostTask<'a>,
) -> (
    super::Ieee802154ArmedTaskOwner<Host<'a>, Host<'a>, Host<'a>>,
    super::Ieee802154InterruptOwner<Host<'a>>,
) {
    use oer_chips_family_radio_ieee802154_values::Ieee802154InterruptStartup as _;
    type P<'a> = super::Ieee802154StartupPort<Host<'a>, Host<'a>, Host<'a>>;
    let p = P::mask_events(setup, task);
    let p = P::runtime_tx_aborts(p);
    let p = P::runtime_rx_aborts(p);
    let p = P::order_before_stop(p);
    let p = P::stop_operation(p);
    let p = P::stop_timer0(p);
    let p = P::stop_timer1(p);
    let p = P::order_before_discard(p);
    let p = P::discard_startup_events(p);
    P::finish(p)
}
/// Drive the teardown atom contract, retaining its affine sample through the masks.
pub(crate) fn deactivate<'a>(
    interrupt: super::Ieee802154InterruptOwner<Host<'a>>,
    task: &mut HostTask<'a>,
) -> HostSetup<'a> {
    use oer_chips_family_radio_ieee802154_values::Ieee802154InterruptTeardown as _;
    type P<'a, 'b> = super::Ieee802154TeardownPort<'b, Host<'a>, Host<'a>, Host<'a>>;
    let p = P::stop_operation(interrupt, task);
    let p = P::stop_timer0(p);
    let p = P::stop_timer1(p);
    let p = P::order_before_sample(p);
    let p = P::sample_events(p);
    let p = P::mask_events(p);
    let p = P::mask_tx_aborts(p);
    let p = P::mask_rx_aborts(p);
    let p = P::acknowledge_events(p);
    P::finish(p)
}
