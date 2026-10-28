//! Legacy rates of the Supported Rates and BSS Membership Selectors element
//! and the Extended Supported Rates and BSS Membership Selectors element.
//!
//! Each octet of either element body names a rate in 500-kb/s units or a
//! BSS membership selector, with bit seven marking a member of the
//! BSSBasicRateSet.

use crate::phy::{DsssPreamble, LegacyRate};

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.3 (Supported Rates and BSS
    /// Membership Selectors element): the rate in 500-kb/s units in bits 0
    /// to 6 and the basic flag in bit 7.
    pub struct RateField[1] (little) {
        pub rate, with_rate: u8 = 0..7;
        pub basic, with_basic: bool = 7..8;
    }
}

/// The non-HT rates a rate field names, in 500-kb/s units, each with its
/// long-preamble form. Entry `i` is bit `i` of a [`LegacyRateSet`].
/// SOURCE: IEEE Std 802.11-2020, 9.4.2.3 (rates of Clauses 15 to 18).
pub const LEGACY_RATES: [(u8, LegacyRate); 12] = [
    (2, LegacyRate::Dsss1M),
    (4, LegacyRate::Dsss2M(DsssPreamble::Long)),
    (11, LegacyRate::Cck5M5(DsssPreamble::Long)),
    (22, LegacyRate::Cck11M(DsssPreamble::Long)),
    (12, LegacyRate::Ofdm6M),
    (18, LegacyRate::Ofdm9M),
    (24, LegacyRate::Ofdm12M),
    (36, LegacyRate::Ofdm18M),
    (48, LegacyRate::Ofdm24M),
    (72, LegacyRate::Ofdm36M),
    (96, LegacyRate::Ofdm48M),
    (108, LegacyRate::Ofdm54M),
];

/// The bits of a [`LegacyRateSet`] that name an entry of [`LEGACY_RATES`].
const RATE_BITS: u16 = const {
    match 1_u16.checked_shl(LEGACY_RATES.len() as u32) {
        Some(beyond) => beyond.wrapping_sub(1),
        None => panic!("every legacy rate has a bit"),
    }
};

/// Which rate fields of the elements a set collects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RateSelection {
    /// Every named rate.
    All,
    /// Only the rates marked basic.
    Basic,
}

/// A set of the rates of [`LEGACY_RATES`]; bit `i` is entry `i`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LegacyRateSet(u16);

impl LegacyRateSet {
    /// No rate.
    pub const EMPTY: Self = Self(0);

    /// The set whose bit `i` names entry `i` of [`LEGACY_RATES`]; bits
    /// above the table name no rate.
    pub const fn from_bits(bits: u16) -> Self {
        Self(bits & RATE_BITS)
    }

    /// The rates `selection` admits in Supported Rates and Extended
    /// Supported Rates element bodies. Membership selectors and unknown
    /// encodings name no legacy rate.
    pub fn from_rate_elements(supported: &[u8], extended: &[u8], selection: RateSelection) -> Self {
        let mut bits = 0_u16;
        for field in supported
            .iter()
            .chain(extended)
            .map(|&octet| RateField::from_bytes([octet]))
        {
            if selection == RateSelection::Basic && !field.basic() {
                continue;
            }
            for (bit, (units, _)) in (0_u16..).zip(LEGACY_RATES) {
                if field.rate() == units {
                    bits |= 1_u16.checked_shl(u32::from(bit)).unwrap_or(0);
                }
            }
        }
        Self(bits)
    }

    /// The set's bits, entry `i` of [`LEGACY_RATES`] at bit `i`.
    pub const fn bits(self) -> u16 {
        self.0
    }

    /// Whether no rate is present.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The rates of the set in table order, each with its long preamble.
    pub fn rates(self) -> impl Iterator<Item = LegacyRate> {
        (0_u16..)
            .zip(LEGACY_RATES)
            .filter(move |(bit, _)| {
                self.0
                    .checked_shr(u32::from(*bit))
                    .is_some_and(|rest| rest & 1 != 0)
            })
            .map(|(_, (_, rate))| rate)
    }
}

#[cfg(test)]
mod tests;
