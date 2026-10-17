use super::*;

fn rsn(pairwise: u8, akm: u8, capabilities: u16) -> [u8; 22] {
    let mut ie = [0_u8; 22];
    ie[0] = 0x30;
    ie[1] = 20;
    ie[2..4].copy_from_slice(&1_u16.to_le_bytes());
    ie[4..8].copy_from_slice(&[0x00, 0x0f, 0xac, 4]);
    ie[8..10].copy_from_slice(&1_u16.to_le_bytes());
    ie[10..14].copy_from_slice(&[0x00, 0x0f, 0xac, pairwise]);
    ie[14..16].copy_from_slice(&1_u16.to_le_bytes());
    ie[16..20].copy_from_slice(&[0x00, 0x0f, 0xac, akm]);
    ie[20..22].copy_from_slice(&capabilities.to_le_bytes());
    ie
}

#[test]
fn accepts_wpa2_psk_ccmp_and_optional_mfpc() {
    let ie = rsn(4, 2, 1 << 7);
    let validated = validate_rsn_element(&ie).unwrap();
    assert_eq!(validated.owned().as_bytes(), &ie);
    assert_eq!(validated.capabilities(), 1 << 7);
    assert_eq!(validated.akm(), Akm::Psk);
}

#[test]
fn selects_the_first_supported_suite_of_a_transition_akm_list() {
    // SAE (8) then PSK (2) from a capable access point: the first listed
    // supported suite.
    let mut ie = [0_u8; 26];
    ie[0] = 0x30;
    ie[1] = 24;
    ie[2..4].copy_from_slice(&1_u16.to_le_bytes());
    ie[4..8].copy_from_slice(&[0x00, 0x0f, 0xac, 4]);
    ie[8..10].copy_from_slice(&1_u16.to_le_bytes());
    ie[10..14].copy_from_slice(&[0x00, 0x0f, 0xac, 4]);
    ie[14..16].copy_from_slice(&2_u16.to_le_bytes());
    ie[16..20].copy_from_slice(&[0x00, 0x0f, 0xac, 8]);
    ie[20..24].copy_from_slice(&[0x00, 0x0f, 0xac, 2]);
    ie[24..26].copy_from_slice(&(1_u16 << 7).to_le_bytes());
    assert_eq!(validate_rsn_element(&ie).unwrap().akm(), Akm::Sae);
    // SAE needs management frame protection.
    ie[24] = 0;
    assert_eq!(validate_rsn_element(&ie).unwrap().akm(), Akm::Psk);
    // FT-SAE (9) is skipped for PSK.
    ie[24] = 1 << 7;
    ie[19] = 9;
    assert_eq!(validate_rsn_element(&ie).unwrap().akm(), Akm::Psk);
}

#[test]
fn rejects_non_ccmp_non_psk_and_required_pmf() {
    assert_eq!(
        validate_rsn_element(&rsn(2, 2, 0)),
        Err(RsnElementError::UnsupportedPairwiseCipher)
    );
    // SAE without management frame protection.
    assert_eq!(
        validate_rsn_element(&rsn(4, 8, 0)),
        Err(RsnElementError::UnsupportedAkm)
    );
    assert_eq!(
        validate_rsn_element(&rsn(4, 8, 1 << 7)).map(|element| element.akm()),
        Ok(Akm::Sae)
    );
    // MFPR without MFPC.
    assert_eq!(
        validate_rsn_element(&rsn(4, 2, 1 << 6)),
        Err(RsnElementError::Malformed)
    );
}

#[test]
fn reports_the_listed_pmkids() {
    let mut ie = [0_u8; 40];
    ie[..22].copy_from_slice(&rsn(4, 2, 0));
    ie[1] = 22;
    assert_eq!(
        validate_rsn_element(&ie[..24]).map(|element| element.pmkid_count()),
        Ok(0)
    );
    ie[1] = 38;
    ie[22..24].copy_from_slice(&1_u16.to_le_bytes());
    assert_eq!(
        validate_rsn_element(&ie[..40]).map(|element| element.pmkid_count()),
        Ok(1)
    );
}

#[test]
fn malformed_elements_are_rejected_before_policy() {
    // One PMKID announced without its 16 bytes.
    let mut truncated_pmkid = [0_u8; 24];
    truncated_pmkid[..22].copy_from_slice(&rsn(4, 2, 0));
    truncated_pmkid[1] = 22;
    truncated_pmkid[22..24].copy_from_slice(&1_u16.to_le_bytes());
    assert_eq!(
        validate_rsn_element(&truncated_pmkid),
        Err(RsnElementError::Malformed)
    );

    // Unsupported pairwise cipher inside a truncated element.
    let mut truncated = rsn(2, 2, 0);
    truncated[1] = 19;
    assert_eq!(
        validate_rsn_element(&truncated[..21]),
        Err(RsnElementError::Malformed)
    );
}

#[test]
fn reports_the_advertised_management_frame_protection() {
    let required = validate_rsn_element(&rsn(4, 6, (1 << 7) | (1 << 6))).unwrap();
    assert_eq!(required.akm(), Akm::PskSha256);
    assert_eq!(
        required.management_frame_protection(),
        ManagementFrameProtection {
            capable: true,
            required: true
        }
    );
    let none = validate_rsn_element(&rsn(4, 2, 0)).unwrap();
    assert!(!none.management_frame_protection().capable);
    assert!(
        !none
            .management_frame_protection()
            .negotiated(required.management_frame_protection())
    );
    assert!(
        required
            .management_frame_protection()
            .negotiated(required.management_frame_protection())
    );
}

#[test]
fn accepts_only_bip_cmac_128_as_group_management_cipher() {
    let mut ie = [0_u8; 28];
    ie[..22].copy_from_slice(&rsn(4, 2, 1 << 7));
    ie[1] = 26;
    ie[22..24].copy_from_slice(&0_u16.to_le_bytes());
    ie[24..28].copy_from_slice(&[0x00, 0x0f, 0xac, 6]);
    assert!(validate_rsn_element(&ie).is_ok());
    ie[27] = 11;
    assert_eq!(
        validate_rsn_element(&ie),
        Err(RsnElementError::UnsupportedGroupManagementCipher)
    );
    // A group management cipher without MFPC.
    ie[20..22].copy_from_slice(&0_u16.to_le_bytes());
    ie[27] = 6;
    assert_eq!(validate_rsn_element(&ie), Err(RsnElementError::Malformed));
}
