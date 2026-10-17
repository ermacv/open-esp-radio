//! By-value register ownership before, during and after an interrupt epoch.
//!
//! CPU routing and pinned ISR registration belong to the execution binding.
//! It must recover the active value from that registration before deactivation:
//! neither an ISR borrow nor a consumer handle can extract these registers.

use core::marker::PhantomData;
use oer_esp32s31_hal::{
    owner::{
        ConnectedStaInterruptPrepared, MacInterruptRegisters, MacInterruptSetup,
        MacPowerInterruptRegisters, RadioRuntimeOwner,
    },
    types::{MacInterruptMask, MacPowerWakeCause},
};

use super::{IrqDisposition, IrqSnapshot, PowerIrqDisposition, PowerIrqSnapshot, handle_power_irq};

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
#[must_use = "detach the CPU route before recovering the active register owners"]
pub struct ActiveEpoch<M: RxDelivery> {
    mac: MacInterruptRegisters,
    power: MacPowerInterruptRegisters,
    delivery: PhantomData<M>,
}

impl<M: RxDelivery> ActiveEpoch<M> {
    #[inline(always)]
    pub fn service_mac(&mut self) -> (IrqDisposition, IrqSnapshot) {
        // Both sealed policies are fieldless. Dispatch is monomorphic without
        // a registry, callback or mutable moderation flag.
        super::handle_mac_irq::<M>(&mut self.mac)
    }

    #[inline(always)]
    pub fn service_power(&mut self) -> (PowerIrqDisposition, PowerIrqSnapshot) {
        handle_power_irq(&mut self.power)
    }

    pub fn prepare_connected_sta_without_power_save(
        &mut self,
        radio: &mut RadioRuntimeOwner,
    ) -> ConnectedStaInterruptPrepared {
        self.mac.prepare_connected_sta_without_power_save(radio)
    }

    pub fn mask_and_acknowledge_power_wake_cause(&mut self, cause: MacPowerWakeCause) {
        self.power.mask_and_acknowledge_wake_cause(cause);
    }

    /// Owning this value proves that no registered ISR retains a borrow.
    /// The execution binding first detaches its CPU route and registration.
    pub fn quiesce(self) -> InactiveEpoch {
        InactiveEpoch::new(self.mac.deactivate(self.power))
    }
}

impl ActiveEpoch<Moderated> {
    /// Restore delivery after the durable RX frontier has been drained.
    /// The binding excludes ISR register access for this bounded transaction.
    pub fn unmask_rx_after_drain(&mut self) {
        self.mac.unmask_rx_delivery_interrupts();
    }
}
