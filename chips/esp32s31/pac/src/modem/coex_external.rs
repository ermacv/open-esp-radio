//! Exact generated-PAC transactions of external coexistence.
//!
//! Each method is one complete `hal_external_coexist.o` leaf of the pinned
//! libcoexist, in its order of fresh-read RMW edges. The routing of the
//! external signals through the GPIO matrix belongs to the platform.

#![forbid(unsafe_code)]

use crate::{
    SharedRadioRegisters,
    generated::{
        ExternalCoexFlag, ExternalCoexGrantDelay, ExternalCoexNibble, ExternalCoexSync,
        ExternalCoexWorkMode,
    },
};

/// Signal wires between the chip and the external radio
/// (`external_coex_wire_t`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ExternalCoexWires {
    One = 0,
    Two = 1,
    Three = 2,
    Four = 3,
}

/// The chip's role toward the external radio (`esp_extern_coex_work_mode_t`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ExternalCoexRole {
    /// The chip arbitrates: the external radio requests and the chip grants.
    Leader = 0,
    /// The external radio arbitrates: the chip requests and it grants.
    Follower = 2,
}

/// One four-bit external priority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExternalCoexPriority(ExternalCoexNibble);

impl ExternalCoexPriority {
    /// The largest external priority, from the generated value domain.
    pub const MAX: u8 = ExternalCoexNibble::MAX as u8;

    /// A priority of the four-bit domain, or `None` above it.
    pub const fn new(value: u8) -> Option<Self> {
        match ExternalCoexNibble::new(value as u32) {
            Some(nibble) => Some(Self(nibble)),
            None => None,
        }
    }
}

fn nibble(value: u32) -> ExternalCoexNibble {
    ExternalCoexNibble::new(value).expect("an external coexistence nibble fits four bits")
}

fn flag(value: bool) -> ExternalCoexFlag {
    ExternalCoexFlag::new(u32::from(value)).expect("a flag fits one bit")
}

impl SharedRadioRegisters {
    /// `hal_set_extern_pti_mode(role)` with the wire count of
    /// `esp_coex_external_get_wire_type`: the synchronization is cleared and
    /// the work mode published, then the leader sets its synchronization and
    /// the follower its advanced grant fields.
    pub fn configure_external_coex_mode(
        &mut self,
        role: ExternalCoexRole,
        wires: ExternalCoexWires,
    ) {
        let block = &mut self.coexistence.coex_external;
        block.set_coex_external_sync(sync(0));
        block.set_coex_external_work_mode(
            ExternalCoexWorkMode::new(role as u32).expect("a role fits the work mode"),
        );
        match role {
            ExternalCoexRole::Leader => block.set_coex_external_sync(sync(0x14)),
            ExternalCoexRole::Follower => {
                let one_wire = wires == ExternalCoexWires::One;
                block.set_coex_external_advanced_grant_2(nibble(1));
                block.set_coex_external_advanced_grant_1(nibble(if one_wire { 1 } else { 8 }));
                block.set_coex_external_advanced_grant_0(nibble(if one_wire { 1 } else { 0xc }));
            }
        }
    }

    /// `hal_set_extern_coex_delay` with the delay as the upper nibble of its
    /// byte, followed by `hal_set_extern_coex_validity(validate_high)`, as
    /// `ic_set_extern_coex_params` applies them.
    pub fn configure_external_coex_grant(
        &mut self,
        delay: ExternalCoexGrantDelay,
        validate_high: bool,
    ) {
        let block = &mut self.coexistence.coex_external;
        block.set_coex_external_grant_delay(delay);
        block.set_coex_external_validate_high(flag(validate_high));
    }

