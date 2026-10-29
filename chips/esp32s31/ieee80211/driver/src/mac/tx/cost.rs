//! Mapping the published S31 PHY format to portable timing facts.
use super::{HtChannelWidth, HtGuardInterval, HtRate, TxPhyRate};
use crate::mac::tx_cost::{PpduTiming, TxCost, TxProtectionTiming, TxResponse};
use core::num::{NonZeroU16, NonZeroU32};
use oer_esp32s31_hal::types::MacLegacyRate;

impl HtRate {
    /// Byte ceiling for one unprotected HT aggregate plus compressed BlockAck.
    /// `block_ack_timing` is the caller's explicit response assumption, not the
    /// RTS-rate field or an observed ACK. S31 uses 10-us SIFS at 2.4 GHz.
    ///
    /// Intersects the model budget with peer and recovered vendor byte limits.
    /// Apply while the aggregate owner is free, using its existing byte-limit
    /// configuration. Slot count, DMA capacity, density and minimum aggregate
    /// geometry still require the ordinary builder admission checks. A result
    /// need not fit even one real MPDU. Recompute after rate/peer changes;
    /// this neither grants multi-PPDU TXOP ownership nor bounds later retries.
    pub fn ampdu_exchange_byte_limit(
        self,
        exchange_micros: u32,
        block_ack_timing: PpduTiming,
        peer_maximum_bytes: u16,
    ) -> Option<NonZeroU16> {
        let budget = TxCost::maximum_psdu_bytes(
            exchange_micros,
            TxPhyRate::Ht(self).ppdu_timing(),
            10,
            TxResponse::BlockAck {
                timing: block_ack_timing,
                psdu_bytes: const { NonZeroU16::new(32).expect("compressed BlockAck includes FCS") },
            },
            TxProtectionTiming::None,
        )?;
        NonZeroU16::new(
            budget
                .get()
                .min(peer_maximum_bytes)
                .min(self.vendor_ampdu_byte_limit().unwrap_or(u16::MAX)),
        )
    }
}

impl TxPhyRate {
    /// S31 operates at 2.4 GHz: ERP/HT include the 6-us signal extension.
    /// HT uses mixed format, one spatial stream, BCC, no STBC. HE duration
    /// stays unknown until coding/padding/packet-extension geometry is modelled.
    pub fn ppdu_timing(self) -> Option<PpduTiming> {
        match self {
            Self::Legacy(rate) => {
                let kbps = crate::mac::portable::legacy_phy_rate(rate).kbps();
                let short = matches!(
                    rate,
                    MacLegacyRate::Dsss2MShort
                        | MacLegacyRate::Cck5M5Short
                        | MacLegacyRate::Cck11MShort
                );
                if matches!(
                    rate,
                    MacLegacyRate::Dsss1MLong
                        | MacLegacyRate::Dsss2MLong
                        | MacLegacyRate::Cck5M5Long
                        | MacLegacyRate::Cck11MLong
                ) || short
                {
                    Some(PpduTiming::Dsss {
                        bitrate_kbps: NonZeroU32::new(kbps)?,
                        preamble_micros: if short { 96 } else { 192 },
                    })
                } else {
                    Some(PpduTiming::BccOfdm {
                        data_bits_per_symbol: NonZeroU16::new((kbps / 250) as u16)?,
                        symbol_nanos: NonZeroU16::new(4_000)?,
                        preamble_micros: 20,
                        tail_bits: 6,
                        signal_extension_micros: 6,
                    })
                }
            }
            Self::Ht(rate) => {
                let bits = match rate.channel_width {
                    HtChannelWidth::Mhz20 => [26, 52, 78, 104, 156, 208, 234, 260],
                    HtChannelWidth::Mhz40 => [54, 108, 162, 216, 324, 432, 486, 540],
                }[rate.mcs.index() as usize];
                Some(PpduTiming::BccOfdm {
                    data_bits_per_symbol: NonZeroU16::new(bits)?,
                    symbol_nanos: NonZeroU16::new(match rate.guard_interval {
                        HtGuardInterval::Long800Ns => 4_000,
                        HtGuardInterval::Short400Ns => 3_600,
                    })?,
                    preamble_micros: 36,
                    tail_bits: 6,
                    signal_extension_micros: 6,
                })
            }
            Self::He(_) => None,
        }
    }
}

#[cfg(test)]
mod tests;
