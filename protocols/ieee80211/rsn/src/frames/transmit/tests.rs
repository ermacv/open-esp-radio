#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects,
    clippy::unreachable
)]
use super::*;

#[test]
fn a_message_owns_its_peer_and_the_mic_it_was_given() {
    let mut m4 = RsnTxFrame::<128>::message4(crate::Akm::Psk, [1; 6], 9).unwrap();
    m4.set_mic(&[0xa5; 16]);
    assert_eq!(m4.peer(), &[1; 6]);
    assert_eq!(&m4.as_bytes()[81..97], &[0xa5; 16]);
}

#[test]
fn a_damaged_transmit_frame_returns_its_parse_error() {
    let mut frame = RsnTxFrame::<128>::message4(crate::Akm::Psk, [1; 6], 9).unwrap();
    frame.bytes[2] = 1;
    assert_eq!(
        frame.key_frame(),
        Err(crate::EapolParseError::LengthMismatch)
    );
}

#[test]
fn insufficient_capacity_never_writes_a_partial_eapol_frame() {
    for result in [
        RsnTxFrame::<0>::message4(crate::Akm::Psk, [1; 6], 9).map(|_| ()),
        RsnTxFrame::<98>::message4(crate::Akm::Psk, [1; 6], 9).map(|_| ()),
    ] {
        assert_eq!(result, Err(RsnFrameError::CapacityExceeded));
    }
    assert_eq!(
        RsnTxFrame::<99>::message4(crate::Akm::Psk, [1; 6], 9)
            .unwrap()
            .as_bytes()
            .len(),
        99
    );
}
