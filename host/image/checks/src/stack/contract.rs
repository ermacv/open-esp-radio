//! What the checks read from the chip profile: the `[memory]` map and the
//! `[interrupts]` contract.

use oer_chip_profile::{InterruptContract, Memory, Profile};

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
            memory: profile.memory.clone()?,
            interrupts: profile.interrupts.clone()?,
        })
    }

    /// The bytes an interrupt stack may use: below them lies the guard.
    pub fn irq_stack_usable_bytes(&self) -> u32 {
        self.interrupts.irq_stack_usable_bytes()
    }
}
