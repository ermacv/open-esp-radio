//! Driver-facing owners of the dedicated IEEE 802.15.4 MAC registers.
//!
//! The PAC hands the IEEE 802.15.4 partition over as its generated parts
//! ([`Ieee802154Partition`]): the MAC block, the modem ETM's channels zero
//! and one and the shared baseband's receive-information read. This module
//! splits the MAC into its generated task and interrupt sides and gives each
//! owner the register operations of the pinned public LL
//! (`ieee802154_common_ll.h`), written over the parts' field accessors:
//!
//! - runtime `EVENT_STATUS` acknowledgement consumes a saved affine sample;
//!   the pre-bind startup step instead discards all fourteen stale lanes
//!   without a sample while delivery remains masked;
//! - the ETM channel-enable set and clear words are written only by their
//!   channels' trigger strobes;
//! - the frame-buffer address registers take only a `DmaWindowAddress`;
//! - the interrupt side holds `EVENT_STATUS` and reads the RX/TX status and
//!   ED result words; the task side holds every other register.
//!
//! The armed interrupt owner has two homes. An interrupt table's handler
//! holds it once `oer_espressif_interrupt_table::Router::bind` placed it, and the task
//! activates its affine startup task owner with the binding's `Bound` proof,
//! publishing the same ten runtime events as later sessions
//! ([`Ieee802154TaskOwner::enable_events`]) and mask them again at the end
//! of each session ([`Ieee802154TaskOwner::mask_events`]). Or the engine pair
//! of the [`ll`](crate::ieee802154::ll) backend holds it with the router's
//! `Silent` proof and polls its events. None of these types can
//! be constructed outside the HAL:
//! `Ieee802154FoundationConfigured::into_operational` hands them out and
//! `Ieee802154FoundationConfigured::from_operational` takes both back, so
//! they never exist apart from their partition.
//!
//! Each owner is generic over where its blocks lie; production uses the
//! chip's addresses, host tests host memory.

#![forbid(unsafe_code)]

use oer_esp32s31_pac::{
    bt_bb::BtV32BasebandRxInfo,
    ieee802154,
    ieee802154::{Ieee802154MacIrq, Ieee802154MacTask},
    modem_etm::ModemEtmIeee802154,
};
use oer_espressif_ieee802154_mac_values::{
    Ieee802154InterruptActivationPlan, Ieee802154InterruptInitializationPort,
    Ieee802154InterruptTransitionPort, execute_interrupt_deactivation,
    execute_interrupt_initialization,
};
use oer_espressif_interrupt_table::{Bound, Silent};
use oer_mmio::{Fixed, Placement};

mod etm;
mod interrupt;
mod task;

pub use interrupt::Ieee802154EventSnapshot;
pub use task::Ieee802154PowerSequence;
pub(crate) use task::{Ieee802154FoundationSnapshot, Ieee802154Pti};

/// Where the chip places the IEEE 802.15.4 MAC block.
pub type MacPlacement = Fixed<{ oer_esp32s31_pac::instances::ieee802154::BASE }>;

/// Where the chip places the modem ETM block.
pub type EtmPlacement = Fixed<{ oer_esp32s31_pac::instances::modem_etm::BASE }>;

/// Where the chip places the shared BTBB baseband block.
pub type RxInfoPlacement = Fixed<{ oer_esp32s31_pac::instances::bt_bb::BASE }>;

/// Exclusive task-side owner of the IEEE 802.15.4 MAC registers: every MAC
/// register but `EVENT_STATUS`, the ETM channels zero and one and the
/// baseband's receive-information read.
///
/// Every operation is expressed in the HAL's semantic vocabulary; drivers
/// never name a register-level value.
#[must_use = "the IEEE 802.15.4 task owner must be returned to its foundation owner"]
pub struct Ieee802154TaskOwner<
    P: Placement = MacPlacement,
    E: Placement = EtmPlacement,
    B: Placement = RxInfoPlacement,
> {
    mac: Ieee802154MacTask<P>,
    etm: ModemEtmIeee802154<E>,
    rx_info: BtV32BasebandRxInfo<B>,
}

