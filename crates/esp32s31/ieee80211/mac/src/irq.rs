//! Allocation-free MAC interrupt acknowledgement and ordered event handoff.

pub mod consumer;
pub mod epoch;

use oer_esp32s31_hal::{
    owner::{MacInterruptRegisters, MacPowerInterruptRegisters},
    types::{
        MacInterruptEvents, MacInterruptObservation, MacInterruptSnapshot,
        MacPowerInterruptObservation, MacPowerInterruptSnapshot,
    },
};

/// Finite MAC interrupt capability used by the hard ISR.
///
/// Production delegates both operations to generated PAC registers. Its
/// associated snapshot is opaque and can only be acknowledged once. Tests
/// model the same ordering with semantic observations and no MMIO identity.
pub(crate) trait MacInterruptStatusSnapshot {
    fn observation(&self) -> MacInterruptObservation;
}

impl MacInterruptStatusSnapshot for MacInterruptSnapshot {
    fn observation(&self) -> MacInterruptObservation {
        MacInterruptSnapshot::observation(self)
    }
}

impl MacInterruptStatusSnapshot for MacInterruptObservation {
    fn observation(&self) -> MacInterruptObservation {
        *self
    }
}

pub(crate) trait MacPowerInterruptStatusSnapshot {
    fn observation(&self) -> MacPowerInterruptObservation;
}

impl MacPowerInterruptStatusSnapshot for MacPowerInterruptSnapshot {
    fn observation(&self) -> MacPowerInterruptObservation {
        MacPowerInterruptSnapshot::observation(self)
    }
}

impl MacPowerInterruptStatusSnapshot for MacPowerInterruptObservation {
    fn observation(&self) -> MacPowerInterruptObservation {
        *self
    }
}

pub(crate) trait MacInterrupt {
    type Snapshot: MacInterruptStatusSnapshot;

    fn status(&mut self) -> Self::Snapshot;
    fn mask_rx_delivery(&mut self);
    fn acknowledge(&mut self, snapshot: Self::Snapshot);
}

impl MacInterrupt for MacInterruptRegisters {
    type Snapshot = MacInterruptSnapshot;

    fn status(&mut self) -> Self::Snapshot {
        self.mac_interrupt_status()
    }

    fn mask_rx_delivery(&mut self) {
        self.mask_rx_delivery_interrupts();
    }

    fn acknowledge(&mut self, snapshot: Self::Snapshot) {
        self.acknowledge_mac_interrupts(snapshot);
    }
}

/// Finite WDEVPWR interrupt capability used by the hard ISR.
///
/// The PAC snapshot exposes the reviewed TSF-timer causes as named fields and
/// groups every still-unknown cause as unhandled. Policy code never receives a
/// register image or an unqualified hardware bit number.
pub(crate) trait MacPowerInterrupt {
    type Snapshot: MacPowerInterruptStatusSnapshot;

    fn status(&mut self) -> Self::Snapshot;
    fn acknowledge(&mut self, snapshot: Self::Snapshot);
}

impl MacPowerInterrupt for MacPowerInterruptRegisters {
    type Snapshot = MacPowerInterruptSnapshot;

    fn status(&mut self) -> Self::Snapshot {
        self.power_interrupt_status()
    }

    fn acknowledge(&mut self, snapshot: Self::Snapshot) {
        self.acknowledge_power_interrupts(snapshot);
    }
}

pub const EVENT_TX_COMPLETE: u32 = 0x01;
pub const EVENT_TX_TIMEOUT: u32 = 0x02;
pub const EVENT_COLLISION: u32 = 0x04;
pub const EVENT_RX_SUCCESS: u32 = 0x08;
pub const EVENT_KNOWN_MASK: u32 = 0x0f;

/// One run-to-completion MAC action in the order used by the vendor ISR.
///
/// This is deliberately not an arbitrary bit mask. Complete
/// `libpp.a[wdev.o]::wDev_ProcessFiq` handles RX success before TX
/// completion, TX timeout and collision when one interrupt snapshot contains
/// several causes. The pinned body routes RX success to
/// `lmacProcessModemStateRxBeacon` instead while power raw-signal bits 21:22
/// are set; those belong to MAC modem-state sleep, which this driver never
/// enables. Those leaves publish separate `ppTask` queue entries, and
/// complete `libpp.a[pp.o]::ppTask` consumes one entry before
/// receiving the next. An executor port must therefore be able to retain that
/// ordering instead of merging RX and TX into one indistinguishable wakeup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrqWork {
    RxSuccess,
    TxComplete,
    TxTimeout,
    Collision,
}

