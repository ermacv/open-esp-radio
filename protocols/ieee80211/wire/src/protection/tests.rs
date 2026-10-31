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

fn erp_information(elements: &[u8]) -> Result<std::vec::Vec<u8>, ErpElementsError> {
    ErpElements::parse(elements).map(|erp| erp.information().collect())
}

#[test]
fn erp_elements_keep_every_value_in_frame_order() {
    assert_eq!(erp_information(&[]), Ok(std::vec![]));
    assert_eq!(
        erp_information(&[42, 1, 0x02, 42, 1, 0x00]),
        Ok(std::vec![0x02, 0x00])
    );
    assert_eq!(
        erp_information(&[42, 1, 0x00, 42, 1, 0x02]),
        Ok(std::vec![0x00, 0x02])
    );
    // Other elements stay opaque, even when their bodies look like ERP.
    assert_eq!(
        erp_information(&[0, 3, 42, 1, 0x02, 42, 1, 0x04, 221, 0]),
        Ok(std::vec![0x04])
    );
    // Many repeats: every one is kept.
    let many: std::vec::Vec<u8> = (0..100).flat_map(|i| [42, 1, i]).collect();
    assert_eq!(
        erp_information(&many).unwrap(),
        (0..100).collect::<std::vec::Vec<u8>>()
    );
}

#[test]
fn damaged_elements_are_refused_when_the_view_is_made() {
    // An ERP element of another length is no ERP Information.
    assert_eq!(
        erp_information(&[42, 2, 0x02, 0x02]),
        Err(ErpElementsError::ErpLength)
    );
    assert_eq!(
        erp_information(&[42, 0, 42, 1, 0x02]),
        Err(ErpElementsError::ErpLength)
    );
    // A Length beyond the elements, or a lone trailing octet.
    assert_eq!(
        erp_information(&[42, 1, 0x02, 0, 9, 1]),
        Err(ErpElementsError::MalformedElements)
    );
    assert_eq!(
        erp_information(&[42, 1, 0x02, 0]),
        Err(ErpElementsError::MalformedElements)
    );
}
