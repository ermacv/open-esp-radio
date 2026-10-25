//! What the checks read from the chip profile: the `[memory]` map and the
//! `[interrupts]` contract.

use oer_chip_profile::Profile;

use oer_espressif_facts::{InterruptContract, Memory};

/// The chip's address map and interrupt contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Contract {
    pub memory: Memory,
    pub interrupts: InterruptContract,
}

impl Contract {
    /// The contract `profile` names; `None` when it names no `[memory]`
    /// map or no `[interrupts]` contract.
    pub fn of(profile: &Profile) -> Option<Self> {
        Some(Self {
            memory: profile.facts.memory.clone()?,
            interrupts: profile.facts.interrupts.clone()?,
        })
    }

    /// The bytes an interrupt stack may use: below them lies the guard.
    pub fn irq_stack_usable_bytes(&self, hart: u32) -> u32 {
        self.interrupts.irq_stack_usable_bytes(hart)
    }
}
