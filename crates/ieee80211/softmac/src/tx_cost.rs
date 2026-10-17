//! Predicted transmission cost; never a measurement of channel occupancy.
//!
//! A PPDU contains PHY overhead once, even for an A-MPDU. Its PSDU input must
//! already include FCS, crypto expansion, delimiters and aggregate padding.
//! CCA freezes, internal hardware retries and completion latency are separate
//! observations. Unknown components prevent a complete service-cost estimate.

use core::num::{NonZeroU16, NonZeroU32};

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
                u64::from(payload_micros) * u64::from(bitrate_kbps.get()) / 8_000
            }
            Self::BccOfdm {
                data_bits_per_symbol,
                symbol_nanos,
                preamble_micros,
                tail_bits,
                signal_extension_micros,
            } => {
                let payload_micros = micros
                    .checked_sub(u32::from(preamble_micros) + u32::from(signal_extension_micros))?;
                let symbols = u64::from(payload_micros) * 1_000 / u64::from(symbol_nanos.get());
                let bits = symbols * u64::from(data_bits_per_symbol.get());
                bits.checked_sub(16 + u64::from(tail_bits))? / 8
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
                u32::from(preamble_micros)
                    + (u32::from(psdu_bytes) * 8_000).div_ceil(bitrate_kbps.get())
            }
            Self::BccOfdm {
                data_bits_per_symbol,
                symbol_nanos,
                preamble_micros,
                tail_bits,
                signal_extension_micros,
            } => {
                let bits = 16 + u32::from(psdu_bytes) * 8 + u32::from(tail_bits);
                let symbols = bits.div_ceil(u32::from(data_bits_per_symbol.get()));
                let nanos = u64::from(symbols) * u64::from(symbol_nanos.get());
                u32::from(preamble_micros)
                    + u32::from(signal_extension_micros)
                    + u32::try_from(nanos.div_ceil(1_000))
                        .expect("bounded PSDU and symbol duration fit u32")
            }
        }
    }
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
pub enum TxProtection {
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
        protection: TxProtection,
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
        protection: TxProtection,
        access: Option<TxAccess>,
    ) -> Self {
        let sifs = u32::from(sifs_micros);
        Self {
            ppdu_micros: phy.map(|phy| phy.duration_micros(psdu_bytes)),
            response_micros: match response {
                TxResponse::None => Some(0),
                TxResponse::Ack(phy) => Some(sifs + phy.duration_micros(14)),
                TxResponse::BlockAck { timing, psdu_bytes } => {
                    Some(sifs + timing.duration_micros(psdu_bytes.get()))
                }
                TxResponse::Timeout { micros } => Some(micros),
                TxResponse::Unknown => None,
            },
            protection_micros: match protection {
                TxProtection::None => Some(0),
                TxProtection::CtsToSelf(phy) => Some(phy.duration_micros(14) + sifs),
                TxProtection::RtsCts { rts, cts } => {
                    Some(rts.duration_micros(20) + cts.duration_micros(14) + 2 * sifs)
                }
                TxProtection::Unknown => None,
            },
            access_micros: access.and_then(|access| {
                (u32::from(access.contention.aifsn) + u32::from(access.contention.backoff_slots))
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
