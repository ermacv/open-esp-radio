//! Exact generated-PAC transactions of external coexistence.
//!
//! Each method is one complete `hal_external_coexist.o` leaf of the pinned
//! libcoexist, in its order of fresh-read RMW edges. The routing of the
//! external signals through the GPIO matrix belongs to the platform.

#![forbid(unsafe_code)]

use crate::{
    SharedRadioRegisters,
    generated::{
        self, ExternalCoexFlag, ExternalCoexGrantDelay, ExternalCoexNibble, ExternalCoexSync,
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
        let block = &self.coexistence.coex_external;
        generated::set_coex_external_sync(block, sync(0));
        generated::set_coex_external_work_mode(
            block,
            ExternalCoexWorkMode::new(role as u32).expect("a role fits the work mode"),
        );
        match role {
            ExternalCoexRole::Leader => generated::set_coex_external_sync(block, sync(0x14)),
            ExternalCoexRole::Follower => {
                let one_wire = wires == ExternalCoexWires::One;
                generated::set_coex_external_advanced_grant_2(block, nibble(1));
                generated::set_coex_external_advanced_grant_1(
                    block,
                    nibble(if one_wire { 1 } else { 8 }),
                );
                generated::set_coex_external_advanced_grant_0(
                    block,
                    nibble(if one_wire { 1 } else { 0xc }),
                );
            }
        }
    }

    /// `hal_set_extern_coex_delay((delay_us << 4) & 0xf0)` followed by
    /// `hal_set_extern_coex_validity(validate_high)`, as
    /// `ic_set_extern_coex_params` applies them.
    pub fn configure_external_coex_grant(&mut self, delay_us: u8, validate_high: bool) {
        let block = &self.coexistence.coex_external;
        generated::set_coex_external_grant_delay(
            block,
            ExternalCoexGrantDelay::new(u32::from(delay_us << 4) & 0xf0)
                .expect("the delay image fits eight bits"),
        );
        generated::set_coex_external_validate_high(block, flag(validate_high));
    }

    /// `hal_set_extern_pti`: the three priorities in the basic modes, or the
    /// fixed advanced image in the follower mode.
    pub fn publish_external_coex_priorities(
        &mut self,
        role: ExternalCoexRole,
        wires: ExternalCoexWires,
        priorities: [ExternalCoexPriority; 3],
    ) {
        let block = &self.coexistence.coex_external;
        match role {
            ExternalCoexRole::Follower => {
                let full = !matches!(wires, ExternalCoexWires::One | ExternalCoexWires::Four);
                generated::set_coex_external_advanced_priority(
                    block,
                    nibble(if full { 0xf } else { 0 }),
                );
                generated::set_coex_external_priority_2(block, nibble(0));
                generated::set_coex_external_priority_1(block, nibble(0xf));
                generated::set_coex_external_priority_0(block, nibble(0));
            }
            ExternalCoexRole::Leader => {
                let [first, second, third] = priorities;
                generated::set_coex_external_advanced_priority(block, nibble(0));
                generated::set_coex_external_priority_2(block, first.0);
                generated::set_coex_external_priority_1(block, second.0);
                generated::set_coex_external_priority_0(block, third.0);
            }
        }
    }

    /// `hal_clr_extern_pti`: the follower first saturates its advanced grant
    /// fields; every mode then clears the four priority fields.
    pub fn clear_external_coex_priorities(&mut self, role: ExternalCoexRole) {
        let block = &self.coexistence.coex_external;
        if role == ExternalCoexRole::Follower {
            generated::set_coex_external_advanced_grant_2(block, nibble(0xf));
            generated::set_coex_external_advanced_grant_1(block, nibble(0xf));
            generated::set_coex_external_advanced_grant_0(block, nibble(0xf));
        }
        generated::set_coex_external_advanced_priority(block, nibble(0));
        generated::set_coex_external_priority_2(block, nibble(0));
        generated::set_coex_external_priority_1(block, nibble(0));
        generated::set_coex_external_priority_0(block, nibble(0));
    }

    /// `hal_enable_extern_coex` (`true`) or `hal_disable_extern_coex`
    /// (`false`): both enable masks, the second enable bit, then the control
    /// enable.
    pub fn set_external_coex_enabled(&mut self, enabled: bool) {
        let block = &self.coexistence.coex_external;
        let mask = nibble(if enabled { 0xf } else { 0 });
        generated::set_coex_external_enable_mask_0(block, mask);
        generated::set_coex_external_enable_mask_1(block, mask);
        generated::set_coex_external_enable_1(block, flag(enabled));
        generated::set_coex_external_enable(block, flag(enabled));
    }
}

fn sync(value: u32) -> ExternalCoexSync {
    ExternalCoexSync::new(value).expect("the synchronization fits ten bits")
}
