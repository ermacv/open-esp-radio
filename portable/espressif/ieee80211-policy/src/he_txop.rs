//! The Espressif estimate of the HE SU APEP length that fits below a TXOP
//! duration RTS threshold.

use oer_ieee80211_mac::phy::{HeGiLtf, HeRate};
use oer_ieee80211_upper_mac::{HePacketPadding, HeTxopDurationRtsThreshold, HeTxopRtsBudget};

/// The nominal packet padding code the vendor stores from the peer's HE
/// Capabilities.
///
/// SOURCE(esp32s31): complete `libnet80211[ieee80211_he.o]::ieee80211_parse_hecap`
/// stores this code at node offset `0x354`; complete
/// `libpp[if_hwctrl.o]::ic_set_he_rts_threshold_bytes_tab` subtracts 8 µs
/// for code one, 16 µs for code two and nothing for any other code.
pub const fn packet_padding_from_code(code: u8) -> HePacketPadding {
    match code {
        1 => HePacketPadding::Us8,
        2 => HePacketPadding::Us16,
        _ => HePacketPadding::None,
    }
}

/// The nominal packet padding code the vendor keeps for a peer whose HE
/// Capabilities element is `he_capability_ie` (the complete element, ID and
/// length included): zero, one or two.
///
/// SOURCE(esp32s31): complete `libnet80211[ieee80211_he.o]::ieee80211_parse_hecap`.
/// HE PHY Capabilities byte six (PPE Thresholds Present, bit seven) is
/// element byte 15 and byte nine is element byte 18. Without PPE Thresholds
/// the vendor stores Nominal Packet Padding (byte 18 bits 7:6). With PPE
/// Thresholds it stores code two only when the NSS1/RU242 PPET16 is zero and
/// PPET8 is None (element bytes 24/25); otherwise the freshly allocated node
/// keeps zero.
pub fn nominal_packet_padding_code(he_capability_ie: &[u8]) -> u8 {
    let Some(&phy_byte_six) = he_capability_ie.get(15) else {
        return 0;
    };
    if phy_byte_six & 0x80 == 0 {
        return he_capability_ie.get(18).map_or(0, |byte| byte >> 6);
    }
    let (Some(&first), Some(&second)) = (he_capability_ie.get(24), he_capability_ie.get(25)) else {
        return 0;
    };
    let ru242 = (first >> 3) & 0x01 != 0;
    let ppet16 = (first >> 7) | ((second & 0x03) << 1);
    if ru242 && ppet16 == 0 && second & 0x1c == 0x1c {
        2
    } else {
        0
    }
}

/// The packet padding the vendor applies to a peer whose HE Capabilities
/// element is `he_capability_ie`.
pub fn packet_padding(he_capability_ie: &[u8]) -> HePacketPadding {
    packet_padding_from_code(nominal_packet_padding_code(he_capability_ie))
}

/// The largest HE SU APEP length whose TXOP stays below the threshold.
///
/// SOURCE(esp32s31): complete `libpp[if_hwctrl.o]::ic_set_he_rts_threshold_bytes_tab`
/// and its `.data` tables in `libpp.a[hal_mac_ctl.o]` (`he_preamble_su`,
/// `he_time_per_sym`, `he_data_bits_per_sym`), plus complete
/// `libpp[pp_he.o]::get_estimated_batime`. The vendor evaluates, in single
/// precision, `threshold * 32 - 20 - preamble - 16 - padding`, subtracts the
/// BlockAck estimate, divides by the symbol duration, floors, multiplies by
/// the RU242 data bits per symbol, subtracts the 22 SERVICE/tail bits and
/// shifts right by three. DCM halves the data bits for MCS 0, 1, 3 and 4.
/// The halfword table stores the arithmetic result modulo 2^16; a budget no
/// larger than the BlockAck estimate stores zero. Complete
/// `libpp[if_hwctrl.o]::ic_get_he_rts_threshold_bytes` selects row one for both 0.8 µs
/// guard-interval configurations.
///
/// The vendor tables hold HE20 MCS 0-9; a rate beyond them gets a budget of
/// zero, so the PPDU is protected.
pub fn maximum_unprotected_apep_bytes(
    threshold: HeTxopDurationRtsThreshold,
    padding: HePacketPadding,
    rate: HeRate,
) -> u16 {
    let (preamble_us, symbol_us) = match rate.gi_ltf() {
        HeGiLtf::Ltf1xGi800Ns | HeGiLtf::Ltf2xGi800Ns => (23.2, 13.6),
        HeGiLtf::Ltf2xGi1600Ns => (24.0, 14.4),
        HeGiLtf::Ltf4xGi3200Ns => (32.0, 16.0),
    };
    let mcs = rate.mcs().index();
    let data_bits = match mcs {
        0 => 117,
        1 => 234,
        2 => 351,
        3 => 468,
        4 => 702,
        5 => 936,
        6 => 1053,
        7 => 1170,
        8 => 1404,
        9 => 1560,
        _ => return 0,
    };
    let block_ack_us = match mcs {
        0 => 68,
        1 | 2 => 44,
        _ => 32,
    } as f32;
    // The checked threshold is 1..=1022: this integer prefix is 12..=32684,
    // so neither operation wraps. Keep the vendor's subsequent f32 steps
    // in their original order at symbol-boundary roundings.
    let budget = i32::from(threshold.units_32_us())
        .wrapping_mul(32)
        .wrapping_sub(20) as f32
        - preamble_us
        - 16.0
        - f32::from(padding.micros());
    if block_ack_us >= budget {
        return 0;
    }
    // The quotient is positive here, so truncation equals `floor`. A time
    // budget below 32704 us and symbol duration >= 13.6 us give fewer than
    // 2405 symbols; the float-to-integer conversion is inside i32's range.
    let symbols = ((budget - block_ack_us) / symbol_us) as i32;
    let mut bits: i32 = data_bits;
    if rate.dcm() {
        bits /= 2;
    }
    // bits <= 1560 and symbols < 2405: the product and subtraction fit
    // i32, even when zero whole symbols leave -22 SERVICE/tail bits.
    // The signed shift and final u16 cast intentionally retain the vendor
    // halfword's low 16 bits. A positive sub-symbol budget produces 65533;
    // clamping that result to zero would change the recovered policy.
    (bits.wrapping_mul(symbols).wrapping_sub(22) >> 3) as u16
}