/// Affine task ownership after masked pre-bind startup discard. Operations
/// become available only after activation with `Bound` or a polling pair's `Silent`.
///
/// ```compile_fail,E0599
/// use oer_esp32s31_hal::ieee802154::mac::Ieee802154ArmedTaskOwner;
/// fn too_early(mut startup: Ieee802154ArmedTaskOwner) {
///     startup.mask_events();
/// }
/// ```
///
/// ```compile_fail,E0382
/// use oer_esp32s31_hal::ieee802154::mac::{Ieee802154ArmedTaskOwner, Ieee802154InterruptOwner};
/// use oer_espressif_interrupt_table::{Bound, Entry};
/// fn twice<I: Entry<Part = Ieee802154InterruptOwner>>(startup: Ieee802154ArmedTaskOwner, bound: &Bound<I>) {
///     let _first = startup.activate(bound);
///     let _second = startup.activate(bound);
/// }
/// ```
///
/// ```compile_fail,E0599
/// use oer_esp32s31_hal::ieee802154::mac::Ieee802154TaskOwner;
/// fn no_general_ack(task: &mut Ieee802154TaskOwner) {
///     task.clear_events();
/// }
/// ```
#[must_use = "bind the IRQ owner or construct its silent polling pair"]
pub struct Ieee802154ArmedTaskOwner<
    P: Placement = MacPlacement,
    E: Placement = EtmPlacement,
    B: Placement = RxInfoPlacement,
> {
    task: Ieee802154TaskOwner<P, E, B>,
}

impl<P: Placement, E: Placement, B: Placement> Ieee802154ArmedTaskOwner<P, E, B> {
    /// Publish the ten runtime events after the IRQ owner has moved into
    /// its permanent handler. This accesses only task-owned enable registers;
    /// it never borrows the handler cell or grants acknowledgement authority.
    pub fn activate<I: oer_espressif_interrupt_table::Entry<Part = Ieee802154InterruptOwner<P>>>(
        mut self,
        _bound: &Bound<I>,
    ) -> Ieee802154TaskOwner<P, E, B> {
        self.task.publish_runtime_events();
        self.task
    }

    pub(crate) fn into_polled(
        self,
        interrupts: Ieee802154InterruptOwner<P>,
        silent: Silent<Ieee802154InterruptOwner<P>>,
    ) -> super::ll::Ieee802154MacParts<P, E, B> {
        (self.task, interrupts, silent)
    }
}

/// Inactive IEEE 802.15.4 interrupt ownership: the MAC's interrupt side
/// while no handler holds it.
#[must_use = "the inactive IEEE 802.15.4 interrupt owner must be returned to its foundation owner"]
pub struct Ieee802154InterruptSetupOwner<P: Placement = MacPlacement> {
    mac: Ieee802154MacIrq<P>,
}

