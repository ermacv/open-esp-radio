//! Driver-facing owners of the dedicated IEEE 802.15.4 MAC registers.
//!
//! The task owner and the active interrupt owner together drive the MAC
//! through the [`ll`](crate::ieee802154::ll) backend. The interrupt owners
//! expose the reviewed activation and teardown transitions. None of these types can be
//! constructed outside the HAL: `Ieee802154FoundationConfigured::into_operational`
//! hands them out and `Ieee802154FoundationConfigured::from_operational`
//! takes both back, so they never exist apart from their partition. Commands and policy use
//! the HAL's semantic vocabulary; only the interrupt event vocabulary, which
//! has no HAL counterpart, is re-exported here.

use oer_esp32s31_pac::{
    Ieee802154InterruptRegisters as PacInterruptRegisters,
    Ieee802154InterruptSetup as PacInterruptSetup, Ieee802154RegisterLease as PacRegisterLease,
    Ieee802154TaskRegisters as PacTaskRegisters,
};

pub use oer_esp32s31_pac::{
    Ieee802154Event, Ieee802154EventMask, Ieee802154EventObservation,
    Ieee802154EventObservationError, Ieee802154RxAbortReason, Ieee802154RxAbortReasonObservation,
    Ieee802154TxAbortReason, Ieee802154TxAbortReasonObservation,
};

use crate::ieee802154::{
    lifecycle::Ieee802154Channel,
    policy::{
        Ieee802154AckTimeout, Ieee802154CcaMode, Ieee802154MacControl, Ieee802154MacPolicyWrites,
        Ieee802154PanIdentity,
    },
};

/// Exclusive task-side owner of the IEEE 802.15.4 MAC registers.
///
/// Every operation is expressed in the HAL's semantic vocabulary; drivers
/// never name a register-level value type.
#[must_use = "the IEEE 802.15.4 task owner must be returned to its foundation owner"]
pub struct Ieee802154TaskOwner {
    registers: PacTaskRegisters,
}

impl Ieee802154TaskOwner {
    pub(crate) const fn new(registers: PacTaskRegisters) -> Self {
        Self { registers }
    }

    pub(crate) fn into_registers(self) -> PacTaskRegisters {
        self.registers
    }

    /// Borrow the PAC task lease for one low-level accessor.
    pub(crate) fn lease(&mut self) -> PacRegisterLease<'_> {
        self.registers.ieee802154_register_lease()
    }
}

impl Ieee802154MacPolicyWrites for Ieee802154TaskOwner {
    fn set_channel(&mut self, channel: Ieee802154Channel) {
        self.registers
            .ieee802154_register_lease()
            .set_frequency_code(oer_esp32s31_pac::Ieee802154FrequencyCode::new(
                channel.frequency_code(),
            ));
    }

    fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
        self.registers
            .ieee802154_register_lease()
            .set_cca_mode(crate::ieee802154::policy::cca_mode_into_pac(mode));
    }

    fn set_cca_threshold_code(&mut self, threshold: i8) {
        self.registers
            .ieee802154_register_lease()
            .set_cca_threshold_code(threshold);
    }

    fn set_mac_control(&mut self, control: Ieee802154MacControl) {
        self.registers
            .ieee802154_register_lease()
            .set_mac_control(control.into_pac());
    }

    fn set_ack_timeout(&mut self, timeout: Ieee802154AckTimeout) {
        self.registers
            .ieee802154_register_lease()
            .set_ack_timeout(timeout.into_pac());
    }

    fn set_primary_pan_identity(&mut self, identity: Ieee802154PanIdentity) {
        self.registers
            .ieee802154_register_lease()
            .set_primary_pan_identity(identity.into_pac());
    }

    fn order_device_accesses(&mut self) {
        self.registers.order_device_accesses();
    }
}

/// Inactive IEEE 802.15.4 interrupt ownership.
#[must_use = "the inactive IEEE 802.15.4 interrupt owner must be returned to its foundation owner"]
pub struct Ieee802154InterruptSetupOwner {
    registers: PacInterruptSetup,
}

impl Ieee802154InterruptSetupOwner {
    pub(crate) const fn new(registers: PacInterruptSetup) -> Self {
        Self { registers }
    }

    pub(crate) fn into_pac(self) -> PacInterruptSetup {
        self.registers
    }

    /// Install the reviewed event/abort baseline, acknowledge one stale
    /// snapshot and return the active interrupt owner. The platform CPU route
    /// must be enabled only afterwards.
    pub fn activate(self, task: &mut Ieee802154TaskOwner) -> Ieee802154InterruptOwner {
        Ieee802154InterruptOwner {
            registers: self.registers.activate(&mut task.registers),
        }
    }
}

/// Active IEEE 802.15.4 hard-IRQ owner.
#[must_use = "the active IEEE 802.15.4 interrupt owner must be deactivated"]
pub struct Ieee802154InterruptOwner {
    registers: PacInterruptRegisters,
}

impl Ieee802154InterruptOwner {
    pub(crate) const fn registers(&self) -> &PacInterruptRegisters {
        &self.registers
    }

    pub(crate) fn registers_mut(&mut self) -> &mut PacInterruptRegisters {
        &mut self.registers
    }

    /// Close one hard-IRQ epoch after the CPU route was disabled and return
    /// inactive setup ownership.
    pub fn deactivate(self, task: &mut Ieee802154TaskOwner) -> Ieee802154InterruptSetupOwner {
        Ieee802154InterruptSetupOwner {
            registers: self.registers.deactivate(&mut task.registers),
        }
    }
}
