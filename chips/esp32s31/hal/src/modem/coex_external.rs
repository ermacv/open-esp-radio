//! Exact generated-PAC transactions of external coexistence.
//!
//! Each method is one complete `hal_external_coexist.o` leaf of the pinned
//! libcoexist, in its order of fresh-read RMW edges. The routing of the
//! external signals through the GPIO matrix belongs to the platform.

#![forbid(unsafe_code)]

use crate::modem::values::{
    ExternalCoexFlag, ExternalCoexGrantDelay, ExternalCoexNibble, ExternalCoexSync,
    ExternalCoexWorkMode,
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
    Leader,
    /// The external radio arbitrates: the chip requests and it grants.
    Follower,
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

/// One finite external-coexistence transaction over its exclusive part.
pub struct ExternalCoexControl<'a, P: oer_mmio::Placement> {
    block: &'a mut oer_esp32s31_pac::coex::CoexExternal<P>,
}
impl<'a, P: oer_mmio::Placement> ExternalCoexControl<'a, P> {
    pub(crate) fn new(block: &'a mut oer_esp32s31_pac::coex::CoexExternal<P>) -> Self {
        Self { block }
    }
    /// `hal_set_extern_pti_mode(role)` with the wire count of
    /// `esp_coex_external_get_wire_type`: the synchronization is cleared and
    /// the work mode published, then the leader sets its synchronization and
    /// the follower its advanced grant fields.
    pub fn configure_external_coex_mode(
        &mut self,
        role: ExternalCoexRole,
        wires: ExternalCoexWires,
    ) {
        let block = &mut *self.block;
        block.modify_control(|w| {
            w.set_sync(oer_mmio::Bits::truncate((sync(0)).get()));
        });
        block.modify_pti(|w| {
            w.set_work_mode(oer_mmio::Bits::truncate(
                (ExternalCoexWorkMode::new(match role {
                    ExternalCoexRole::Leader => {
                        oer_esp32s31_pac::coex::pti::WorkMode::Leader.bits()
                    }
                    ExternalCoexRole::Follower => {
                        oer_esp32s31_pac::coex::pti::WorkMode::Follower.bits()
                    }
                })
                .expect("a role fits the work mode"))
                .get(),
            ));
        });
        match role {
            ExternalCoexRole::Leader => block.modify_control(|w| {
                w.set_sync(oer_mmio::Bits::truncate((sync(0x14)).get()));
            }),
            ExternalCoexRole::Follower => {
                let one_wire = wires == ExternalCoexWires::One;
                block.modify_pti(|w| {
                    w.set_bits_24_27_opaque(oer_mmio::Bits::truncate((nibble(1)).get()));
                });
                block.modify_pti(|w| {
                    w.set_bits_20_23_opaque(oer_mmio::Bits::truncate(
                        (nibble(if one_wire { 1 } else { 8 })).get(),
                    ));
                });
                block.modify_pti(|w| {
                    w.set_bits_16_19_opaque(oer_mmio::Bits::truncate(
                        (nibble(if one_wire { 1 } else { 0xc })).get(),
                    ));
                });
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
        let block = &mut *self.block;
        block.modify_control(|w| {
            w.set_grant_delay_low(oer_mmio::Bits::truncate(0));
            w.set_grant_delay(oer_mmio::Bits::truncate(delay.get()));
        });
        block.modify_control(|w| {
            w.set_validate_high((flag(validate_high)).get() != 0);
        });
    }

    /// `hal_set_extern_pti`: the three priorities in the basic modes, or the
    /// fixed advanced image in the follower mode.
    pub fn publish_external_coex_priorities(
        &mut self,
        role: ExternalCoexRole,
        wires: ExternalCoexWires,
        priorities: [ExternalCoexPriority; 3],
    ) {
        let block = &mut *self.block;
        match role {
            ExternalCoexRole::Follower => {
                let full = !matches!(wires, ExternalCoexWires::One | ExternalCoexWires::Four);
                block.modify_pti(|w| {
                    w.set_advanced_priority(oer_mmio::Bits::truncate(
                        (nibble(if full { 0xf } else { 0 })).get(),
                    ));
                });
                block.modify_pti(|w| {
                    w.set_priority_2(oer_mmio::Bits::truncate((nibble(0)).get()));
                });
                block.modify_pti(|w| {
                    w.set_priority_1(oer_mmio::Bits::truncate((nibble(0xf)).get()));
                });
                block.modify_pti(|w| {
                    w.set_priority_0(oer_mmio::Bits::truncate((nibble(0)).get()));
                });
            }
            ExternalCoexRole::Leader => {
                let [first, second, third] = priorities;
                block.modify_pti(|w| {
                    w.set_advanced_priority(oer_mmio::Bits::truncate((nibble(0)).get()));
                });
                block.modify_pti(|w| {
                    w.set_priority_2(oer_mmio::Bits::truncate((first.0).get()));
                });
                block.modify_pti(|w| {
                    w.set_priority_1(oer_mmio::Bits::truncate((second.0).get()));
                });
                block.modify_pti(|w| {
                    w.set_priority_0(oer_mmio::Bits::truncate((third.0).get()));
                });
            }
        }
    }

    /// `hal_clr_extern_pti`: the follower first saturates its advanced grant
    /// fields; every mode then clears the four priority fields.
    pub fn clear_external_coex_priorities(&mut self, role: ExternalCoexRole) {
        let block = &mut *self.block;
        if role == ExternalCoexRole::Follower {
            block.modify_pti(|w| {
                w.set_bits_24_27_opaque(oer_mmio::Bits::truncate((nibble(0xf)).get()));
            });
            block.modify_pti(|w| {
                w.set_bits_20_23_opaque(oer_mmio::Bits::truncate((nibble(0xf)).get()));
            });
            block.modify_pti(|w| {
                w.set_bits_16_19_opaque(oer_mmio::Bits::truncate((nibble(0xf)).get()));
            });
        }
        block.modify_pti(|w| {
            w.set_advanced_priority(oer_mmio::Bits::truncate((nibble(0)).get()));
        });
        block.modify_pti(|w| {
            w.set_priority_2(oer_mmio::Bits::truncate((nibble(0)).get()));
        });
        block.modify_pti(|w| {
            w.set_priority_1(oer_mmio::Bits::truncate((nibble(0)).get()));
        });
        block.modify_pti(|w| {
            w.set_priority_0(oer_mmio::Bits::truncate((nibble(0)).get()));
        });
    }

    /// `hal_enable_extern_coex` (`true`) or `hal_disable_extern_coex`
    /// (`false`): both enable masks, the second enable bit, then the control
    /// enable.
    pub fn set_external_coex_enabled(&mut self, enabled: bool) {
        let block = &mut *self.block;
        let mask = nibble(if enabled { 0xf } else { 0 });
        block.modify_enable_mask_0(|w| {
            w.set_mask(oer_mmio::Bits::truncate((mask).get()));
        });
        block.modify_enable_mask_1(|w| {
            w.set_mask(oer_mmio::Bits::truncate((mask).get()));
        });
        block.modify_enable_1(|w| {
            w.set_enable((flag(enabled)).get() != 0);
        });
        block.modify_control(|w| {
            w.set_enable((flag(enabled)).get() != 0);
        });
    }
}

fn sync(value: u32) -> ExternalCoexSync {
    ExternalCoexSync::new(value).expect("the synchronization fits ten bits")
}

impl super::SharedRadioRegisters {
    pub fn configure_external_coex_mode(
        &mut self,
        role: ExternalCoexRole,
        wires: ExternalCoexWires,
    ) {
        ExternalCoexControl::new(&mut self.coexistence.coex_external)
            .configure_external_coex_mode(role, wires);
    }
    pub fn configure_external_coex_grant(&mut self, delay: ExternalCoexGrantDelay, high: bool) {
        ExternalCoexControl::new(&mut self.coexistence.coex_external)
            .configure_external_coex_grant(delay, high);
    }
    pub fn publish_external_coex_priorities(
        &mut self,
        role: ExternalCoexRole,
        wires: ExternalCoexWires,
        priorities: [ExternalCoexPriority; 3],
    ) {
        ExternalCoexControl::new(&mut self.coexistence.coex_external)
            .publish_external_coex_priorities(role, wires, priorities);
    }
    pub fn clear_external_coex_priorities(&mut self, role: ExternalCoexRole) {
        ExternalCoexControl::new(&mut self.coexistence.coex_external)
            .clear_external_coex_priorities(role);
    }
    pub fn set_external_coex_enabled(&mut self, enabled: bool) {
        ExternalCoexControl::new(&mut self.coexistence.coex_external)
            .set_external_coex_enabled(enabled);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_esp32s31_pac::{coex::CoexExternal, instances};
    use std::vec;
    #[test]
    fn grant_delay_replaces_the_entire_vendor_byte_and_keeps_all_other_words() {
        for (initial, delay, high, expected_word) in [
            (0xffff_ffff, 0, true, 0xffff_fe01),
            (0xffff_fffe, 5, false, 0xffff_fea0),
            (0x5a5a_5a5a, 15, false, 0x5a5a_5be0),
            (0xa5a5_a5a5, 0, true, 0xa5a5_a401),
            (0, 5, false, 0xa0),
        ] {
            let mut memory = vec![initial; instances::coex::SIZE.div_ceil(4)];
            let mut expected = memory.clone();
            let mut part = CoexExternal::host(&mut memory).expect("external part fits");
            ExternalCoexControl::new(&mut part).configure_external_coex_grant(
                ExternalCoexGrantDelay::new(delay).expect("four-bit delay"),
                high,
            );
            drop(part);
            expected[0xa0 / 4] = expected_word;
            assert_eq!(memory, expected);
        }
    }
}
