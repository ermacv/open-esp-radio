use super::require_ht40_mcs7;

#[test]
fn ht40_ceiling_vector_requires_mcs7_and_width_but_accepts_either_gi() {
    require_ht40_mcs7("STA RX/AP TX", 40, "150.0 MBit/s MCS 7 40MHz short GI").unwrap();
    require_ht40_mcs7("STA RX/AP TX", 40, "135.0 MBit/s MCS 7 40MHz").unwrap();
    assert!(require_ht40_mcs7("STA RX/AP TX", 20, "150.0 MBit/s MCS 7 40MHz short GI",).is_err());
    assert!(require_ht40_mcs7("STA RX/AP TX", 40, "135.0 MBit/s MCS 6 40MHz short GI",).is_err());
    assert!(require_ht40_mcs7("STA RX/AP TX", 40, "120.0 MBit/s MCS 7 40MHz").is_err());
}