/// [`maximum_unprotected_apep_bytes`] as the portable protection policy's
/// budget.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EspressifHeTxopRtsBudget;

impl HeTxopRtsBudget for EspressifHeTxopRtsBudget {
    fn max_unprotected_apep_bytes(
        &self,
        threshold: HeTxopDurationRtsThreshold,
        padding: HePacketPadding,
        rate: HeRate,
    ) -> u16 {
        maximum_unprotected_apep_bytes(threshold, padding, rate)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_padding_follows_nominal_padding_or_the_ru242_ppe_exception() {
        use super::{HePacketPadding, nominal_packet_padding_code, packet_padding};
        let mut element = [0_u8; 26];
        element[18] = 0x40;
        assert_eq!(packet_padding(&element[..24]), HePacketPadding::Us8);
        element[18] = 0x00;
        assert_eq!(nominal_packet_padding_code(&element[..24]), 0);

        // PPE Thresholds present: RU242 selected, PPET16 zero and PPET8 None.
        element[15] |= 0x80;
        element[24] = 0x08;
        element[25] = 0x1c;
        assert_eq!(packet_padding(&element), HePacketPadding::Us16);
        element[25] = 0x00;
        assert_eq!(packet_padding(&element), HePacketPadding::None);
        assert_eq!(nominal_packet_padding_code(&element[..10]), 0);
    }

    use oer_ieee80211_mac::phy::{FecCoding, HeMcs, PpduBandwidth, SpatialStreams};

    use super::*;

    fn he(mcs: u8) -> HeRate {
        HeRate::new(
            HeMcs::new(mcs).unwrap(),
            SpatialStreams::ONE,
            PpduBandwidth::Mhz20,
            HeGiLtf::Ltf2xGi800Ns,
            FecCoding::Bcc,
            false,
        )
        .unwrap()
    }

    #[test]
    fn the_budget_matches_the_vendor_byte_table() {
        let threshold = HeTxopDurationRtsThreshold::new(64).unwrap();
        // 64 * 32 - 20 - 23.2 - 16 = 1988.8 µs; minus the 32 µs BlockAck
        // estimate is 143 whole 13.6 µs symbols of 1170 bits at MCS 7.
        assert_eq!(
            maximum_unprotected_apep_bytes(threshold, HePacketPadding::None, he(7)),
            20_911
        );
        assert!(maximum_unprotected_apep_bytes(threshold, HePacketPadding::Us16, he(7)) < 20_911);
        let short = HeTxopDurationRtsThreshold::new(2).unwrap();
        assert_eq!(
            EspressifHeTxopRtsBudget.max_unprotected_apep_bytes(
                short,
                HePacketPadding::None,
                he(0)
            ),
            0
        );
        assert_eq!(
            maximum_unprotected_apep_bytes(threshold, HePacketPadding::None, he(11)),
            0
        );
        assert_eq!(packet_padding_from_code(2), HePacketPadding::Us16);
        assert_eq!(packet_padding_from_code(3), HePacketPadding::None);
    }

    #[test]
    fn partial_symbols_keep_the_signed_halfword_result() {
        for (mcs, first_positive_threshold, next_budget) in [(0, 4, 26), (9, 3, 387)] {
            let budget = |units| {
                maximum_unprotected_apep_bytes(
                    HeTxopDurationRtsThreshold::new(units).unwrap(),
                    HePacketPadding::None,
                    he(mcs),
                )
            };
            assert_eq!(budget(first_positive_threshold - 1), 0);
            // The remaining time is positive but fits no complete symbol:
            // (-22 >> 3) modulo 2^16 is 65533.
            assert_eq!(budget(first_positive_threshold), 65533);
            assert_eq!(budget(first_positive_threshold + 1), next_budget);
        }
    }

    #[test]
    fn the_maximum_threshold_keeps_the_halfword_modulus() {
        // 2398 complete symbols at MCS 9 give (2398 * 1560 - 22) >> 3
        // = 467607 bytes, whose low halfword is 8855.
        let threshold = HeTxopDurationRtsThreshold::new(1022).unwrap();
        assert_eq!(
            maximum_unprotected_apep_bytes(threshold, HePacketPadding::None, he(9)),
            8855
        );
        assert!(HeTxopDurationRtsThreshold::new(1023).is_none());
    }
}
