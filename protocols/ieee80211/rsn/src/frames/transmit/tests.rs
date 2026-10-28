#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects,
    clippy::unreachable
)]
use super::*;

#[test]
fn ethernet_builder_owns_complete_eapol_frame() {
    let mut m4 = RsnTxFrame::<128>::message4(crate::Akm::Psk, [1; 6], 9).unwrap();
    m4.set_mic(&[0xa5; 16]);
    let ethernet = RsnEthernetFrame::<160>::build([2; 6], &m4).unwrap();
    assert_eq!(&ethernet.as_bytes()[..6], &[1; 6]);
    assert_eq!(&ethernet.as_bytes()[6..12], &[2; 6]);
    assert_eq!(&ethernet.as_bytes()[12..14], &EAPOL_ETHERTYPE);
    assert_eq!(&ethernet.as_bytes()[14 + 81..14 + 97], &[0xa5; 16]);
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
fn insufficient_capacity_never_writes_a_partial_eapol_or_ethernet_frame() {
    for result in [
        RsnTxFrame::<0>::message4(crate::Akm::Psk, [1; 6], 9).map(|_| ()),
        RsnTxFrame::<98>::message4(crate::Akm::Psk, [1; 6], 9).map(|_| ()),
    ] {
        assert_eq!(result, Err(RsnFrameError::CapacityExceeded));
    }
    let frame = RsnTxFrame::<99>::message4(crate::Akm::Psk, [1; 6], 9).unwrap();
    assert_eq!(
        RsnEthernetFrame::<112>::build([2; 6], &frame),
        Err(RsnFrameError::CapacityExceeded)
    );
    assert_eq!(
        RsnEthernetFrame::<113>::build([2; 6], &frame)
            .unwrap()
            .as_bytes()
            .len(),
        113
    );
}
