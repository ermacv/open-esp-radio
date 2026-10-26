//! The MAC and WDEVPWR interrupt banks of the radio interrupt handler and
//! the epochs it runs them through.
//!
//! The image's interrupt table binds both banks to the radio interrupt
//! handler once; [`InterruptEpoch`] is what that handler holds. No task
//! reaches a bank: a task asks the handler for each transition (through the
//! driver's doorbell), and the CPU route stays as the binding wrote it.
//!
//! - **Closed**: both banks masked and every latched event cleared. An
//!   epoch starts here, whatever the boot left in the banks; an interrupt
//!   that reaches a closed epoch closes it again
//!   ([`InterruptEpoch::close_stray`]).
//! - **Active** ([`InterruptEpoch::activate`]): both banks masked, every
//!   stale event cleared, then the runtime mask and the station TBTT
//!   published.
//! - **Masked** ([`InterruptEpoch::mask_all`]): every source masked, nothing
//!   acknowledged: a latched event stays latched until the epoch closes or
//!   activates again.
//!
//! Clearing every latched event takes the masking proof:
//! [`InterruptEpoch::mask_all`] returns [`MaskedBanks`], whose
//! [`MaskedBanks::clear_all`] closes the epoch.

use core::marker::PhantomData;
use oer_esp32s31_hal::{
    owner::{MacInterruptRegisters, MacPowerInterruptRegisters},
    types::{MacInterruptEnableState, MacInterruptMask},
};

use super::{
    BanksMasked, Fault, IrqDisposition, IrqSnapshot, MacInterrupt, MacInterruptStatusSnapshot,
    MacPowerInterrupt, MacPowerInterruptStatusSnapshot, PowerIrqDisposition, PowerIrqSnapshot,
    handle_mac_irq, handle_power_irq,
};

mod sealed {
    pub trait Sealed {}
}

/// The RX delivery policy, fixed when the handler's epoch is made.
pub trait RxDelivery: sealed::Sealed {
    const MODERATES_RX: bool;
}

pub struct Unmoderated;
pub struct Moderated;
impl sealed::Sealed for Unmoderated {}
impl sealed::Sealed for Moderated {}
impl RxDelivery for Unmoderated {
    const MODERATES_RX: bool = false;
}
impl RxDelivery for Moderated {
    const MODERATES_RX: bool = true;
}

/// Where an epoch's banks are.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EpochState {
    /// Both banks masked and cleared.
    Closed,
    /// The runtime mask published.
    Active,
    /// Every source masked; latched events kept.
    Masked,
    /// A fatal sample retained; both banks stay masked until reset.
    Quarantined(Fault),
}

/// Both interrupt banks of the radio interrupt handler, in one epoch state.
///
/// `I` and `P` are the two banks: the HAL's in production, the host models
/// of [`super::model`] in tests. `M` is the RX delivery policy: a moderated
/// epoch masks the RX delivery group before it acknowledges an RX event and
/// alone can restore it ([`InterruptEpoch::unmask_rx_after_drain`]).
pub struct InterruptEpoch<
    M: RxDelivery,
    I: MacInterrupt = MacInterruptRegisters,
    P: MacPowerInterrupt = MacPowerInterruptRegisters,
> {
    mac: I,
    power: P,
    state: EpochState,
    delivery: PhantomData<M>,
}

impl<M: RxDelivery, I: MacInterrupt, P: MacPowerInterrupt> InterruptEpoch<M, I, P> {
    /// The handler's banks, closed. No register is touched: the first
    /// transition masks and clears them, and an interrupt that reaches the
    /// closed epoch closes them.
    pub const fn new(mac: I, power: P) -> Self {
        Self {
            mac,
            power,
            state: EpochState::Closed,
            delivery: PhantomData,
        }
    }

    pub const fn state(&self) -> EpochState {
        self.state
    }

    /// Mask both banks, clear every stale event, then publish `mask` and
    /// the station TBTT: MAC first, then WDEVPWR, at each step.
    ///
    /// A previous epoch may leave a level event latched. Both banks stay
    /// masked until every stale event is acknowledged: publishing the
    /// runtime mask before the clear would raise the line on a stale event.
    pub fn activate(&mut self, mask: MacInterruptMask) -> Result<(), Fault> {
        if let EpochState::Quarantined(fault) = self.state {
            return Err(fault);
        }
        self.mask_all().clear_all();
        self.mac.publish(mask);
        self.power.enable_station_tbtt();
        self.state = EpochState::Active;
        Ok(())
    }

    /// Capture a visible fatal cause before a handler request can clear it.
    pub fn check_fault(&mut self) -> Result<(), Fault> {
        if let EpochState::Quarantined(fault) = self.state {
            return Err(fault);
        }
        if let Some(status) = self.mac.status().observation().hardware_assertion_status() {
            return Err(self.quarantine(Fault::HardwareAssertion { status }));
        }
        Ok(())
    }

