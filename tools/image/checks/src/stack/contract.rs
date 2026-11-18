//! What the checks read from the chip profile and its stack policy: the
//! `[memory]` map, the `[interrupts]` contract and the interrupt stacks'
//! margin.

use oer_tools_chip_profile::Profile;

use oer_chips_family_facts::{InterruptContract, Memory};

/// The chip's address map and interrupt contract, and the stack policy's
/// margin above an interrupt stack's proven bound.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Contract {
    pub memory: Memory,
    pub interrupts: InterruptContract,
    /// `irq_stack_margin_percent` of the chip's stack policy.
    pub irq_margin_percent: u32,
}

impl Contract {
    /// The contract `profile` names, with its stack policy's interrupt-stack
    /// margin; `None` when it names no `[memory]` map or no `[interrupts]`
    /// contract.
    pub fn of(profile: &Profile, irq_margin_percent: u32) -> Option<Self> {
        Some(Self {
            memory: profile.facts.memory.clone()?,
            interrupts: profile.facts.interrupts.clone()?,
            irq_margin_percent,
        })
    }

    /// What an interrupt stack whose bound is `bound` must hold above its
    /// guard: the bound plus the policy's margin.
    pub fn irq_stack_with_margin(&self, bound: u64) -> oer_chips_family_facts::Result<u64> {
        oer_chips_family_facts::irq_stack_with_margin(bound, self.irq_margin_percent)
    }
}

/// The interrupt-stack margin of the tracked chip `id`'s stack policy, for
/// tests that build a [`Contract`] from a tracked profile.
#[cfg(test)]
pub(crate) fn tracked_margin(id: &str) -> u32 {
    tracked_policy(id)
        .irq_stack_margin_percent()
        .expect("a margin")
}

#[cfg(test)]
fn tracked_policy(id: &str) -> oer_tools_chip_profile::policy::StackPolicy {
    let text = oer_chips_family_facts::tracked::text(&format!("chips/{id}/facts/stack.toml"))
        .expect("a tracked stack policy");
    toml::from_str(text).expect("a stack policy")
}

/// The tracked chip `id`'s profile with its stack policy's interrupt guards
/// written in, as `StackPolicy::runtime_profile` writes them for an image
/// that places them.
#[cfg(test)]
pub(crate) fn tracked_profile(id: &str) -> oer_tools_chip_profile::Profile {
    let mut profile = oer_tools_chip_profile::Profile::tracked(id).expect("a tracked profile");
    if let Some(interrupts) = profile.facts.interrupts.as_mut() {
        interrupts.irq_stack_guard_bytes = tracked_policy(id).irq_stack_guard_bytes;
    }
    profile
}
