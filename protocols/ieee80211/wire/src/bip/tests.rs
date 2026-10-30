use super::*;

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
    let protection = BipProtection::append(&mut frame, 26, 4, 0x0102_0304_0506).unwrap();
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
        (parsed.key_id(), parsed.packet_number()),
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
        BipProtection::append(&mut frame, 27, 4, 1).err(),
        Some(BipFormatError::Truncated)
    );
    assert_eq!(
        BipProtection::append(&mut frame, 26, 4, IPN_MAX + 1).err(),
        Some(BipFormatError::PacketNumberOutOfRange)
    );
    assert_eq!(
        BipProtection::append(&mut frame, 23, 4, 1).err(),
        Some(BipFormatError::Truncated)
    );
    assert_eq!(frame, before);
}
