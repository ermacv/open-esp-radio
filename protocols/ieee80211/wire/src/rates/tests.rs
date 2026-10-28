use super::*;

#[test]
fn basic_selection_keeps_only_flagged_rates_of_both_elements() {
    let basic = LegacyRateSet::from_rate_elements(&[2, 0x84, 0x8b], &[0x8c, 0x98, 0xb0], RateSelection::Basic);
    assert_eq!(
        basic.rates().collect::<std::vec::Vec<_>>(),
        [
            LegacyRate::Dsss2M(DsssPreamble::Long),
            LegacyRate::Cck5M5(DsssPreamble::Long),
            LegacyRate::Ofdm6M,
            LegacyRate::Ofdm12M,
            LegacyRate::Ofdm24M,
        ]
    );
}

#[test]
fn all_selection_ignores_the_basic_flag_and_unknown_encodings() {
    let all = LegacyRateSet::from_rate_elements(&[0x82, 0x84, 0x96, 0xff], &[0x0c, 0x30, 0x7f], RateSelection::All);
    assert_eq!(
        all.rates().collect::<std::vec::Vec<_>>(),
        [
            LegacyRate::Dsss1M,
            LegacyRate::Dsss2M(DsssPreamble::Long),
            LegacyRate::Cck11M(DsssPreamble::Long),
            LegacyRate::Ofdm6M,
            LegacyRate::Ofdm24M,
        ]
    );
    assert!(LegacyRateSet::from_rate_elements(&[0xff], &[], RateSelection::All).is_empty());
}

#[test]
fn bits_round_trip_and_name_only_table_entries() {
    assert_eq!(LegacyRateSet::from_bits(0xffff).bits(), 0x0fff);
    assert_eq!(LegacyRateSet::from_bits(0x0015).rates().count(), 3);
    assert!(LegacyRateSet::EMPTY.is_empty());
}
