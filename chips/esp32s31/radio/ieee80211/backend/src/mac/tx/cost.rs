//! Mapping the published S31 PHY format to portable timing facts.
use super::{HeFecCoding, HeRate, HtChannelWidth, HtGuardInterval, HtRate, TxPhyRate};
use crate::mac::rx::HeGuardIntervalAndLtf;
use crate::mac::tx_cost::{
    HeNominalPacketPadding, HeSuCoding, HeSuTiming, LdpcCodeRate, PpduTiming, TxCost,
    TxProtectionTiming, TxResponse,
};
use core::num::{NonZeroU16, NonZeroU32};
use oer_chips_esp32s31_hal::types::MacLegacyRate;

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
    /// S31 operates at 2.4 GHz: ERP/HT/HE include the 6-us signal extension.
    /// HT uses mixed format, one spatial stream, BCC, no STBC. HE is the SU
    /// format the queue formatter programs; see [`HeRate::su_timing`].
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
                let bits = *match rate.channel_width {
                    HtChannelWidth::Mhz20 => [26, 52, 78, 104, 156, 208, 234, 260],
                    HtChannelWidth::Mhz40 => [54, 108, 162, 216, 324, 432, 486, 540],
                }
                .get(rate.mcs.index() as usize)?;
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
            Self::He(rate) => rate.su_timing().map(PpduTiming::HeSu),
        }
    }
}

impl HeRate {
    /// HE SU timing of the PPDU the S31 queue formatter programs: HE20, one
    /// spatial stream, 242-tone RU, no STBC, no midamble, BCC or LDPC as
    /// [`Self::fec_coding`] selects, every GI/LTF of the type. Its length
    /// argument is the programmed APEP_LENGTH (the S-MPDU's delimited
    /// length, or the aggregate length). A DCM rate is `None`: its geometry
    /// is not modelled.
    ///
    /// The packet extension is an upper bound. `T_PE` follows the receiver's
    /// nominal packet padding, which the hardware takes from the HE packet
    /// padding register that only `program_he20_peer` writes, and no
    /// backend path calls it; the hardware's value is unknown. A 16-us
    /// nominal padding, the largest an HE PPDU admits, gives `T_PE = 4a` us,
    /// which bounds every smaller nominal padding at each `a`, so the
    /// duration overstates by at most 16 us and the byte limit errs low.
    ///
    /// SOURCE: `program_he_mac_tx_ppdu` publishes Bandwidth 20 MHz,
    /// `OneStreamNoMidamble`, this rate's GI/LTF and its FEC coding bit.
    pub fn su_timing(self) -> Option<HeSuTiming> {
        if self.is_dcm() {
            return None;
        }
        let mcs = self.mcs();
        // SOURCE: IEEE Std 802.11ax-2021, clause 27 HE-MCS table for a
        // 242-tone RU and N_SS = 1: N_SD = 234, N_CBPS = 234 x N_BPSCS and
        // N_DBPS = N_CBPS x R. Its N_SD,short table of the pre-FEC padding
        // process (27.3.12) gives 60 for a 242-tone RU: N_CBPS,short =
        // 60 x N_BPSCS and N_DBPS,short = N_CBPS,short x R. Written from these
        // definitions; table numbers are not cited because the standard's
        // text was not consulted for this change.
        let coded: u16 = mcs.entry(&[234, 468, 468, 936, 936, 1404, 1404, 1404, 1872, 1872]);
        let data: u16 = mcs.entry(&[117, 234, 351, 468, 702, 936, 1053, 1170, 1404, 1560]);
        let short_coded: u16 = mcs.entry(&[60, 120, 120, 240, 240, 360, 360, 360, 480, 480]);
        let short_data: u16 = mcs.entry(&[30, 60, 90, 120, 180, 240, 270, 300, 360, 400]);
        let code_rate = mcs.entry(&[
            LdpcCodeRate::Half,
            LdpcCodeRate::Half,
            LdpcCodeRate::ThreeQuarters,
            LdpcCodeRate::Half,
            LdpcCodeRate::ThreeQuarters,
            LdpcCodeRate::TwoThirds,
            LdpcCodeRate::ThreeQuarters,
            LdpcCodeRate::FiveSixths,
            LdpcCodeRate::ThreeQuarters,
            LdpcCodeRate::FiveSixths,
        ]);
        // SOURCE: IEEE Std 802.11ax-2021, clause 27 timing-related
        // parameters and the HE SU TXTIME of 27.4.3: before the data field,
        // L-STF 8 + L-LTF 8 + L-SIG 4 + RL-SIG 4 + HE-SIG-A 8 + HE-STF 4 us
        // (non-TB) = 36 us, then N_HE-LTF = 1 HE-LTF symbol (one space-time
        // stream) of 3.2, 6.4 or 12.8 us for 1x, 2x or 4x plus the GI.
        // T_SYM = 12.8 us + GI.
        let (preamble_nanos, symbol_nanos) = match self.guard_interval_and_ltf() {
            // 36 + 3.2 + 0.8 us; 12.8 + 0.8 us.
            HeGuardIntervalAndLtf::OneLtf800Ns => (40_000, 13_600),
            // 36 + 6.4 + 0.8 us; 12.8 + 0.8 us.
            HeGuardIntervalAndLtf::TwoLtf800Ns => (43_200, 13_600),
            // 36 + 6.4 + 1.6 us; 12.8 + 1.6 us.
            HeGuardIntervalAndLtf::TwoLtf1600Ns => (44_000, 14_400),
            // 36 + 12.8 + 3.2 us; 12.8 + 3.2 us.
            HeGuardIntervalAndLtf::FourLtf3200Ns => (52_000, 16_000),
        };
        Some(HeSuTiming {
            data_bits_per_symbol: NonZeroU16::new(data)?,
            coded_bits_per_symbol: NonZeroU16::new(coded)?,
            short_data_bits_per_symbol: NonZeroU16::new(short_data)?,
            short_coded_bits_per_symbol: NonZeroU16::new(short_coded)?,
            coding: match self.fec_coding() {
                HeFecCoding::Bcc => HeSuCoding::Bcc,
                HeFecCoding::Ldpc => HeSuCoding::Ldpc(code_rate),
            },
            symbol_nanos: NonZeroU16::new(symbol_nanos)?,
            preamble_nanos,
            nominal_packet_padding: HeNominalPacketPadding::Micros16,
            signal_extension_micros: 6,
        })
    }
}

#[cfg(test)]
mod tests;
