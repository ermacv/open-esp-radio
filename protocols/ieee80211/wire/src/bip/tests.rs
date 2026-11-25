use super::*;

/// The highest IPN: six octets.
const HIGHEST: u64 = (1 << 48) - 1;

fn ipn(value: u64) -> oer_portable_wire::U64<48> {
    oer_portable_wire::U64::new(value).unwrap()
}

/// A group Deauthentication to the broadcast address from 02:..:01 with
/// Retry, Power Management and More Data set and a two-octet reason.
fn deauthentication() -> [u8; 26 + MANAGEMENT_MIC_ELEMENT_LEN] {
    let mut frame = [0; 26 + MANAGEMENT_MIC_ELEMENT_LEN];
    frame[..2].copy_from_slice(&[0xc0, 0x38]);
    frame[4..10].copy_from_slice(&[0xff; 6]);
    frame[10..16].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
    frame[16..22].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
    frame[24..26].copy_from_slice(&[7, 0]);
    frame
}

#[test]
fn an_appended_element_has_the_reference_layout_and_parses_back() {
    let mut frame = deauthentication();
    let protection =
        BipProtection::append(&mut frame, 26, IgtkKeyId::Four, ipn(0x0102_0304_0506)).unwrap();
    let [aad, body, element, zero] = protection.mic_input();
    let mut expected_aad = std::vec![0xc0, 0x00];
    expected_aad.extend_from_slice(&[0xff; 6]);
    expected_aad.extend_from_slice(&[2, 0, 0, 0, 0, 1, 2, 0, 0, 0, 0, 1]);
    assert_eq!(aad, expected_aad);
    assert_eq!(body, &[7, 0]);
    assert_eq!(element, &[76, 16, 4, 0, 6, 5, 4, 3, 2, 1]);
    assert_eq!(zero, &[0; 8]);
    assert_eq!(
        protection.finish(&[0xa5; 8]),
        26 + MANAGEMENT_MIC_ELEMENT_LEN
    );
    assert_eq!(&frame[26 + 10..], &[0xa5; 8]);

    let parsed = BipFrame::parse(&frame).unwrap();
    assert_eq!(
        (parsed.key_id().key_id(), parsed.packet_number().get()),
        (4, 0x0102_0304_0506)
    );
    assert_eq!(parsed.mic(), &[0xa5; 8]);
    let [aad, body, element, zero] = parsed.mic_input();
    assert_eq!(aad, expected_aad);
    assert_eq!(
        (body, element, zero),
        (&[7, 0][..], &frame[26..36], &[0; 8][..])
    );
}

#[test]
fn a_frame_without_its_element_or_room_is_refused_and_untouched() {
    let mut frame = deauthentication();
    assert_eq!(
        BipFrame::parse(&frame).err(),
        Some(BipFormatError::MissingManagementMic)
    );
    assert_eq!(
        BipFrame::parse(&frame[..23 + MANAGEMENT_MIC_ELEMENT_LEN]).err(),
        Some(BipFormatError::Truncated)
    );
    let before = frame;
    assert_eq!(
        BipProtection::append(&mut frame, 27, IgtkKeyId::Four, ipn(1)).err(),
        Some(BipFormatError::Truncated)
    );
    assert_eq!(
        BipProtection::append(&mut frame, 23, IgtkKeyId::Four, ipn(1)).err(),
        Some(BipFormatError::Truncated)
    );
    assert_eq!(frame, before);
}

#[test]
fn a_data_frame_or_another_version_with_a_valid_element_is_refused() {
    let mut protected = deauthentication();
    let length = BipProtection::append(&mut protected, 26, IgtkKeyId::Four, ipn(1))
        .unwrap()
        .finish(&[0xa5; 8]);
    assert!(BipFrame::parse(&protected[..length]).is_ok());
    for (byte, value) in [(0, 0x08), (0, 0x88), (0, 0xc1), (0, 0xc4)] {
        let mut frame = protected;
        frame[byte] = value;
        assert_eq!(
            BipFrame::parse(&frame[..length]).err(),
            Some(BipFormatError::NotManagement),
            "{value:#04x}"
        );
        let before = frame;
        assert_eq!(
            BipProtection::append(&mut frame, 26, IgtkKeyId::Four, ipn(1)).err(),
            Some(BipFormatError::NotManagement)
        );
        assert_eq!(frame, before);
    }
}

/// The highest IPN is written and read whole.
#[test]
fn the_highest_ipn_round_trips() {
    let mut frame = [0_u8; 26 + MANAGEMENT_MIC_ELEMENT_LEN];
    frame[..26].copy_from_slice(&deauthentication()[..26]);
    let protection = BipProtection::append(&mut frame, 26, IgtkKeyId::Four, ipn(HIGHEST)).unwrap();
    let length = protection.finish(&[0; BIP_MIC_LEN]);
    assert_eq!(
        BipFrame::parse(&frame[..length])
            .unwrap()
            .packet_number()
            .get(),
        HIGHEST
    );
    assert_eq!(ipn_from_bytes([0xff; IPN_LEN]).get(), HIGHEST);
    assert_eq!(
        ipn_to_bytes(ipn_from_bytes([1, 2, 3, 4, 5, 6])),
        [1, 2, 3, 4, 5, 6]
    );
}

#[test]
fn an_igtk_is_named_four_or_five_and_by_no_other_field() {
    for value in 0..=u16::MAX {
        let field = KeyIdField::from_bytes(value.to_le_bytes());
        let key_id = IgtkKeyId::from_field(field);
        assert_eq!(key_id.is_some(), matches!(value, 4 | 5));
        if let Some(key_id) = key_id {
            assert_eq!(key_id.field(), field);
        }
    }
}