/// Armed IEEE 802.15.4 interrupt owner: the MAC's interrupt part
/// (`ZB_MAC`), `EVENT_STATUS` and the status words it observes.
///
/// Task command, DMA, policy and event-enable operations are absent from
/// this type. An image's interrupt table binds it by value once, and its
/// handler holds it from then on; the task enables the events with the
/// binding's proof:
///
/// ```no_run
/// use oer_esp32s31_hal::ieee802154::mac::{
///     Ieee802154ArmedTaskOwner,
///     Ieee802154InterruptOwner, Ieee802154InterruptSetupOwner, Ieee802154TaskOwner,
/// };
/// use oer_esp32s31_hal::platform::InterruptController;
/// use oer_espressif_interrupt_table::{BindError, Bound, Router};
/// # const LINES: oer_espressif_interrupt_table::Lines =
/// #     oer_espressif_interrupt_table::Lines { first: 16, count: 48, levels: 8, harts: oer_esp32s31_pac::HARTS as u8, tie_break: oer_espressif_interrupt_table::TieBreak::HighestLine };
/// use oer_espressif_ieee802154_mac_values::Ieee802154EventMask;
/// pub fn on_ieee802154(mac: &mut Ieee802154InterruptOwner) {
///     // Acknowledge the sampled named events; latch them for the task.
///     let snapshot = mac.events();
///     let _observed = snapshot.observation();
///     mac.clear_events(snapshot, Ieee802154EventMask::NAMED);
/// }
/// oer_espressif_interrupt_table::interrupt_table! {
///     lines: LINES,
///     handler_attributes: [];
///     contexts { radio: 3; }
///     mac: MacToken = Ieee802154InterruptOwner => on_ieee802154, line 20, context radio, hart 0, Hardware;
/// }
/// fn bind(
///     router: &mut Router<InterruptController>, token: MacToken,
///     task: Ieee802154TaskOwner, setup: Ieee802154InterruptSetupOwner,
/// ) -> Result<(Ieee802154ArmedTaskOwner, Bound<MacToken>), (Ieee802154ArmedTaskOwner, BindError<MacToken>)> {
///     let (startup, interrupts) = setup.arm(task);
///     match router.bind(token, interrupts) {
///         Ok(bound) => Ok((startup, bound)),
///         Err(refused) => Err((startup, refused)),
///     }
/// }
/// fn start(startup: Ieee802154ArmedTaskOwner, bound: &Bound<MacToken>)
///     -> Ieee802154TaskOwner
/// {
///     let mut task = startup.activate(bound);
///     // ... the session ...
///     task.mask_events();
///     task
/// }
/// # fn main() {}
/// ```
///
/// The owner moves into its one binding:
///
/// ```compile_fail,E0382
/// # use oer_esp32s31_hal::ieee802154::mac::{Ieee802154InterruptOwner, Ieee802154TaskOwner};
/// # use oer_esp32s31_hal::platform::InterruptController;
/// # use oer_espressif_interrupt_table::Router;
/// # const LINES: oer_espressif_interrupt_table::Lines =
/// #     oer_espressif_interrupt_table::Lines { first: 16, count: 48, levels: 8, harts: oer_esp32s31_pac::HARTS as u8, tie_break: oer_espressif_interrupt_table::TieBreak::HighestLine };
/// # pub fn on_ieee802154(_: &mut Ieee802154InterruptOwner) {}
/// # oer_espressif_interrupt_table::interrupt_table! {
/// #     lines: LINES,
/// #     handler_attributes: [];
/// #     contexts { radio: 3; }
/// #     mac: MacToken = Ieee802154InterruptOwner => on_ieee802154, line 20, context radio, hart 0, Hardware;
/// # }
/// fn start(
///     router: &mut Router<InterruptController>,
///     token: MacToken,
///     task: &mut Ieee802154TaskOwner,
///     interrupts: Ieee802154InterruptOwner,
/// ) {
///     let _bound = router.bind(token, interrupts);
///     // The handler holds the owner now; the task cannot take it back.
///     let _setup = interrupts.deactivate(task);
/// }
/// # fn main() {}
/// ```
#[must_use = "the armed IEEE 802.15.4 interrupt owner must be bound or deactivated"]
pub struct Ieee802154InterruptOwner<P: Placement = MacPlacement> {
    mac: Ieee802154MacIrq<P>,
}

/// Owned MAC task/IRQ parts and the channel-disjoint ETM and RSSI capabilities.
/// The radio root moves these parts here from the PAC's one claim.
pub struct Ieee802154Partition {
    pub(crate) task: Ieee802154MacTask,
    pub(crate) interrupt: Ieee802154MacIrq,
    pub(crate) etm: ModemEtmIeee802154,
    pub(crate) rx_info: BtV32BasebandRxInfo,
}

impl Ieee802154Partition {
    /// Assemble the protocol's parts without accessing any register.
    pub fn new(
        mac: oer_esp32s31_pac::ownership::Ieee802154Peripherals,
        etm: ModemEtmIeee802154,
        rx_info: BtV32BasebandRxInfo,
    ) -> Self {
        Self {
            task: mac.ieee802154_mac_task,
            interrupt: mac.ieee802154_mac_irq,
            etm,
            rx_info,
        }
    }
}

impl<P: Placement> oer_mmio::InterruptSource for Ieee802154InterruptOwner<P> {
    const SOURCE: u16 = <Ieee802154MacIrq<P> as oer_mmio::InterruptSource>::SOURCE;
}

/// Split the partition into the task owner and the inactive interrupt
/// owner. This performs no MMIO.
pub(crate) fn split(
    partition: Ieee802154Partition,
) -> (Ieee802154TaskOwner, Ieee802154InterruptSetupOwner) {
    let Ieee802154Partition {
        task,
        interrupt,
        etm,
        rx_info,
    } = partition;
    (
        Ieee802154TaskOwner {
            mac: task,
            etm,
            rx_info,
        },
        Ieee802154InterruptSetupOwner { mac: interrupt },
    )
}

