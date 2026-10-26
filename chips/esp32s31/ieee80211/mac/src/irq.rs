//! Allocation-free MAC interrupt acknowledgement and ordered event handoff.

pub mod consumer;
pub mod epoch;
/// Host models of both interrupt banks, for epochs without MMIO.
#[cfg(any(test, feature = "test-support"))]
pub mod model;

use oer_esp32s31_hal::{
    owner::{MacInterruptRegisters, MacPowerInterruptRegisters},
    types::{
        MacInterruptEnableState, MacInterruptEvents, MacInterruptMask, MacInterruptObservation,
        MacInterruptRawSnapshot, MacInterruptSnapshot, MacPowerInterruptObservation,
        MacPowerInterruptSnapshot,
    },
};

mod sealed {
    /// Only this crate names the interrupt banks an epoch may own.
    pub trait Sealed {}
}

/// One sampled MAC interrupt image, seen through its semantic observation.
///
/// Production samples are opaque and can be acknowledged only once; models
/// use the observation itself.
pub trait MacInterruptStatusSnapshot: sealed::Sealed {
    fn observation(&self) -> MacInterruptObservation;
}

impl sealed::Sealed for MacInterruptSnapshot {}
impl MacInterruptStatusSnapshot for MacInterruptSnapshot {
    fn observation(&self) -> MacInterruptObservation {
        MacInterruptSnapshot::observation(self)
    }
}

impl sealed::Sealed for MacInterruptObservation {}
impl MacInterruptStatusSnapshot for MacInterruptObservation {
    fn observation(&self) -> MacInterruptObservation {
        *self
    }
}

/// One unmasked MAC RAW sample. A nonfatal sample can be consumed by the
/// bank's acknowledgement; a fatal sample carries the complete diagnostic.
pub trait MacInterruptRawSample: sealed::Sealed {
    fn fatal_assertion_raw(&self) -> Option<u32>;
}

impl sealed::Sealed for MacInterruptRawSnapshot {}
impl MacInterruptRawSample for MacInterruptRawSnapshot {
    fn fatal_assertion_raw(&self) -> Option<u32> {
        MacInterruptRawSnapshot::fatal_assertion_raw(self)
    }
}

/// One sampled WDEVPWR image, seen through its semantic observation.
pub trait MacPowerInterruptStatusSnapshot: sealed::Sealed {
    fn observation(&self) -> MacPowerInterruptObservation;
}

impl sealed::Sealed for MacPowerInterruptSnapshot {}
impl MacPowerInterruptStatusSnapshot for MacPowerInterruptSnapshot {
    fn observation(&self) -> MacPowerInterruptObservation {
        MacPowerInterruptSnapshot::observation(self)
    }
}

impl sealed::Sealed for MacPowerInterruptObservation {}
impl MacPowerInterruptStatusSnapshot for MacPowerInterruptObservation {
    fn observation(&self) -> MacPowerInterruptObservation {
        *self
    }
}

/// The proof that both banks of an epoch were masked, which closing their
/// pending samples takes ([`epoch::MaskedBanks::clear_all`]). Only this crate
/// makes one.
#[derive(Debug)]
pub struct BanksMasked {
    _private: (),
}

impl BanksMasked {
    pub(crate) const fn new() -> Self {
        Self { _private: () }
    }
}

/// The MAC interrupt bank of the radio interrupt handler.
///
/// Production is the HAL's bank; host tests use
/// [`model::MacInterruptModel`] (feature `test-support`). The trait is
/// sealed: no other bank can enter an epoch.
pub trait MacInterrupt: sealed::Sealed {
    type Snapshot: MacInterruptStatusSnapshot;
    type RawSnapshot: MacInterruptRawSample;

    fn status(&mut self) -> Self::Snapshot;
    /// Sample RAW independently of delivery masks, without acknowledging it.
    fn raw(&mut self) -> Self::RawSnapshot;
    /// Mask the RX delivery group and leave every other source as it is.
    fn mask_rx_delivery(&mut self);
    /// Enable the RX delivery group and leave every other source as it is.
    fn unmask_rx_delivery(&mut self);
    /// Mask every source; a latched event stays latched.
    fn mask_all(&mut self);
    fn acknowledge(&mut self, snapshot: Self::Snapshot);
    /// Acknowledge only the captured RAW events of the masked bank.
    fn acknowledge_raw(&mut self, snapshot: Self::RawSnapshot, masked: &BanksMasked);
    /// Publish one reviewed enable image.
    fn publish(&mut self, mask: MacInterruptMask);
    /// The published enable image, classified.
    fn enable_state(&self) -> MacInterruptEnableState;
}

impl sealed::Sealed for MacInterruptRegisters {}
impl MacInterrupt for MacInterruptRegisters {
    type Snapshot = MacInterruptSnapshot;
    type RawSnapshot = MacInterruptRawSnapshot;

    fn status(&mut self) -> Self::Snapshot {
        self.mac_interrupt_status()
    }

    fn raw(&mut self) -> Self::RawSnapshot {
        self.mac_interrupt_raw()
    }

    fn mask_rx_delivery(&mut self) {
        self.mask_rx_delivery_interrupts();
    }

    fn unmask_rx_delivery(&mut self) {
        self.unmask_rx_delivery_interrupts();
    }

    fn mask_all(&mut self) {
        self.mask_all_mac_interrupts();
    }

    fn acknowledge(&mut self, snapshot: Self::Snapshot) {
        self.acknowledge_mac_interrupts(snapshot);
    }

    fn acknowledge_raw(&mut self, snapshot: Self::RawSnapshot, _masked: &BanksMasked) {
        self.acknowledge_mac_interrupt_raw(snapshot);
    }

