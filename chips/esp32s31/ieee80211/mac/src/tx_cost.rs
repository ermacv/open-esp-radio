//! Predicted transmission cost; never a measurement of channel occupancy.
//!
//! A PPDU contains PHY overhead once, even for an A-MPDU. Its PSDU input must
//! already include FCS, crypto expansion, delimiters and aggregate padding.
//! CCA freezes, internal hardware retries and completion latency are separate
//! observations. Unknown components prevent a complete service-cost estimate.

use core::num::{NonZeroU16, NonZeroU32, NonZeroU64};

/// PHY timing facts for reviewed DSSS/CCK and BCC OFDM formats. This does not
/// approximate LDPC, HE packet extension or unsupported coding as ordinary HT.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PpduTiming {
    Dsss {
        bitrate_kbps: NonZeroU32,
        preamble_micros: u16,
    },
    BccOfdm {
        data_bits_per_symbol: NonZeroU16,
        symbol_nanos: NonZeroU16,
        preamble_micros: u16,
        tail_bits: u8,
        signal_extension_micros: u8,
    },
}

impl PpduTiming {
    /// Largest nonempty PSDU whose rounded PPDU fits the supplied duration.
    /// Includes PHY overhead, but no response or contention. This is a model
    /// bound, not a hardware TXOP grant. The result is capped at the u16 PSDU
    /// geometry supported by this model; all intermediate arithmetic is u64.
    pub fn maximum_psdu_bytes(self, micros: u32) -> Option<NonZeroU16> {
        let bytes = match self {
            Self::Dsss {
                bitrate_kbps,
                preamble_micros,
            } => {
                let payload_micros = micros.checked_sub(u32::from(preamble_micros))?;
                u64::from(payload_micros).checked_mul(u64::from(bitrate_kbps.get()))? / 8_000
            }
            Self::BccOfdm {
                data_bits_per_symbol,
                symbol_nanos,
                preamble_micros,
                tail_bits,
                signal_extension_micros,
            } => {
                let payload_micros = micros.checked_sub(
                    u32::from(preamble_micros).checked_add(u32::from(signal_extension_micros))?,
                )?;
                let symbols =
                    u64::from(payload_micros).checked_mul(1_000)? / NonZeroU64::from(symbol_nanos);
                let bits = symbols.checked_mul(u64::from(data_bits_per_symbol.get()))?;
                bits.checked_sub(u64::from(tail_bits).checked_add(16)?)? / 8
            }
        };
        NonZeroU16::new(bytes.min(u64::from(u16::MAX)) as u16)
    }

    /// Rounded up only after calculating the complete PPDU, using exact
    /// bits/symbol rather than a rounded nominal bitrate. SERVICE is 16 bits.
    pub fn duration_micros(self, psdu_bytes: u16) -> u32 {
        match self {
            Self::Dsss {
                bitrate_kbps,
                preamble_micros,
            } => {
                // A u16 PSDU contributes at most 524,280,000 microseconds,
                // and the u16 preamble still fits u32. Widen before rounding.
                u32::from(preamble_micros).saturating_add(ceil_div(
                    u64::from(psdu_bytes).saturating_mul(8_000),
                    NonZeroU64::from(bitrate_kbps),
                ) as u32)
            }
            Self::BccOfdm {
                data_bits_per_symbol,
                symbol_nanos,
                preamble_micros,
                tail_bits,
                signal_extension_micros,
            } => {
                // At most 524,551 bits and 65,535 ns per symbol: the
                // rounded payload is below 34,377,000 us, even at one bit/symbol.
                // These widened operations and the final u32 conversion are exact.
                let bits = (u64::from(psdu_bytes) << 3)
                    .saturating_add(16)
                    .saturating_add(u64::from(tail_bits));
                let symbols = ceil_div(bits, NonZeroU64::from(data_bits_per_symbol));
                let nanos = symbols.saturating_mul(u64::from(symbol_nanos.get()));
                u32::from(preamble_micros)
                    .saturating_add(u32::from(signal_extension_micros))
                    .saturating_add(ceil_div(
                        nanos,
                        const {
                            match NonZeroU64::new(1_000) {
                                Some(value) => value,
                                None => panic!("nonzero nanos per microsecond"),
                            }
                        },
                    ) as u32)
            }
        }
    }
}