/// Rejoin the task owner and the inactive interrupt owner into the
/// partition. This performs no MMIO.
pub(crate) fn reunite(
    task: Ieee802154TaskOwner,
    interrupts: Ieee802154InterruptSetupOwner,
) -> Ieee802154Partition {
    let Ieee802154TaskOwner { mac, etm, rx_info } = task;
    Ieee802154Partition {
        task: mac,
        interrupt: interrupts.mac,
        etm,
        rx_info,
    }
}

impl<P: Placement, E: Placement, B: Placement> Ieee802154TaskOwner<P, E, B> {
    /// The owner of host-memory parts, for the tests of its operations.
    #[cfg(test)]
    pub(crate) const fn from_parts(
        mac: Ieee802154MacTask<P>,
        etm: ModemEtmIeee802154<E>,
        rx_info: BtV32BasebandRxInfo<B>,
    ) -> Self {
        Self { mac, etm, rx_info }
    }

    /// The MAC's task side, for the tests of its operations.
    #[cfg(test)]
    pub(crate) const fn mac_part(&self) -> &Ieee802154MacTask<P> {
        &self.mac
    }

    /// The ETM channels, for the tests of their operations.
    #[cfg(test)]
    pub(crate) const fn etm_part(&self) -> &ModemEtmIeee802154<E> {
        &self.etm
    }

    /// Order descriptor memory and MMIO at a hardware ownership boundary.
    pub(crate) fn order_device_accesses(&mut self) {
        oer_riscv::fence();
    }
}

impl<P: Placement> Ieee802154InterruptSetupOwner<P> {
    /// The owner of a host-memory interrupt side, for the tests of its
    /// operations.
    #[cfg(test)]
    pub(crate) const fn from_part(mac: Ieee802154MacIrq<P>) -> Self {
        Self { mac }
    }

    /// Mask delivery, issue STOP and both timer stops, then discard every
    /// inherited event lane without a sample, before the IRQ owner can bind.
    /// All pre-bind pending is stale: no operation can react to it. The
    /// returned task owner permits only bound activation or silent polling.
    /// This is not a producer-drain or all-lane masked-visibility proof.
    pub fn arm<E: Placement, B: Placement>(
        mut self,
        mut task: Ieee802154TaskOwner<P, E, B>,
    ) -> (
        Ieee802154ArmedTaskOwner<P, E, B>,
        Ieee802154InterruptOwner<P>,
    ) {
        execute_interrupt_initialization(
            &mut TransitionPort {
                task: &mut task,
                interrupt: &mut self.mac,
            },
            Ieee802154InterruptActivationPlan::SOURCE_CONFIRMED_BASELINE,
        );
        (
            Ieee802154ArmedTaskOwner { task },
            Ieee802154InterruptOwner { mac: self.mac },
        )
    }
}

impl<P: Placement, E: Placement, B: Placement> Ieee802154TaskOwner<P, E, B> {
    /// Publish the runtime events the handler serves: from here the MAC
    /// raises its interrupt. `bound` proves that the interrupt table's
    /// handler holds the armed interrupt owner and its line and route are
    /// written.
    ///
    /// ```compile_fail,E0061
    /// # use oer_esp32s31_hal::ieee802154::mac::Ieee802154TaskOwner;
    /// fn enable(task: &mut Ieee802154TaskOwner) {
    ///     // Enabling the interrupt takes the binding's proof.
    ///     task.enable_events();
    /// }
    /// ```
    ///
    /// ```compile_fail,E0271
    /// # use oer_esp32s31_hal::ieee802154::mac::Ieee802154TaskOwner;
    /// # use oer_espressif_interrupt_table::Bound;
    /// struct OtherIrq;
    /// impl oer_mmio::InterruptSource for OtherIrq {
    ///     const SOURCE: u16 = 3;
    /// }
    /// fn enable<E: oer_espressif_interrupt_table::Entry<Part = OtherIrq>>(task: &mut Ieee802154TaskOwner, bound: &Bound<E>) {
    ///     // Another part's binding proves nothing about this interrupt.
    ///     task.enable_events(bound);
    /// }
    /// ```
    pub fn enable_events<
        I: oer_espressif_interrupt_table::Entry<Part = Ieee802154InterruptOwner<P>>,
    >(
        &mut self,
        bound: &Bound<I>,
    ) {
        let _bound = bound;
        self.publish_runtime_events();
    }

