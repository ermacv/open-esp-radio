//! HE SU PPDU duration from its geometry: preamble, pre-FEC padded data
//! symbols, LDPC extra symbol segment and packet extension.
//!
//! One spatial stream, no STBC (`m_STBC = 1`), one BCC encoder (`N_ES = 1`),
//! no midamble (`N_MA = 0`), not ER SU and not DCM. The caller selects the
//! per-RU symbol geometry; this module applies the standard's equations to it.
//!
//! SOURCE: IEEE Std 802.11ax-2021, clause 27 pre-FEC padding process
//! (`N_excess`, pre-FEC padding factor `a`, `N_SYM`, LDPC extra symbol
//! segment), 27.3.12 (packet extension) and 27.4.3 (TXTIME and PSDU_LENGTH
//! calculation, HE SU). The equations are written from the standard's
//! procedure; its text was not consulted for this change. 27.3.12 for the PE
//! field, its PE duration table and Equations (27-83)/(27-84) for `a` are as
//! MathWorks `wlanHESUConfig` cites them; no other equation or table number
//! is cited.

use super::ceil_div;
use core::num::{NonZeroU16, NonZeroU64};

/// SERVICE field bits.
///
/// SOURCE: IEEE Std 802.11ax-2021, clause 27 pre-FEC padding process
/// (`N_service = 16`).
const SERVICE_BITS: u64 = 16;
/// BCC tail bits per encoder; LDPC has none.
///
/// SOURCE: IEEE Std 802.11ax-2021, clause 27 pre-FEC padding process
/// (`N_tail = 6` for BCC, 0 for LDPC).
const BCC_TAIL_BITS: u64 = 6;
/// Duration of an HE NDP's packet extension, whatever the nominal padding.
///
/// SOURCE: IEEE Std 802.11ax-2021, 27.3.12 (an HE sounding NDP, APEP_LENGTH
/// 0, has no data field and a 4-us PE field).
const NDP_PACKET_EXTENSION_MICROS: u64 = 4;

const NANOS_PER_MICRO: NonZeroU64 = at_least_one(1_000);

/// `value` for a nonzero `value`, one otherwise; total, for constant divisors.
const fn at_least_one(value: u64) -> NonZeroU64 {
    NonZeroU64::MIN.saturating_add(value.saturating_sub(1))
}

/// LDPC code rate `R` of an HE-MCS.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LdpcCodeRate {
    Half,
    TwoThirds,
    ThreeQuarters,
    FiveSixths,
}

impl LdpcCodeRate {
    /// `R` as numerator and nonzero denominator.
    const fn fraction(self) -> (u64, NonZeroU64) {
        let (numerator, denominator) = match self {
            Self::Half => (1, 2),
            Self::TwoThirds => (2, 3),
            Self::ThreeQuarters => (3, 4),
            Self::FiveSixths => (5, 6),
        };
        (numerator, at_least_one(denominator))
    }
}

/// Forward error correction of the data field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeSuCoding {
    Bcc,
    Ldpc(LdpcCodeRate),
}

/// Nominal packet padding `T_PE,nominal` of the receiver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeNominalPacketPadding {
    Micros0,
    Micros8,
    Micros16,
}

impl HeNominalPacketPadding {
    /// PE field duration for pre-FEC padding factor `a` (1..=4).
    ///
    /// SOURCE: IEEE Std 802.11ax-2021, 27.3.12, PE field duration table:
    /// nominal 0 us gives 0 for every `a`; nominal 8 us gives 0, 0, 4, 8;
    /// nominal 16 us gives 4, 8, 12, 16 for `a` = 1, 2, 3, 4.
    const fn packet_extension_micros(self, a: u64) -> u64 {
        match self {
            Self::Micros0 => 0,
            Self::Micros8 => a.saturating_sub(2).saturating_mul(4),
            Self::Micros16 => a.saturating_mul(4),
        }
    }
}

/// Symbol geometry and overhead of one HE SU PPDU format.
///
/// `preamble_nanos` is everything before the data field: L-STF, L-LTF,
/// L-SIG, RL-SIG, HE-SIG-A, HE-STF and `N_HE-LTF` HE-LTF symbols, each with
/// its guard interval. `symbol_nanos` is `T_SYM` = 12.8 us + GI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeSuTiming {
    /// `N_DBPS`.
    pub data_bits_per_symbol: NonZeroU16,
    /// `N_CBPS`.
    pub coded_bits_per_symbol: NonZeroU16,
    /// `N_DBPS,short`.
    pub short_data_bits_per_symbol: NonZeroU16,
    /// `N_CBPS,short`.
    pub short_coded_bits_per_symbol: NonZeroU16,
    pub coding: HeSuCoding,
    pub symbol_nanos: NonZeroU16,
    pub preamble_nanos: u16,
    pub nominal_packet_padding: HeNominalPacketPadding,
    pub signal_extension_micros: u8,
}

