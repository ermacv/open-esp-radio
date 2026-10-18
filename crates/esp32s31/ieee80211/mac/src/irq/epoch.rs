//! By-value register ownership before, during and after an interrupt epoch.
//!
//! CPU routing and pinned ISR registration belong to the execution binding.
//! It must recover the active value from that registration before deactivation:
//! neither an ISR borrow nor a consumer handle can extract these registers.

use core::marker::PhantomData;
use oer_esp32s31_hal::{
    owner::{MacInterruptRegisters, MacInterruptSetup, MacPowerInterruptRegisters},
    types::MacInterruptMask,
};

use super::{
    IrqDisposition, IrqSnapshot, MacInterrupt, MacPowerInterrupt, PowerIrqDisposition,
    PowerIrqSnapshot, handle_mac_irq, handle_power_irq,
};

mod sealed {
    pub trait Sealed {}
}

/// The policy is fixed before publication of a register owner to an ISR.
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

/// The sole setup owner. Only this state can activate or lend setup registers.
#[must_use = "the setup capability belongs to its installation"]
pub struct InactiveEpoch {
    setup: MacInterruptSetup,
}

impl InactiveEpoch {
    pub const fn new(setup: MacInterruptSetup) -> Self {
        Self { setup }
    }

    pub fn setup(&mut self) -> &mut MacInterruptSetup {
        &mut self.setup
    }

    pub fn into_setup(self) -> MacInterruptSetup {
        self.setup
    }

    pub fn activate(self, mask: MacInterruptMask) -> ActiveEpoch<Unmoderated> {
        self.activate_with_policy(mask)
    }

    pub fn activate_rx_moderated(self, mask: MacInterruptMask) -> ActiveEpoch<Moderated> {
        self.activate_with_policy(mask)
    }

    fn activate_with_policy<M: RxDelivery>(self, mask: MacInterruptMask) -> ActiveEpoch<M> {
        let (mac, power) = self.setup.activate(mask);
        ActiveEpoch {
            mac,
            power,
            delivery: PhantomData,
        }
    }
}

/// Register capabilities owned by one active installation, never global slots.
/// A moderated instance alone has the operation which restores RX delivery.
///
/// `I` and `P` are the two interrupt banks: the generated PAC capabilities in
/// production, the host models of [`super::model`] in tests. Only the
/// production banks can be quiesced back into an [`InactiveEpoch`].
#[must_use = "detach the CPU route before recovering the active register owners"]
pub struct ActiveEpoch<
    M: RxDelivery,
    I: MacInterrupt = MacInterruptRegisters,
    P: MacPowerInterrupt = MacPowerInterruptRegisters,
> {
    mac: I,
    power: P,
    delivery: PhantomData<M>,
}

impl<M: RxDelivery, I: MacInterrupt, P: MacPowerInterrupt> ActiveEpoch<M, I, P> {
    #[inline(always)]
    pub fn service_mac(&mut self) -> (IrqDisposition, IrqSnapshot) {
        // Both sealed policies are fieldless. Dispatch is monomorphic without
        // a registry, callback or mutable moderation flag.
        handle_mac_irq::<M, I>(&mut self.mac)
    }

    #[inline(always)]
    pub fn service_power(&mut self) -> (PowerIrqDisposition, PowerIrqSnapshot) {
        handle_power_irq(&mut self.power)
    }

    /// Mask every source of both banks: MAC first, then WDEVPWR.
    ///
    /// Nothing is acknowledged; a latched event stays latched, so the epoch
    /// can still be quiesced, which clears both banks. The caller excludes
    /// both handlers for the transaction: the owner of the interrupt line
    /// applies it from its own handler.
    pub fn mask_all(&mut self) {
        self.mac.mask_all();
        self.power.mask_all();
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<M: RxDelivery, I: MacInterrupt, P: MacPowerInterrupt> ActiveEpoch<M, I, P> {
    /// An active epoch over host banks, for tests without MMIO.
    pub fn for_host_test(mac: I, power: P) -> Self {
        Self {
            mac,
            power,
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

impl<M: RxDelivery> ActiveEpoch<M> {
    /// Owning this value proves that no registered ISR retains a borrow.
    /// The execution binding first detaches its CPU route and registration.
    pub fn quiesce(self) -> InactiveEpoch {
        InactiveEpoch::new(self.mac.deactivate(self.power))
    }
}

impl<I: MacInterrupt, P: MacPowerInterrupt> ActiveEpoch<Moderated, I, P> {
    /// Restore delivery after the durable RX frontier has been drained.
    /// The binding excludes ISR register access for this bounded transaction.
    ///
    /// Only the RX delivery group is enabled: after [`Self::mask_all`] every
    /// other source stays masked.
    pub fn unmask_rx_after_drain(&mut self) {
        self.mac.unmask_rx_delivery();
    }
}
