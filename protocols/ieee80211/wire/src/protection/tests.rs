use super::*;

#[test]
fn erp_information_round_trips_protection_and_membership_bits() {
    let erp = ErpProtection::from_information(Some(0x07));
    assert!(erp.use_protection());
    assert!(erp.long_preamble_required());
    assert_eq!(erp.information(false), 0x06);
    assert_eq!(ErpProtection::new(true, false).information(true), 0x03);
    assert_eq!(ErpProtection::from_information(None), ErpProtection::NONE);
}

#[test]
fn ht_operation_protection_encodes_mode_and_non_greenfield_bits() {
    for mode in [
        HtProtectionMode::None,
        HtProtectionMode::Nonmember,
        HtProtectionMode::TwentyMhz,
        HtProtectionMode::NonHtMixed,
    ] {
        assert_eq!(HtProtectionMode::from_field(mode.field()), mode);
    }
    let protection = HtOperationProtection {
        mode: HtProtectionMode::TwentyMhz,
        non_greenfield_present: true,
    };
    assert_eq!(protection.information_byte(), 0x06);
    let mut element = [0_u8; 24];
    element[..2].copy_from_slice(&[61, 22]);
    element[4] = protection.information_byte();
    assert_eq!(
        HtProtectionMode::from_operation_ie(Some(&element)),
        HtProtectionMode::TwentyMhz
    );
}
