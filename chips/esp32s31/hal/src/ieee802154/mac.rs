//! Driver-facing owners of the dedicated IEEE 802.15.4 MAC registers.
//!
//! The PAC hands the IEEE 802.15.4 partition over as its generated parts
//! ([`Ieee802154Partition`]): the MAC block, the modem ETM's channels zero
//! and one and the shared baseband's receive-information read. This module
//! splits the MAC into its generated task and interrupt sides and gives each
//! owner the register operations of the pinned public LL
//! (`ieee802154_common_ll.h`), written over the parts' field accessors:
//!
//! - `EVENT_STATUS` is written only by acknowledging a sample of it, so an
//!   event raised after the sample stays latched;
//! - the ETM channel-enable set and clear words are written only by their
//!   channels' trigger strobes;
//! - the frame-buffer address registers take only a `DmaWindowAddress`;
//! - the interrupt side holds `EVENT_STATUS` and reads the RX/TX status and
//!   ED result words; the task side holds every other register.
//!
//! The task owner and the active interrupt owner together drive the MAC
//! through the [`ll`](crate::ieee802154::ll) backend. The interrupt owners
//! expose the activation and teardown transitions. None of these types can
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
    Ieee802154InterruptActivationPlan, Ieee802154InterruptTransitionPort,
    execute_interrupt_activation, execute_interrupt_deactivation,
};
use oer_mmio::{Fixed, Placement};

mod etm;
mod interrupt;
mod task;

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

/// Inactive IEEE 802.15.4 interrupt ownership: the MAC's interrupt side
/// while no handler holds it.
#[must_use = "the inactive IEEE 802.15.4 interrupt owner must be returned to its foundation owner"]
pub struct Ieee802154InterruptSetupOwner<P: Placement = MacPlacement> {
    mac: Ieee802154MacIrq<P>,
}

/// Active IEEE 802.15.4 hard-IRQ owner.
///
/// Task command, DMA, policy and event-enable operations are absent from
/// this type. It must be deactivated after the platform CPU route is
/// disabled and returned with the task owner before the partition can be.
#[must_use = "the active IEEE 802.15.4 interrupt owner must be deactivated"]
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
        oer_mmio::fence();
    }
}

impl<P: Placement> Ieee802154InterruptSetupOwner<P> {
    /// The owner of a host-memory interrupt side, for the tests of its
    /// operations.
    #[cfg(test)]
    pub(crate) const fn from_part(mac: Ieee802154MacIrq<P>) -> Self {
        Self { mac }
    }

    /// Install the runtime event and abort baseline, acknowledge one stale
    /// sample and return the active interrupt owner. The platform CPU route
    /// must be enabled only afterwards.
    ///
    /// `EVENT_ENABLE` stays zero while both abort fields take their runtime
    /// baseline and the stale sample is acknowledged; the runtime events are
    /// published last and fenced before the owner is returned.
    pub fn activate<E: Placement, B: Placement>(
        self,
        task: &mut Ieee802154TaskOwner<P, E, B>,
    ) -> Ieee802154InterruptOwner<P> {
        let mut port = TransitionPort {
            task,
            interrupt: self.mac,
        };
        execute_interrupt_activation(
            &mut port,
            Ieee802154InterruptActivationPlan::SOURCE_CONFIRMED_BASELINE,
        );
        Ieee802154InterruptOwner {
            mac: port.interrupt,
        }
    }
}

impl<P: Placement> Ieee802154InterruptOwner<P> {
    /// Close one hard-IRQ epoch after the CPU route was disabled and return
    /// inactive setup ownership.
    ///
    /// The operation and both MAC timers stop, the event, transmit-abort and
    /// receive-abort enables are cleared, one final sample is acknowledged,
    /// and both phases are fenced before the setup owner is returned.
    pub fn deactivate<E: Placement, B: Placement>(
        self,
        task: &mut Ieee802154TaskOwner<P, E, B>,
    ) -> Ieee802154InterruptSetupOwner<P> {
        let mut port = TransitionPort {
            task,
            interrupt: self.mac,
        };
        execute_interrupt_deactivation(&mut port);
        Ieee802154InterruptSetupOwner {
            mac: port.interrupt,
        }
    }
}

/// The task owner and the interrupt side for one activation or teardown.
struct TransitionPort<'task, P: Placement, E: Placement, B: Placement> {
    task: &'task mut Ieee802154TaskOwner<P, E, B>,
    interrupt: Ieee802154MacIrq<P>,
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

    fn mask_all_events(&mut self) {
        self.task.mask_all_events();
    }

    fn enable_runtime_events(&mut self) {
        self.task.enable_runtime_events();
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
        oer_mmio::fence();
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
