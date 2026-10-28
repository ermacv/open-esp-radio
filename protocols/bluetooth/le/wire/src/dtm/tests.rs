use super::*;
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[test]
fn prbs_patterns_match_the_vendor_table_fingerprints() {
    let mut payload = [0; 255];
    DtmPayloadPattern::Prbs9.fill(&mut payload);
    assert_eq!(
        payload[..16],
        [
            0xff, 0xc1, 0xfb, 0xe8, 0x4c, 0x90, 0x72, 0x8b, 0xe7, 0xb3, 0x51, 0x89, 0x63, 0xab,
            0x23, 0x23
        ]
    );
    assert_eq!(fnv1a64(&payload), 0x94db_648c_b178_dce3);
    DtmPayloadPattern::Prbs15.fill(&mut payload);
    assert_eq!(fnv1a64(&payload), 0x4655_41b8_492c_b9ba);
    for selector in 0..8 {
        let pattern = DtmPayloadPattern::from_hci_selector(selector).unwrap();
        assert_eq!(pattern.payload_type(), selector);
    }
    assert_eq!(DtmPayloadPattern::from_hci_selector(8), None);
}