    /// The runtime events, published and fenced.
    pub(crate) fn publish_runtime_events(&mut self) {
        self.enable_runtime_events();
        oer_riscv::fence();
    }

    /// Mask every MAC event at the end of a session, fenced: the MAC raises
    /// no interrupt until [`enable_events`](Self::enable_events) runs again.
    /// Timer pending and the tested RX_ABORT for ED_COEX_REJECT (reason 26)
    /// are retained but hidden from EVENT_STATUS reads under mask. Stop and
    /// acknowledge timer producers before masking their events. A positive
    /// abort sample saved before masking can acknowledge that event under mask.
    /// SOURCE: HW8_S31_IEEE_MASKED_STATUS_20261026;
    /// HW26_S31_IEEE_ABORT_20261027 (RX_ABORT reason 26 only).
    pub fn mask_events(&mut self) {
        self.mask_all_events();
        oer_riscv::fence();
    }
}

impl<P: Placement> Ieee802154InterruptOwner<P> {
    /// Close one polled epoch of the engine pair and return inactive setup
    /// ownership. A bound owner never comes back: its handler holds it.
    ///
    /// Stop the operation and both MAC timers, sample before masking events
    /// and aborts, then acknowledge that saved sample. Both phases are fenced
    /// before the setup owner is returned.
    /// SOURCE: hw8-20261026-radio-local-05, ieee-timers.json, PR #689.
    pub fn deactivate<E: Placement, B: Placement>(
        mut self,
        task: &mut Ieee802154TaskOwner<P, E, B>,
    ) -> Ieee802154InterruptSetupOwner<P> {
        let mut port = TransitionPort {
            task,
            interrupt: &mut self.mac,
        };
        execute_interrupt_deactivation(&mut port);
        Ieee802154InterruptSetupOwner { mac: self.mac }
    }
}

/// The task owner and the interrupt side for one activation or teardown.
struct TransitionPort<'task, P: Placement, E: Placement, B: Placement> {
    task: &'task mut Ieee802154TaskOwner<P, E, B>,
    interrupt: &'task mut Ieee802154MacIrq<P>,
}

impl<P: Placement, E: Placement, B: Placement> Ieee802154InterruptTransitionPort
    for TransitionPort<'_, P, E, B>
{
    type EventSnapshot = ieee802154::event_status::Snapshot;

    fn stop_operation(&mut self) {
        self.task.issue(ieee802154::command::Opcode::Stop);
    }

    fn stop_timer0(&mut self) {
        self.task.issue(ieee802154::command::Opcode::Timer0Stop);
    }

    fn stop_timer1(&mut self) {
        self.task.issue(ieee802154::command::Opcode::Timer1Stop);
    }

    fn enable_all_events(&mut self) {
        self.task.enable_all_events();
    }

    fn mask_all_events(&mut self) {
        self.task.mask_all_events();
    }

    fn mask_all_tx_aborts(&mut self) {
        self.task.mask_all_tx_aborts();
    }

    fn enable_runtime_tx_aborts(&mut self) {
        self.task.enable_only_runtime_tx_aborts();
    }

    fn mask_all_rx_aborts(&mut self) {
        self.task.mask_all_rx_aborts();
    }

    fn enable_runtime_rx_aborts(&mut self) {
        self.task.enable_only_runtime_rx_aborts();
    }

    fn order_device_accesses(&mut self) {
        oer_riscv::fence();
    }

    fn sample_events(&mut self) -> Self::EventSnapshot {
        self.interrupt.sample_event_status()
    }

    fn acknowledge_events(&mut self, snapshot: Self::EventSnapshot) {
        self.interrupt.acknowledge_event_status(snapshot);
    }
}

#[cfg(test)]
pub(crate) mod host;

#[cfg(test)]
mod tests;

impl<P: Placement, E: Placement, B: Placement> Ieee802154InterruptInitializationPort
    for TransitionPort<'_, P, E, B>
{
    fn discard_startup_events(&mut self) {
        self.interrupt.discard_ieee802154_startup_events();
    }
}
