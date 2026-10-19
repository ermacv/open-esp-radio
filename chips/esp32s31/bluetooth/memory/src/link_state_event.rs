//! The event word every role's link state shares and its projection into a
//! scheduler item.
//!
//! Byte `+0x60` of a link state is the event priority and byte `+0x61` the
//! index of the rounded transmit power
//! (`r_sym_ble_5YIHPyqYswSaQs7KMvEA` rounds a signed dBm request to it). The
//! common insertion copies that index into item `+0x14` bits 27:20; an LE 1M
//! role also clears the rate lanes in bits 31:28. The two bytes above stay
//! with each role's own codec.

#![forbid(unsafe_code)]

use crate::le_tx_power::LeTxPower;

const PRIORITY_BYTE: u32 = 0x0000_00ff;
const POWER_BYTE: u32 = 0x0000_ff00;
const POWER_SHIFT: u32 = 8;
const ITEM_POWER_LANES: u32 = 0x0ff0_0000;
const ITEM_RATE_LANES: u32 = 0xf000_0000;
const ITEM_POWER_SHIFT: u32 = 20;

/// Link-state word `+0x60`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LinkStateEventWord(u32);

impl LinkStateEventWord {
    pub(crate) const fn from_word(word: u32) -> Self {
        Self(word)
    }

    pub(crate) const fn word(self) -> u32 {
        self.0
    }

    /// Replace the event priority byte.
    pub(crate) const fn with_priority(self, priority: u8) -> Self {
        Self((self.0 & !PRIORITY_BYTE) | priority as u32)
    }

    /// Replace the transmit-power index byte.
    pub(crate) const fn with_power(self, power: LeTxPower) -> Self {
        Self((self.0 & !POWER_BYTE) | ((power.index() as u32) << POWER_SHIFT))
    }

    /// The transmit-power index a scheduler item copies.
    pub(crate) const fn power_index(self) -> u8 {
        ((self.0 & POWER_BYTE) >> POWER_SHIFT) as u8
    }
}

/// Item `+0x14` carrying `power_index` in its power lanes; every other bit
/// is kept.
pub(crate) const fn item_with_power(word_14: u32, power_index: u8) -> u32 {
    (word_14 & !ITEM_POWER_LANES) | ((power_index as u32) << ITEM_POWER_SHIFT)
}

/// The power index item `+0x14` carries.
#[cfg(test)]
pub(crate) const fn item_power_index(word_14: u32) -> u8 {
    ((word_14 & ITEM_POWER_LANES) >> ITEM_POWER_SHIFT) as u8
}

/// Item `+0x14` of an LE 1M event: zero rate lanes and `power_index` in the
/// power lanes.
pub(crate) const fn item_with_le_1m_power(word_14: u32, power_index: u8) -> u32 {
    item_with_power(word_14 & !ITEM_RATE_LANES, power_index)
}

#[cfg(test)]
mod tests;