    fn publish(&mut self, mask: MacInterruptMask) {
        self.publish_mac_interrupt_mask(mask);
    }

    fn enable_state(&self) -> MacInterruptEnableState {
        self.mac_interrupt_enable()
    }
}

/// The WDEVPWR interrupt bank of the radio interrupt handler.
///
/// The PAC snapshot exposes the reviewed TSF-timer causes as named fields and
/// groups every still-unknown cause as unhandled. Policy code never receives a
/// register image or an unqualified hardware bit number. Sealed like
/// [`MacInterrupt`]; host tests use [`model::PowerInterruptModel`].
pub trait MacPowerInterrupt: sealed::Sealed {
    type Snapshot: MacPowerInterruptStatusSnapshot;

    fn status(&mut self) -> Self::Snapshot;
    /// Mask delivery without acknowledging RAW pending. STATUS hides masked
    /// causes; sampling STATUS after this is not proof that RAW is clear.
    fn mask_all(&mut self);
    fn acknowledge(&mut self, snapshot: Self::Snapshot);
    /// Acknowledge every latched cause of the masked bank.
    fn clear_all(&mut self, masked: &BanksMasked);
    /// Enable the station TBTT cause.
    fn enable_station_tbtt(&mut self);
}

impl sealed::Sealed for MacPowerInterruptRegisters {}
impl MacPowerInterrupt for MacPowerInterruptRegisters {
    type Snapshot = MacPowerInterruptSnapshot;

    fn status(&mut self) -> Self::Snapshot {
        self.power_interrupt_status()
    }

    fn mask_all(&mut self) {
        self.mask_all_power_interrupts();
    }

    fn acknowledge(&mut self, snapshot: Self::Snapshot) {
        self.acknowledge_power_interrupts(snapshot);
    }

    fn clear_all(&mut self, _masked: &BanksMasked) {
        self.clear_all_power_interrupts();
    }

    fn enable_station_tbtt(&mut self) {
        self.enable_station_tbtt_interrupt();
    }
}

pub const EVENT_TX_COMPLETE: u32 = 0x01;
pub const EVENT_TX_TIMEOUT: u32 = 0x02;
pub const EVENT_BSS_COLOR_COLLISION: u32 = 0x04;
pub const EVENT_RX_SUCCESS: u32 = 0x08;
pub const EVENT_KNOWN_MASK: u32 = 0x0f;

/// One MAC action; the independent HE color event follows RX and TX work.
///
/// This is deliberately not an arbitrary bit mask. Complete
/// `libpp.a[wdev.o]::wDev_ProcessFiq` handles RX success before TX
/// completion and TX timeout when one interrupt snapshot contains
/// several causes. The pinned body routes RX success to
/// `lmacProcessModemStateRxBeacon` instead while power raw-signal bits 17:18
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
    BssColorCollision,
}

impl IrqWork {
    pub const fn event_bit(self) -> u32 {
        match self {
            Self::RxSuccess => EVENT_RX_SUCCESS,
            Self::TxComplete => EVENT_TX_COMPLETE,
            Self::TxTimeout => EVENT_TX_TIMEOUT,
            Self::BssColorCollision => EVENT_BSS_COLOR_COLLISION,
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
/// Keeping selection pure lets host tests exercise the production decision
/// without depending on the executor's atomic-instruction model.
pub const fn next_irq_work(pending: u32) -> Option<IrqWork> {
    if pending & EVENT_RX_SUCCESS != 0 {
        Some(IrqWork::RxSuccess)
    } else if pending & EVENT_TX_COMPLETE != 0 {
        Some(IrqWork::TxComplete)
    } else if pending & EVENT_TX_TIMEOUT != 0 {
        Some(IrqWork::TxTimeout)
    } else if pending & EVENT_BSS_COLOR_COLLISION != 0 {
        Some(IrqWork::BssColorCollision)
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
    if causes.bss_color_collision() {
        mask |= EVENT_BSS_COLOR_COLLISION;
    }
    if causes.rx_success() {
        mask |= EVENT_RX_SUCCESS;
    }
    mask
}

/// A terminal MAC interrupt failure, carrying the original unmasked RAW sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fault {
    /// SOURCE: BLOB_LIBPP_WIFI_IRQ_FACTS_20261026: MAC21 posts the vendor's
    /// fatal assertion diagnostic. Its physical trigger remains unknown.
    FatalAssertion { raw: u32 },
}

/// Acknowledge one complete MAC snapshot, returning its finite publication.
/// The caller publishes `snapshot.posted_events` after releasing the register
/// borrow, so an executor waker can never reenter a live MMIO transaction.
#[inline(always)]
pub fn handle_mac_irq<M: epoch::RxDelivery, I: MacInterrupt>(
    interrupt: &mut I,
) -> Result<(IrqDisposition, IrqSnapshot), Fault> {
    let status = interrupt.status();
    let observation = status.observation();
    if let Some(raw) = interrupt.raw().fatal_assertion_raw() {
        return Err(Fault::FatalAssertion { raw });
    }
    let work_events = observation.work_events();
    let snapshot = IrqSnapshot {
        had_status: !observation.is_empty(),
        posted_events: event_mask(work_events),
        had_auxiliary_event: observation.has_auxiliary_event(),
        had_unhandled_event: observation.has_unhandled_event(),
    };
    if observation.is_empty() {
        return Ok((IrqDisposition::Spurious, snapshot));
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
    Ok((disposition, snapshot))
}

/// Acknowledge one complete WDEVPWR snapshot before its causes are published.
#[inline(always)]
pub fn handle_power_irq<P: MacPowerInterrupt>(
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