/// Data symbols and final pre-FEC padding factor of one APEP length.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct HeSuDataSymbols {
    pub symbols: u64,
    /// Pre-FEC padding factor `a`, 1..=4.
    pub padding_factor: u64,
}

// Every operation below is total: division and remainder use nonzero
// divisors, the rest saturates. The u16 APEP bounds every value far below
// saturation (at most 524,302 bits), so no saturation is reached.
#[deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable
)]
impl HeSuTiming {
    /// `N_SYM` and `a` for an APEP_LENGTH of `apep_bytes`.
    ///
    /// SOURCE: IEEE Std 802.11ax-2021, clause 27 pre-FEC padding process:
    /// `N_excess`, `a_init`, `N_SYM,init`, then for LDPC `N_pld`/`N_avbits`
    /// and the LDPC encoding process of IEEE Std 802.11-2020, 19.3.11.7.5
    /// steps b) to d), whose extra symbol segment makes `a = a_init + 1`, or
    /// `a = 1` and one more symbol when `a_init = 4`.
    pub(super) fn data_symbols(self, apep_bytes: u16) -> HeSuDataSymbols {
        let tail = match self.coding {
            HeSuCoding::Bcc => BCC_TAIL_BITS,
            HeSuCoding::Ldpc(_) => 0,
        };
        let data = NonZeroU64::from(self.data_bits_per_symbol);
        let short = NonZeroU64::from(self.short_data_bits_per_symbol);
        let bits = (u64::from(apep_bytes) << 3)
            .saturating_add(tail)
            .saturating_add(SERVICE_BITS);
        let initial_symbols = ceil_div(bits, data);
        let excess = bits % data;
        let initial_factor = if excess == 0 {
            4
        } else {
            ceil_div(excess, short).min(4)
        };
        let HeSuCoding::Ldpc(rate) = self.coding else {
            return HeSuDataSymbols {
                symbols: initial_symbols,
                padding_factor: initial_factor,
            };
        };
        let (last_data, last_coded) = if initial_factor == 4 {
            (
                u64::from(self.data_bits_per_symbol.get()),
                u64::from(self.coded_bits_per_symbol.get()),
            )
        } else {
            (
                initial_factor.saturating_mul(short.get()),
                initial_factor.saturating_mul(u64::from(self.short_coded_bits_per_symbol.get())),
            )
        };
        let full_symbols = initial_symbols.saturating_sub(1);
        let payload = full_symbols
            .saturating_mul(data.get())
            .saturating_add(last_data);
        let available = full_symbols
            .saturating_mul(u64::from(self.coded_bits_per_symbol.get()))
            .saturating_add(last_coded);
        if !ldpc_extra_symbol_segment(payload, available, rate) {
            HeSuDataSymbols {
                symbols: initial_symbols,
                padding_factor: initial_factor,
            }
        } else if initial_factor == 4 {
            HeSuDataSymbols {
                symbols: initial_symbols.saturating_add(1),
                padding_factor: 1,
            }
        } else {
            HeSuDataSymbols {
                symbols: initial_symbols,
                padding_factor: initial_factor.saturating_add(1),
            }
        }
    }

    /// TXTIME rounded up to a whole microsecond.
    ///
    /// SOURCE: IEEE Std 802.11ax-2021, 27.4.3: TXTIME = 20 + T_HE-PREAMBLE +
    /// N_SYM x T_SYM + N_MA x N_HE-LTF x T_HE-LTF-SYM + T_PE +
    /// SignalExtension, with `N_MA = 0`. The standard does not round an HE
    /// TXTIME; this model rounds its sum up once. APEP_LENGTH 0 is an NDP.
    pub fn duration_micros(self, apep_bytes: u16) -> u32 {
        let (data_nanos, extension_micros) = if apep_bytes == 0 {
            (0, NDP_PACKET_EXTENSION_MICROS)
        } else {
            let data = self.data_symbols(apep_bytes);
            (
                data.symbols
                    .saturating_mul(u64::from(self.symbol_nanos.get())),
                self.nominal_packet_padding
                    .packet_extension_micros(data.padding_factor),
            )
        };
        let nanos = u64::from(self.preamble_nanos)
            .saturating_add(data_nanos)
            .saturating_add(
                extension_micros
                    .saturating_add(u64::from(self.signal_extension_micros))
                    .saturating_mul(NANOS_PER_MICRO.get()),
            );
        // At most 524,302 symbols of 65,535 ns: far below u32 microseconds.
        u32::try_from(ceil_div(nanos, NANOS_PER_MICRO)).unwrap_or(u32::MAX)
    }

