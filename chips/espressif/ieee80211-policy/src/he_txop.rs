//! The Espressif estimate of the HE SU APEP length that fits below a TXOP
//! duration RTS threshold.

use core::num::NonZeroU64;

use oer_ieee80211_upper_mac::protection::{HePacketPadding, HeTxopDurationRtsThreshold};
use oer_ieee80211_upper_mac::HeTxopRtsBudget;
use oer_ieee80211_wire::phy::{HeGiLtf, HeRate};

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
    // Counts of 2^-20 microseconds represent the vendor's binary32 table
    // entries exactly: 23.2f32, 13.6f32 and 14.4f32 are not decimal tenths.
    const SCALE: u64 = 1 << 20;
    const SYMBOL_800: NonZeroU64 = NonZeroU64::new(14_260_634).expect("positive symbol duration");
    const SYMBOL_1600: NonZeroU64 = NonZeroU64::new(15_099_494).expect("positive symbol duration");
    const SYMBOL_3200: NonZeroU64 = NonZeroU64::new(16 * SCALE).expect("positive symbol duration");
    let (preamble, symbol) = match rate.gi_ltf() {
        HeGiLtf::Ltf1xGi800Ns | HeGiLtf::Ltf2xGi800Ns => (24_326_964, SYMBOL_800),
        HeGiLtf::Ltf2xGi1600Ns => (24 * SCALE, SYMBOL_1600),
        HeGiLtf::Ltf4xGi3200Ns => (32 * SCALE, SYMBOL_3200),
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
    };
    // The checked threshold is 1..=1022. The prefix is an exact integer
    // 12..=32684 us; its scaled representation fits 35 bits.
    let prefix = u64::from(threshold.units_32_us())
        .wrapping_mul(32)
        .wrapping_sub(20)
        .wrapping_mul(SCALE);
    let Some(after_preamble) = prefix.checked_sub(preamble) else {
        return 0;
    };
    // Preserve the first subtraction's round-to-nearest, ties-to-even.
    // Subsequent subtractions are small whole microseconds: a positive
    // result has no greater magnitude and remains exactly representable.
    let budget = round_binary32_significand(after_preamble);
    let overhead = 16_u64
        .wrapping_add(u64::from(padding.micros()))
        .wrapping_add(block_ack_us)
        .wrapping_mul(SCALE);
    if budget <= overhead {
        return 0;
    }
    let symbols = whole_symbols(budget.wrapping_sub(overhead), symbol) as i32;
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

/// Round a positive fixed-point value to binary32's 24 significant bits.
fn round_binary32_significand(value: u64) -> u64 {
    let shift = u64::BITS
        .wrapping_sub(value.leading_zeros())
        .saturating_sub(24);
    if shift == 0 {
        return value;
    }
    // The input fits 35 bits, so shift <= 11 and every operation fits u64.
    let retained = value >> shift;
    let discarded = value & 1_u64.wrapping_shl(shift).wrapping_sub(1);
    let halfway = 1_u64.wrapping_shl(shift.wrapping_sub(1));
    let round_up = discarded > halfway || (discarded == halfway && retained & 1 != 0);
    retained.wrapping_add(u64::from(round_up)) << shift
}

/// Divide, round as binary32, then truncate, without floating point.
fn whole_symbols(numerator: u64, symbol: NonZeroU64) -> u64 {
    // The denominator is one of the positive table entries above. The
    // quotient is below 2405, so binary32 can only change its integer part
    // by rounding up to the next integer. At a power of two the spacing
    // immediately below that integer is half the spacing above it.
    let floor = numerator / symbol;
    // floor < 2405, so the addition is exact; the nonzero type also makes
    // ilog2 total, including a budget that fits no complete symbol.
    let next = NonZeroU64::MIN.saturating_add(floor);
    let midpoint_shift = 24_u32
        .wrapping_sub(next.ilog2())
        .wrapping_add(u32::from(next.is_power_of_two()));
    let below_next = symbol.get().wrapping_sub(numerator % symbol);
    // Compare the exact quotient with the midpoint below `next`. Its
    // significand is even, so an exact midpoint rounds to `next` too.
    // midpoint_shift <= 25; the shifted distance fits 49 bits.
    let round_up = below_next << midpoint_shift <= symbol.get();
    floor.wrapping_add(u64::from(round_up))
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

    use oer_ieee80211_wire::phy::{FecCoding, HeMcs, PpduBandwidth, SpatialStreams};

    use super::*;

    #[test]
    fn whole_symbol_rounding_handles_zero_and_power_of_two_midpoints() {
        let symbol = NonZeroU64::new(16 << 20).unwrap();
        assert_eq!(whole_symbols(0, symbol), 0);
        assert_eq!(whole_symbols(1, symbol), 0);
        for next in [2_u64, 4, 8, 16, 1024, 2048] {
            // Immediately below a power of two, half an f32 ULP is
            // next / 2 units of this exact binary32 symbol duration.
            let exact = next * symbol.get();
            let halfway = next / 2;
            assert_eq!(whole_symbols(exact - halfway, symbol), next);
            assert_eq!(whole_symbols(exact - halfway - 1, symbol), next - 1);
        }
    }

    // The original vendor-ordered single-precision calculation is an
    // independent oracle; production code contains no floating point.
    fn f32_reference(
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

    #[test]
    fn every_admitted_input_matches_the_original_f32_result() {
        let mut compared = 0;
        for units in 1..=1022 {
            let threshold = HeTxopDurationRtsThreshold::new(units).expect("enabled threshold");
            for gi_ltf in [
                HeGiLtf::Ltf1xGi800Ns,
                HeGiLtf::Ltf2xGi800Ns,
                HeGiLtf::Ltf2xGi1600Ns,
                HeGiLtf::Ltf4xGi3200Ns,
            ] {
                for mcs in 0..=9 {
                    for dcm in [false, true] {
                        if dcm && !matches!(mcs, 0 | 1 | 3 | 4) {
                            continue;
                        }
                        let rate = HeRate::new(
                            HeMcs::new(mcs).expect("HE20 MCS"),
                            SpatialStreams::ONE,
                            PpduBandwidth::Mhz20,
                            gi_ltf,
                            FecCoding::Bcc,
                            dcm,
                        )
                        .expect("admitted HE20 rate");
                        for padding in [
                            HePacketPadding::None,
                            HePacketPadding::Us8,
                            HePacketPadding::Us16,
                        ] {
                            assert_eq!(
                                maximum_unprotected_apep_bytes(threshold, padding, rate),
                                f32_reference(threshold, padding, rate),
                                "threshold={units}, gi_ltf={gi_ltf:?}, mcs={mcs}, dcm={dcm}, padding={padding:?}"
                            );
                            compared += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(compared, 171_696);
    }

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