    /// Mask every source of both banks, MAC first, then WDEVPWR. Nothing is
    /// acknowledged; a latched event stays latched. The returned proof
    /// clears both banks; dropped, the epoch stays masked (a closed epoch
    /// stays closed).
    pub fn mask_all(&mut self) -> MaskedBanks<'_, M, I, P> {
        self.mac.mask_all();
        self.power.mask_all();
        if self.state == EpochState::Active {
            self.state = EpochState::Masked;
        }
        MaskedBanks { epoch: self }
    }

    /// Mask and clear both banks: the epoch closes.
    pub fn close(&mut self) {
        self.mask_all().clear_all();
    }

    /// Whether either bank shows an enabled event in the closed epoch, which
    /// it then closes again: an interrupt left from before the epoch (an
    /// enable a reset restored, or the boot left). Samples both banks and
    /// acknowledges nothing, unless it closes them. An open epoch returns
    /// `false` and touches nothing.
    pub fn close_stray(&mut self) -> Result<bool, Fault> {
        if let EpochState::Quarantined(fault) = self.state {
            return Err(fault);
        }
        if self.state != EpochState::Closed {
            return Ok(false);
        }
        let observation = self.mac.status().observation();
        if let Some(status) = observation.hardware_assertion_status() {
            return Err(self.quarantine(Fault::HardwareAssertion { status }));
        }
        let mac = !observation.is_empty();
        let power = !self.power.status().observation().is_empty();
        if mac || power {
            self.close();
        }
        Ok(mac || power)
    }

    /// Sample, moderate and acknowledge the MAC bank; `None` in a closed
    /// epoch, which samples nothing ([`Self::close_stray`] serves it).
    #[inline(always)]
    pub fn service_mac(&mut self) -> Result<Option<(IrqDisposition, IrqSnapshot)>, Fault> {
        if let EpochState::Quarantined(fault) = self.state {
            return Err(fault);
        }
        if self.state == EpochState::Closed {
            return Ok(None);
        }
        // Both sealed policies are fieldless. Dispatch is monomorphic without
        // a registry, callback or mutable moderation flag.
        match handle_mac_irq::<M, I>(&mut self.mac) {
            Ok(sample) => Ok(Some(sample)),
            Err(fault) => Err(self.quarantine(fault)),
        }
    }

    fn quarantine(&mut self, fault: Fault) -> Fault {
        drop(self.mask_all());
        self.state = EpochState::Quarantined(fault);
        fault
    }

    /// Sample and acknowledge the WDEVPWR bank; `None` in a closed or quarantined epoch.
    #[inline(always)]
    pub fn service_power(&mut self) -> Option<(PowerIrqDisposition, PowerIrqSnapshot)> {
        if matches!(self.state, EpochState::Closed | EpochState::Quarantined(_)) {
            return None;
        }
        Some(handle_power_irq(&mut self.power))
    }

    /// The MAC bank's published enable image, classified.
    pub fn mac_interrupt_enable(&self) -> MacInterruptEnableState {
        self.mac.enable_state()
    }
}

impl<I: MacInterrupt, P: MacPowerInterrupt> InterruptEpoch<Moderated, I, P> {
    /// Restore RX delivery after the durable RX frontier has been drained:
    /// only the RX delivery group, and only in an active epoch. Returns
    /// whether it was restored; a masked or closed epoch keeps every source
    /// masked.
    pub fn unmask_rx_after_drain(&mut self) -> bool {
        if self.state != EpochState::Active {
            return false;
        }
        self.mac.unmask_rx_delivery();
        true
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<M: RxDelivery, I: MacInterrupt, P: MacPowerInterrupt> InterruptEpoch<M, I, P> {
    /// An active epoch over host banks, for tests without MMIO.
    pub fn for_host_test(mac: I, power: P) -> Self {
        Self {
            mac,
            power,
            state: EpochState::Active,
            delivery: PhantomData,
        }
    }

    /// Borrow both banks to inspect their state.
    pub fn banks(&self) -> (&I, &P) {
        (&self.mac, &self.power)
    }

    /// Borrow both banks to latch an event, as the hardware would.
    pub fn banks_mut(&mut self) -> (&mut I, &mut P) {
        (&mut self.mac, &mut self.power)
    }
}

/// Both banks of an epoch, masked: the proof that clearing every latched
/// event takes. It borrows the epoch, so no other transition runs between
/// the mask and the clear.
///
/// Clearing without the mask does not compile:
///
/// ```compile_fail,E0599
/// use oer_esp32s31_ieee80211_mac::irq::epoch::{InterruptEpoch, Moderated};
/// fn clear(epoch: &mut InterruptEpoch<Moderated>) {
///     epoch.clear_all();
/// }
/// ```
#[must_use = "a masked epoch is cleared with `clear_all`, or stays masked"]
pub struct MaskedBanks<'a, M: RxDelivery, I: MacInterrupt, P: MacPowerInterrupt> {
    epoch: &'a mut InterruptEpoch<M, I, P>,
}

impl<M: RxDelivery, I: MacInterrupt, P: MacPowerInterrupt> MaskedBanks<'_, M, I, P> {
    /// Acknowledge every latched event of both masked banks, MAC first,
    /// then WDEVPWR: the epoch closes.
    pub fn clear_all(self) {
        let masked = BanksMasked::new();
        self.epoch.mac.clear_all(&masked);
        self.epoch.power.clear_all(&masked);
        if !matches!(self.epoch.state, EpochState::Quarantined(_)) {
            self.epoch.state = EpochState::Closed;
        }
    }
}
