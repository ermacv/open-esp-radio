//! The A-MPDU limits of the ESP32-S31 MAC: the longest aggregate at each
//! rate, the table of them and the protection spacing of a recipient's
//! density.

use crate::mac::tx::{
    HtAmpduDensity, HtChannelWidth, HtGuardInterval, HtMcs, HtProtectionSpacing, HtRate,
};
use oer_protocols_ieee80211_contracts_port::{AmpduCapabilities, AmpduLengthTable, PhyFormatSet};

/// The longest aggregate the core publishes at any rate: the 16 bits of the
/// HT-SIG HT Length field (IEEE Std 802.11-2020, 19.3.9.4.3). Every rate's
/// length ([`esp32s31_ampdu_length`]) lies within it, and the aggregate
/// owner is configured to it; `AmpduBudget` holds each aggregate to its
/// rate's length.
pub const ESP32S31_AMPDU_MAX_LENGTH: u16 = u16::MAX;

/// The longest aggregate the core publishes at `rate`: at an HT rate the
/// vendor's per-rate ceiling (`rx11NRate2AMPDULimit`, see
/// [`HtRate::vendor_ampdu_byte_limit`]), or [`ESP32S31_AMPDU_MAX_LENGTH`]
/// where the vendor sets none; at an HE rate the ROM's APEP of the rate
/// and guard interval at the default TXOP
/// ([`HeRate::checked_maximum_apep_bytes`](crate::mac::tx::HeRate::checked_maximum_apep_bytes)),
/// within [`ESP32S31_AMPDU_MAX_LENGTH`]: what `AmpduBudget` takes when
/// the access category has no TXOP limit, while a TXOP limit in force
/// bounds it further; `None` at a legacy rate.
///
/// SOURCE(esp32s31): `libpp[trc.o]::rcSetTxAmpduLimit` stores the per-rate
/// ceiling (`rcGetAmpduSched+0x90`, `rx11NRate2AMPDULimit`) beside the
/// peer's length at the controller's `+0x80` and `+0x7e`, which only the HT
/// check `libpp[pp.o]::ppCheckTxAMPDUlength` (`+0x38`, `+0x4e`) reads. The HE
/// check `libpp[pp.o]::ppCheckTxHEAMPDUlength` bounds an aggregate by the
/// BlockAck window (`+0x6c`) and by `libpp[trc.o]::rx11AXRate2AMPDULimit`
/// (`+0x154`), the APEP of the rate within its access category's TXOP
/// (`g_he_max_apep_length_tab`, filled by
/// `rx11AXRate2AMPDULimit_update` from `he_max_apep_length` or the TXOP).
pub const fn esp32s31_ampdu_length(rate: crate::mac::tx::TxPhyRate) -> Option<u16> {
    use crate::mac::tx::TxPhyRate;
    match rate {
        TxPhyRate::Ht(rate) => Some(match rate.vendor_ampdu_byte_limit() {
            Some(limit) => limit,
            None => ESP32S31_AMPDU_MAX_LENGTH,
        }),
        TxPhyRate::He(rate) => {
            match rate.checked_maximum_apep_bytes(crate::mac::tx::HeEdcaTxopLimit::DEFAULT) {
                Some(apep) if apep < ESP32S31_AMPDU_MAX_LENGTH as u32 => Some(apep as u16),
                Some(_) => Some(ESP32S31_AMPDU_MAX_LENGTH),
                None => None,
            }
        }
        TxPhyRate::Legacy(_) => None,
    }
}

/// [`esp32s31_ampdu_length`] at every portable rate the transmit formatter
/// sends (HT MCS 0 to 7 at 20 and 40 MHz with either guard interval, HE20
/// SU MCS 0 to 9 with every guard interval); zero at every other rate. An
/// HE entry holds the rate without DCM, which halves the APEP: the rates
/// the port is given carry none, and a DCM aggregate the plan sized above
/// its APEP is refused with its owners returned.
pub static ESP32S31_AMPDU_LENGTHS: AmpduLengthTable = esp32s31_ampdu_lengths();

const fn esp32s31_ampdu_lengths() -> AmpduLengthTable {
    use crate::mac::tx::TxPhyRate;
    // The vendor's selector takes the rate code alone: both widths share
    // one length.
    const fn ht(mcs: u8, guard: HtGuardInterval) -> u32 {
        let Some(mcs) = HtMcs::from_index(mcs) else {
            panic!("HT MCS 0 to 7");
        };
        match esp32s31_ampdu_length(TxPhyRate::Ht(HtRate::new(
            mcs,
            guard,
            HtChannelWidth::Mhz20,
        ))) {
            Some(length) => length as u32,
            None => 0,
        }
    }
    let mut table = AmpduLengthTable::uniform(0);
    let mut mcs = 0;
    while mcs <= HtMcs::Mcs7.index() {
        let long = ht(mcs, HtGuardInterval::Long800Ns);
        let short = ht(mcs, HtGuardInterval::Short400Ns);
        let index = mcs as usize;
        table.ht[0][0][index] = long;
        table.ht[1][0][index] = long;
        table.ht[0][1][index] = short;
        table.ht[1][1][index] = short;
        mcs += 1;
    }
    // The table's guard intervals: 0.8 µs (with either HE-LTF size, whose
    // APEP is one), 1.6 µs and 3.2 µs.
    const fn he(mcs: u8, guard: crate::mac::rx::HeGuardIntervalAndLtf) -> u32 {
        let Some(mcs) = crate::mac::tx::HeMcs::from_index(mcs) else {
            panic!("HE MCS 0 to 9");
        };
        match esp32s31_ampdu_length(TxPhyRate::He(crate::mac::tx::HeRate::new(mcs, guard))) {
            Some(length) => length as u32,
            None => 0,
        }
    }
    use crate::mac::rx::HeGuardIntervalAndLtf as Gi;
    let mut mcs = 0;
    while mcs < 10 {
        let index = mcs as usize;
        let one = he(mcs, Gi::OneLtf800Ns);
        let two = he(mcs, Gi::TwoLtf800Ns);
        table.he[0][0][index] = if one < two { one } else { two };
        table.he[0][1][index] = he(mcs, Gi::TwoLtf1600Ns);
        table.he[0][2][index] = he(mcs, Gi::FourLtf3200Ns);
        mcs += 1;
    }
    table
}

/// The limits of the core's aggregates of up to `slots` subframes.
pub const fn esp32s31_ampdu_capabilities(slots: usize) -> AmpduCapabilities {
    AmpduCapabilities {
        max_subframes: if slots > u16::MAX as usize {
            u16::MAX
        } else {
            slots as u16
        },
        formats: PhyFormatSet::HT.union(PhyFormatSet::HE),
        max_length: &ESP32S31_AMPDU_LENGTHS,
    }
}

/// The owner's protection spacing for a recipient's Minimum MPDU Start
/// Spacing encoding (IEEE 0-7, bits 4:2 of its A-MPDU Parameters).
pub const fn protection_spacing(min_mpdu_start_spacing: u8) -> HtProtectionSpacing {
    HtProtectionSpacing::from_density(HtAmpduDensity::from_spacing_code(min_mpdu_start_spacing))
}
