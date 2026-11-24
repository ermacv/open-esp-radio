use super::*;

#[test]
fn a_message_owns_its_peer_and_the_mic_it_was_given() {
    let mut m4 = RsnTxFrame::<128>::message4(crate::Akm::Psk, [1; 6], 9);
    m4.set_mic(&[0xa5; 16]);
    assert_eq!(m4.peer(), &[1; 6]);
    assert_eq!(&m4.as_bytes()[81..97], &[0xa5; 16]);
}

#[test]
fn a_transmit_frame_parses_and_its_damaged_copy_returns_the_parse_error() {
    let frame = RsnTxFrame::<128>::message4(crate::Akm::Psk, [1; 6], 9);
    assert!(frame.key_frame().is_ok());
    let mut damaged = frame.as_bytes().to_vec();
    damaged[2] = 1;
    assert_eq!(
        crate::EapolKeyFrame::parse(&damaged),
        Err(oer_protocols_ieee80211_wire::eapol::EapolParseError::LengthMismatch)
    );
}

/// A frame without Key Data fills its exact length; a smaller capacity is
/// a compile error, not a refusal.
#[test]
fn message4_fills_its_exact_length() {
    assert_eq!(
        RsnTxFrame::<99>::message4(crate::Akm::Psk, [1; 6], 9)
            .as_bytes()
            .len(),
        99
    );
}
