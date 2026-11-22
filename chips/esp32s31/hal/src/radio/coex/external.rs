//! Exact generated-PAC transactions of external coexistence.
//!
//! Each method is one complete `hal_external_coexist.o` leaf of the pinned
//! libcoexist, in its order of fresh-read RMW edges. The routing of the
//! external signals through the GPIO matrix belongs to the platform.

#![forbid(unsafe_code)]

use crate::radio::modem::values::{ExternalCoexGrantDelay, ExternalCoexNibble};
use oer_chips_esp32s31_pac::coex::pti::WorkMode;

/// Whether the external grant is valid high or low
/// (`hal_set_extern_coex_validity`).
pub use oer_chips_esp32s31_pac::coex::control::ValidateHigh as ExternalCoexGrantValidity;

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

/// One finite external-coexistence transaction over its exclusive part.
pub struct ExternalCoexControl<'a, P: oer_portable_mmio::Placement> {
    block: &'a mut oer_chips_esp32s31_pac::coex::CoexExternal<P>,
}
impl<'a, P: oer_portable_mmio::Placement> ExternalCoexControl<'a, P> {
    pub(crate) fn new(block: &'a mut oer_chips_esp32s31_pac::coex::CoexExternal<P>) -> Self {
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
            w.set_sync(oer_portable_mmio::Bits::constant::<0>());
        });
        block.modify_pti(|w| {
            w.set_work_mode(match role {
                ExternalCoexRole::Leader => WorkMode::Leader.into(),
                ExternalCoexRole::Follower => WorkMode::Follower.into(),
            });
        });
        match role {
            ExternalCoexRole::Leader => block.modify_control(|w| {
                w.set_sync(oer_portable_mmio::Bits::constant::<0x14>());
            }),
            ExternalCoexRole::Follower => {
                let one_wire = wires == ExternalCoexWires::One;
                block.modify_pti(|w| {
                    w.set_bits_24_27_opaque(oer_portable_mmio::Bits::constant::<1>());
                });
                block.modify_pti(|w| {
                    w.set_bits_20_23_opaque(if one_wire {
                        oer_portable_mmio::Bits::constant::<1>()
                    } else {
                        oer_portable_mmio::Bits::constant::<8>()
                    });
                });
                block.modify_pti(|w| {
                    w.set_bits_16_19_opaque(if one_wire {
                        oer_portable_mmio::Bits::constant::<1>()
                    } else {
                        oer_portable_mmio::Bits::constant::<0xc>()
                    });
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
        validity: ExternalCoexGrantValidity,
    ) {
        let block = &mut *self.block;
        block.modify_control(|w| {
            w.set_grant_delay_low(oer_portable_mmio::Bits::constant::<0>());
            w.set_grant_delay(delay.field());
        });
        block.modify_control(|w| {
            w.set_validate_high(validity);
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
                    w.set_advanced_priority(if full {
                        oer_portable_mmio::Bits::constant::<0xf>()
                    } else {
                        oer_portable_mmio::Bits::constant::<0>()
                    });
                });
                block.modify_pti(|w| {
                    w.set_priority_2(oer_portable_mmio::Bits::constant::<0>());
                });
                block.modify_pti(|w| {
                    w.set_priority_1(oer_portable_mmio::Bits::constant::<0xf>());
                });
                block.modify_pti(|w| {
                    w.set_priority_0(oer_portable_mmio::Bits::constant::<0>());
                });
            }
            ExternalCoexRole::Leader => {
                let [first, second, third] = priorities;
                block.modify_pti(|w| {
                    w.set_advanced_priority(oer_portable_mmio::Bits::constant::<0>());
                });
                block.modify_pti(|w| {
                    w.set_priority_2(first.0.field());
                });
                block.modify_pti(|w| {
                    w.set_priority_1(second.0.field());
                });
                block.modify_pti(|w| {
                    w.set_priority_0(third.0.field());
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
                w.set_bits_24_27_opaque(oer_portable_mmio::Bits::constant::<0xf>());
            });
            block.modify_pti(|w| {
                w.set_bits_20_23_opaque(oer_portable_mmio::Bits::constant::<0xf>());
            });
            block.modify_pti(|w| {
                w.set_bits_16_19_opaque(oer_portable_mmio::Bits::constant::<0xf>());
            });
        }
        block.modify_pti(|w| {
            w.set_advanced_priority(oer_portable_mmio::Bits::constant::<0>());
        });
        block.modify_pti(|w| {
            w.set_priority_2(oer_portable_mmio::Bits::constant::<0>());
        });
        block.modify_pti(|w| {
            w.set_priority_1(oer_portable_mmio::Bits::constant::<0>());
        });
        block.modify_pti(|w| {
            w.set_priority_0(oer_portable_mmio::Bits::constant::<0>());
        });
    }

    /// `hal_enable_extern_coex`: both enable masks, the second enable bit,
    /// then the control enable.
    pub fn enable_external_coex(&mut self) {
        self.write_external_coex_enable(
            oer_portable_mmio::Bits::constant::<0xf>(),
            oer_chips_esp32s31_pac::coex::dictionaries::ExternalCoexEnable::Enabled,
        );
    }

    /// `hal_disable_extern_coex`: the same four writes, cleared.
    pub fn disable_external_coex(&mut self) {
        self.write_external_coex_enable(
            oer_portable_mmio::Bits::constant::<0>(),
            oer_chips_esp32s31_pac::coex::dictionaries::ExternalCoexEnable::Disabled,
        );
    }

    fn write_external_coex_enable(
        &mut self,
        mask: oer_portable_mmio::Bits<4>,
        enable: oer_chips_esp32s31_pac::coex::dictionaries::ExternalCoexEnable,
    ) {
        let block = &mut *self.block;
        block.modify_enable_mask_0(|w| {
            w.set_mask(mask);
        });
        block.modify_enable_mask_1(|w| {
            w.set_mask(mask);
        });
        block.modify_enable_1(|w| {
            w.set_enable(enable);
        });
        block.modify_control(|w| {
            w.set_enable(enable);
        });
    }
}

impl crate::radio::modem::SharedRadioRegisters {
    pub fn configure_external_coex_mode(
        &mut self,
        role: ExternalCoexRole,
        wires: ExternalCoexWires,
    ) {
        ExternalCoexControl::new(&mut self.coexistence.coex_external)
            .configure_external_coex_mode(role, wires);
    }
    pub fn configure_external_coex_grant(
        &mut self,
        delay: ExternalCoexGrantDelay,
        validity: ExternalCoexGrantValidity,
    ) {
        ExternalCoexControl::new(&mut self.coexistence.coex_external)
            .configure_external_coex_grant(delay, validity);
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
    pub fn enable_external_coex(&mut self) {
        ExternalCoexControl::new(&mut self.coexistence.coex_external).enable_external_coex();
    }
    pub fn disable_external_coex(&mut self) {
        ExternalCoexControl::new(&mut self.coexistence.coex_external).disable_external_coex();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_chips_esp32s31_pac::{coex::CoexExternal, instances};
    use std::vec;
    #[test]
    fn grant_delay_replaces_the_entire_vendor_byte_and_keeps_all_other_words() {
        for (initial, delay, high, expected_word) in [
            (
                0xffff_ffff,
                0,
                ExternalCoexGrantValidity::ValidHigh,
                0xffff_fe01,
            ),
            (
                0xffff_fffe,
                5,
                ExternalCoexGrantValidity::ValidLow,
                0xffff_fea0,
            ),
            (
                0x5a5a_5a5a,
                15,
                ExternalCoexGrantValidity::ValidLow,
                0x5a5a_5be0,
            ),
            (
                0xa5a5_a5a5,
                0,
                ExternalCoexGrantValidity::ValidHigh,
                0xa5a5_a401,
            ),
            (0, 5, ExternalCoexGrantValidity::ValidLow, 0xa0),
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