impl IrqWork {
    pub const fn event_bit(self) -> u32 {
        match self {
            Self::RxSuccess => EVENT_RX_SUCCESS,
            Self::TxComplete => EVENT_TX_COMPLETE,
            Self::TxTimeout => EVENT_TX_TIMEOUT,
            Self::Collision => EVENT_COLLISION,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IrqSnapshot {
    /// Whether the generated PAC observed at least one asserted STATUS field.
    pub had_status: bool,
    /// Driver-local task event mask posted by this handler invocation.
    pub posted_events: u32,
    /// Known STATUS fields acknowledged without an independent work item.
    pub had_auxiliary_event: bool,
    /// At least one non-dispatched STATUS field was asserted.
    pub had_unhandled_event: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrqDisposition {
    Posted,
    Spurious,
    /// A nonzero image was acknowledged but contained no task-side work.
    AcknowledgedOnly,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerIrqSnapshot {
    pub observation: MacPowerInterruptObservation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerIrqDisposition {
    Posted,
    Spurious,
}

/// Select one action from a driver-local task event set in recovered vendor order.
///
/// Keeping selection pure lets validation probes exercise the same production
/// decision without depending on the executor's atomic-instruction model.
pub const fn next_irq_work(pending: u32) -> Option<IrqWork> {
    if pending & EVENT_RX_SUCCESS != 0 {
        Some(IrqWork::RxSuccess)
    } else if pending & EVENT_TX_COMPLETE != 0 {
        Some(IrqWork::TxComplete)
    } else if pending & EVENT_TX_TIMEOUT != 0 {
        Some(IrqWork::TxTimeout)
    } else if pending & EVENT_COLLISION != 0 {
        Some(IrqWork::Collision)
    } else {
        None
    }
}

pub const fn event_mask(causes: MacInterruptEvents) -> u32 {
    let mut mask = 0;
    if causes.tx_complete() {
        mask |= EVENT_TX_COMPLETE;
    }
    if causes.tx_timeout() {
        mask |= EVENT_TX_TIMEOUT;
    }
    if causes.collision() {
        mask |= EVENT_COLLISION;
    }
    if causes.rx_success() {
        mask |= EVENT_RX_SUCCESS;
    }
    mask
}

/// Acknowledge one complete MAC snapshot, returning its finite publication.
/// The caller publishes `snapshot.posted_events` after releasing the register
/// borrow, so an executor waker can never reenter a live MMIO transaction.
#[inline(always)]
pub fn handle_mac_irq<M: epoch::RxDelivery>(
    interrupt: &mut MacInterruptRegisters,
) -> (IrqDisposition, IrqSnapshot) {
    acknowledge_mac::<_, M>(interrupt)
}

#[inline(always)]
fn acknowledge_mac<I: MacInterrupt, M: epoch::RxDelivery>(
    interrupt: &mut I,
) -> (IrqDisposition, IrqSnapshot) {
    let status = interrupt.status();
    let observation = status.observation();
    let work_events = observation.work_events();
    let snapshot = IrqSnapshot {
        had_status: !observation.is_empty(),
        posted_events: event_mask(work_events),
        had_auxiliary_event: observation.has_auxiliary_event(),
        had_unhandled_event: observation.has_unhandled_event(),
    };
    if observation.is_empty() {
        return (IrqDisposition::Spurious, snapshot);
    }
    if M::MODERATES_RX && work_events.contains(MacInterruptEvents::RX_SUCCESS) {
        // Mask before W1C; a racing level remains latched until the driver
        // restores the source after draining the durable RX frontier.
        interrupt.mask_rx_delivery();
    }
    interrupt.acknowledge(status);
    let disposition = if work_events.is_empty() {
        IrqDisposition::AcknowledgedOnly
    } else {
        IrqDisposition::Posted
    };
    (disposition, snapshot)
}

/// Acknowledge one complete WDEVPWR snapshot before its causes are published.
#[inline(always)]
pub fn handle_power_irq(
    interrupt: &mut MacPowerInterruptRegisters,
) -> (PowerIrqDisposition, PowerIrqSnapshot) {
    acknowledge_power(interrupt)
}

#[inline(always)]
fn acknowledge_power<P: MacPowerInterrupt>(
    interrupt: &mut P,
) -> (PowerIrqDisposition, PowerIrqSnapshot) {
    let status = interrupt.status();
    let observation = status.observation();
    let snapshot = PowerIrqSnapshot { observation };
    if observation.is_empty() {
        return (PowerIrqDisposition::Spurious, snapshot);
    }
    interrupt.acknowledge(status);
    (PowerIrqDisposition::Posted, snapshot)
}

#[cfg(test)]
mod tests;