    /// `hal_set_extern_pti`: the three priorities in the basic modes, or the
    /// fixed advanced image in the follower mode.
    pub fn publish_external_coex_priorities(
        &mut self,
        role: ExternalCoexRole,
        wires: ExternalCoexWires,
        priorities: [ExternalCoexPriority; 3],
    ) {
        let block = &mut self.coexistence.coex_external;
        match role {
            ExternalCoexRole::Follower => {
                let full = !matches!(wires, ExternalCoexWires::One | ExternalCoexWires::Four);
                block.set_coex_external_advanced_priority(nibble(if full { 0xf } else { 0 }));
                block.set_coex_external_priority_2(nibble(0));
                block.set_coex_external_priority_1(nibble(0xf));
                block.set_coex_external_priority_0(nibble(0));
            }
            ExternalCoexRole::Leader => {
                let [first, second, third] = priorities;
                block.set_coex_external_advanced_priority(nibble(0));
                block.set_coex_external_priority_2(first.0);
                block.set_coex_external_priority_1(second.0);
                block.set_coex_external_priority_0(third.0);
            }
        }
    }

    /// `hal_clr_extern_pti`: the follower first saturates its advanced grant
    /// fields; every mode then clears the four priority fields.
    pub fn clear_external_coex_priorities(&mut self, role: ExternalCoexRole) {
        let block = &mut self.coexistence.coex_external;
        if role == ExternalCoexRole::Follower {
            block.set_coex_external_advanced_grant_2(nibble(0xf));
            block.set_coex_external_advanced_grant_1(nibble(0xf));
            block.set_coex_external_advanced_grant_0(nibble(0xf));
        }
        block.set_coex_external_advanced_priority(nibble(0));
        block.set_coex_external_priority_2(nibble(0));
        block.set_coex_external_priority_1(nibble(0));
        block.set_coex_external_priority_0(nibble(0));
    }

    /// `hal_enable_extern_coex` (`true`) or `hal_disable_extern_coex`
    /// (`false`): both enable masks, the second enable bit, then the control
    /// enable.
    pub fn set_external_coex_enabled(&mut self, enabled: bool) {
        let block = &mut self.coexistence.coex_external;
        let mask = nibble(if enabled { 0xf } else { 0 });
        block.set_coex_external_enable_mask_0(mask);
        block.set_coex_external_enable_mask_1(mask);
        block.set_coex_external_enable_1(flag(enabled));
        block.set_coex_external_enable(flag(enabled));
    }
}

fn sync(value: u32) -> ExternalCoexSync {
    ExternalCoexSync::new(value).expect("the synchronization fits ten bits")
}

#[cfg(test)]
mod tests {
    use crate::generated::{
        ExternalCoexGrantDelay,
        coex_external::{self as coex, CoexExternal},
    };
    use std::vec;

    /// The grant delay replaces bits 8:1 and keeps every other bit of the
    /// word it reads.
    ///
    /// SOURCE: BLOB_LIBCOEX_EXTERNAL, `hal_external_coexist.o::hal_set_extern_coex_delay`
    /// replaces bits 8:1; `ic_set_extern_coex_params` supplies `delay << 4`.
    #[test]
    fn grant_delay_replaces_the_complete_vendor_byte_and_preserves_other_fields() {
        for (initial, delay, expected) in [
            (0xffff_ffff_u32, 0, 0xffff_fe01_u32),
            (0xffff_fffe, 5, 0xffff_fea0),
            (0x5a5a_5a5a, 15, 0x5a5a_5be0),
            (0xa5a5_a5a5, 0, 0xa5a5_a401),
            (0, 5, 0xa0),
        ] {
            let mut memory = vec![initial; coex::SIZE / 4];
            let mut block = CoexExternal::host(&mut memory).expect("memory holds the block");
            block.set_coex_external_grant_delay(
                ExternalCoexGrantDelay::new(delay).expect("a four-bit delay"),
            );
            drop(block);
            let words: vec::Vec<u32> = memory.into_iter().filter(|word| *word != initial).collect();
            assert!(words.len() <= 1, "one word changes at most");
            assert_eq!(words.first().copied().unwrap_or(initial), expected);
        }
    }
}
