//! Driver-facing owners of the dedicated IEEE 802.15.4 MAC registers.
//!
//! The task owner and the active interrupt owner together drive the MAC
//! through the [`ll`](crate::i::ll) backend. The interrupt owners
//! expose the reviewed activation and teardown transitions. The owners are
//! made from and returned to the PAC partition; the MAC lifecycle that will
//! own clocks, reset and foundation around them is not part of this crate
//! yet.

use oer_esp32c5_pac::{
    Ieee802154InterruptRegisters as PacInterruptRegisters,
    Ieee802154InterruptSetup as PacInterruptSetup, Ieee802154RegisterLease as PacRegisterLease,
    Ieee802154TaskRegisters as PacTaskRegisters,
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
    /// Split the IEEE 802.15.4 partition into the task owner and its
    /// inactive interrupt owner. This performs no MMIO.
    pub fn new(
        partition: oer_esp32c5_pac::Ieee802154Partition,
    ) -> (Self, Ieee802154InterruptSetupOwner) {
        let (registers, interrupts) = PacTaskRegisters::new(partition);
        (
            Self { registers },
            Ieee802154InterruptSetupOwner {
                registers: interrupts,
            },
        )
    }

    /// Reunite the task owner with its inactive interrupt owner and return
    /// the partition. This performs no MMIO.
    pub fn into_partition(
        self,
        interrupts: Ieee802154InterruptSetupOwner,
    ) -> oer_esp32c5_pac::Ieee802154Partition {
        self.registers.into_partition(interrupts.registers)
    }

    /// `ieee802154_txon_delay_set` of the ESP32-C5 libbtbb: the vendor
    /// transmit-on, turnaround, receive-on and transmit-off delays, in the
    /// vendor order. `ieee802154_mac_init` applies them after the register
    /// part of the MAC initialization, before interrupts are allocated, so
    /// the interrupt owner must still be inactive.
    pub fn apply_txon_delay(&mut self, interrupts: &mut Ieee802154InterruptSetupOwner) {
        interrupts
            .registers
            .polled_register_lease(&mut self.registers)
            .apply_txon_delay();
    }

    /// Borrow the PAC task lease for one low-level accessor.
    pub(crate) fn lease(&mut self) -> PacRegisterLease<'_> {
        self.registers.ieee802154_register_lease()
    }
}

/// Inactive IEEE 802.15.4 interrupt ownership.
#[must_use = "the inactive IEEE 802.15.4 interrupt owner must be returned to its foundation owner"]
pub struct Ieee802154InterruptSetupOwner {
    registers: PacInterruptSetup,
}

impl Ieee802154InterruptSetupOwner {
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