// Nonzero division is total; rounding can add one only when the quotient
// is strictly smaller than the numerator, so saturation is never reached.
fn ceil_div(numerator: u64, denominator: NonZeroU64) -> u64 {
    (numerator / denominator).saturating_add(u64::from(numerator % denominator != 0))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxResponse {
    None,
    Ack(PpduTiming),
    /// Explicit wire length including FCS, e.g. 32 for compressed BlockAck.
    BlockAck {
        timing: PpduTiming,
        psdu_bytes: NonZeroU16,
    },
    /// Explicit failed-response waiting budget; not proof of on-air occupancy.
    Timeout {
        micros: u32,
    },
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxProtectionTiming {
    None,
    CtsToSelf(PpduTiming),
    RtsCts { rts: PpduTiming, cts: PpduTiming },
    Unknown,
}

/// Selected backoff slots, not the contention-window exponent or maximum.
/// Excludes the unbounded time for which CCA can freeze the countdown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxAccess {
    pub contention: TxContention,
    pub slot_micros: u16,
}

/// Programmed access parameters, independent of BSS slot duration.
/// A zero backoff is a known selection, not missing evidence. These counts
/// do not describe how long the MAC actually waited or whether it contended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxContention {
    pub aifsn: u8,
    pub backoff_slots: u16,
}

/// Cost of one planned publication. Retry policy sums each attempt at its own
/// rate/length; it must not multiply a final rate by the number of attempts.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxCost {
    pub ppdu_micros: Option<u32>,
    pub response_micros: Option<u32>,
    pub protection_micros: Option<u32>,
    pub access_micros: Option<u32>,
}

impl TxCost {
    /// Convert a modelled exchange budget to a nonempty PSDU byte ceiling.
    /// The caller supplies its expected response/protection; they need not be
    /// measurements, but unknown facts cannot produce a grant. Excludes
    /// contention, CCA freezes and CPU residence, like `exchange_micros`.
    /// Recompute when the rate or assumptions change. This bounds one
    /// publication, not the sum of retries or a hardware-owned TXOP.
    pub fn maximum_psdu_bytes(
        exchange_micros: u32,
        phy: Option<PpduTiming>,
        sifs_micros: u16,
        response: TxResponse,
        protection: TxProtectionTiming,
    ) -> Option<NonZeroU16> {
        let overhead = Self::estimate(0, None, sifs_micros, response, protection, None);
        let ppdu_budget = exchange_micros
            .checked_sub(overhead.response_micros?)?
            .checked_sub(overhead.protection_micros?)?;
        phy?.maximum_psdu_bytes(ppdu_budget)
    }

    pub fn estimate(
        psdu_bytes: u16,
        phy: Option<PpduTiming>,
        sifs_micros: u16,
        response: TxResponse,
        protection: TxProtectionTiming,
        access: Option<TxAccess>,
    ) -> Self {
        let sifs = u32::from(sifs_micros);
        Self {
            ppdu_micros: phy.map(|phy| phy.duration_micros(psdu_bytes)),
            response_micros: match response {
                TxResponse::None => Some(0),
                TxResponse::Ack(phy) => sifs.checked_add(phy.duration_micros(14)),
                TxResponse::BlockAck { timing, psdu_bytes } => {
                    sifs.checked_add(timing.duration_micros(psdu_bytes.get()))
                }
                TxResponse::Timeout { micros } => Some(micros),
                TxResponse::Unknown => None,
            },
            protection_micros: match protection {
                TxProtectionTiming::None => Some(0),
                TxProtectionTiming::CtsToSelf(phy) => phy.duration_micros(14).checked_add(sifs),
                TxProtectionTiming::RtsCts { rts, cts } => rts
                    .duration_micros(20)
                    .checked_add(cts.duration_micros(14))
                    .and_then(|duration| duration.checked_add(sifs.checked_mul(2)?)),
                TxProtectionTiming::Unknown => None,
            },
            access_micros: access.and_then(|access| {
                u32::from(access.contention.aifsn)
                    .checked_add(u32::from(access.contention.backoff_slots))?
                    .checked_mul(u32::from(access.slot_micros))?
                    .checked_add(sifs)
            }),
        }
    }

    /// Exchange estimate excluding contention. A timeout is a waiting budget,
    /// so this value must never be presented as measured airtime.
    pub fn exchange_micros(self) -> Option<u32> {
        self.ppdu_micros?
            .checked_add(self.response_micros?)?
            .checked_add(self.protection_micros?)
    }

    /// Includes selected AIFS/backoff, still excluding CCA freezes and host CPU.
    pub fn service_micros(self) -> Option<u32> {
        self.exchange_micros()?.checked_add(self.access_micros?)
    }
}

#[cfg(test)]
mod tests;