    /// Largest nonzero APEP length whose PPDU fits `micros`, capped at u16.
    ///
    /// For a symbol longer than 12 us (every HE GI gives 13.6 us or more) the
    /// duration is nondecreasing in APEP length: `(N_SYM, a)` grows in
    /// quarter-symbol steps, an LDPC extra segment moves it one quarter at
    /// most, never past the next length's state, and the next symbol
    /// outweighs the largest PE reduction (12 us) when `a` restarts at 1. A
    /// binary search over `duration_micros` is then exact at every boundary;
    /// for a shorter symbol its result still fits but need not be largest.
    pub fn maximum_apep_bytes(self, micros: u32) -> Option<NonZeroU16> {
        let fits = |bytes: u16| self.duration_micros(bytes) <= micros;
        if !fits(1) {
            return None;
        }
        if fits(u16::MAX) {
            return NonZeroU16::new(u16::MAX);
        }
        // Invariant: `low` fits, `high` does not, so `low < high`.
        let (mut low, mut high) = (1_u16, u16::MAX);
        while high.wrapping_sub(low) > 1 {
            let middle = low.wrapping_add(high.wrapping_sub(low) / 2);
            if fits(middle) {
                low = middle;
            } else {
                high = middle;
            }
        }
        NonZeroU16::new(low)
    }
}

/// Whether the LDPC encoding of `payload` bits into `available` coded bits
/// needs an extra symbol segment.
///
/// SOURCE: IEEE Std 802.11-2020, 19.3.11.7.5 steps b) to d) and its LDPC
/// PPDU encoding parameters table (`N_CW`, `L_LDPC`), applied to HE by the
/// pre-FEC padding process of IEEE Std 802.11ax-2021, clause 27. Every `(1 - R)` and `R` term is scaled by the
/// rate's denominator, so the comparisons are exact integers.
#[deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable
)]
fn ldpc_extra_symbol_segment(payload: u64, available: u64, rate: LdpcCodeRate) -> bool {
    let (numerator, denominator) = rate.fraction();
    let den = denominator.get();
    // (1 - R) x den.
    let parity = den.saturating_sub(numerator);
    // available >= payload + threshold x (1 - R), scaled by den.
    let longer = |threshold: u64| {
        available.saturating_mul(den)
            >= payload
                .saturating_mul(den)
                .saturating_add(threshold.saturating_mul(parity))
    };
    let (codewords, length) = if available <= 648 {
        (1, if longer(912) { 1296 } else { 648 })
    } else if available <= 1296 {
        (1, if longer(1464) { 1944 } else { 1296 })
    } else if available <= 1944 {
        (1, 1944)
    } else if available <= 2592 {
        (2, if longer(2916) { 1944 } else { 1296 })
    } else {
        // ceil(N_pld / (1944 x R)); 1944 x R x den is nonzero.
        let information = at_least_one(numerator.saturating_mul(1944));
        (ceil_div(payload.saturating_mul(den), information), 1944)
    };
    let block = codewords.saturating_mul(length);
    // N_CW x L_LDPC x R is an integer for every L_LDPC and R.
    let shortened = (block.saturating_mul(numerator) / denominator).saturating_sub(payload);
    let punctured = block.saturating_sub(available).saturating_sub(shortened);
    // N_CW x L_LDPC x (1 - R), scaled by den.
    let block_parity = block.saturating_mul(parity);
    let scaled_punctured = punctured.saturating_mul(den).saturating_mul(10);
    // N_punc > 0.1 x N_CW x L_LDPC x (1 - R) and
    // N_shrt < 1.2 x N_punc x R / (1 - R), or
    // N_punc > 0.3 x N_CW x L_LDPC x (1 - R).
    (scaled_punctured > block_parity
        && shortened.saturating_mul(parity).saturating_mul(10)
            < punctured.saturating_mul(numerator).saturating_mul(12))
        || scaled_punctured > block_parity.saturating_mul(3)
}

#[cfg(test)]
mod tests;
