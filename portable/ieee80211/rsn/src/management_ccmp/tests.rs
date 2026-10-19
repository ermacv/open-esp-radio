use super::*;

use crate::ccmp_mpdu::test_vector::{MANAGEMENT_BODY, management_wire as frame};

fn receiver() -> ManagementCcmpReceiver {
    ManagementCcmpReceiver::new(&core::array::from_fn(|index| index as u8))
}

#[test]
fn a_protected_management_frame_opens_once() {
    let mut ccmp = receiver();
    let mut first = frame();
    assert_eq!(ccmp.open(&mut first), Ok(MANAGEMENT_BODY.as_slice()));
    let mut again = frame();
    assert_eq!(ccmp.open(&mut again), Err(ManagementCcmpError::Replay));
}

#[test]
fn a_changed_header_or_body_fails_without_advancing_the_replay_state() {
    let mut ccmp = receiver();
    let mut changed_address = frame();
    changed_address[9] ^= 1;
    assert_eq!(
        ccmp.open(&mut changed_address),
        Err(ManagementCcmpError::InvalidMic)
    );
    assert!(changed_address[32..36].iter().all(|byte| *byte == 0));
    let mut changed_body = frame();
    changed_body[32] ^= 1;
    assert_eq!(
        ccmp.open(&mut changed_body),
        Err(ManagementCcmpError::InvalidMic)
    );
    assert!(changed_body[32..36].iter().all(|byte| *byte == 0));
    // Retry, Power Management and More Data are outside the MIC.
    let mut retry_cleared = frame();
    retry_cleared[1] &= !0x08;
    assert!(ccmp.open(&mut retry_cleared).is_ok());
}

#[test]
fn an_unprotected_or_short_frame_is_refused() {
    let mut ccmp = receiver();
    let mut unprotected = frame();
    unprotected[1] &= !0x40;
    assert_eq!(
        ccmp.open(&mut unprotected),
        Err(ManagementCcmpError::NotProtected)
    );
    let mut no_ext_iv = frame();
    no_ext_iv[27] = 0;
    assert_eq!(
        ccmp.open(&mut no_ext_iv),
        Err(ManagementCcmpError::MissingExtendedIv)
    );
    assert_eq!(ccmp.open(&mut [0; 39]), Err(ManagementCcmpError::Truncated));
}

#[test]
fn an_oversized_body_is_refused_before_transform_or_replay_commit() {
    let mut ccmp = receiver();
    let mut oversized = frame();
    oversized.resize(
        HEADER_LEN + CCMP_HEADER_LEN + usize::from(u16::MAX) + 1 + MIC_LEN,
        0xa5,
    );
    let before = oversized.clone();
    assert_eq!(ccmp.open(&mut oversized), Err(ManagementCcmpError::TooLong));
    assert_eq!(oversized, before);
    assert!(ccmp.open(&mut frame()).is_ok());
}

#[test]
fn data_and_reserved_ccmp_headers_never_reach_management_replay() {
    let mut ccmp = receiver();
    let mut data = frame();
    data[0] = 0x08;
    let before = data.clone();
    assert_eq!(
        ccmp.open(&mut data),
        Err(ManagementCcmpError::MalformedHeader)
    );
    assert_eq!(data, before);
    let mut reserved = frame();
    reserved[26] = 1;
    assert_eq!(
        ccmp.open(&mut reserved),
        Err(ManagementCcmpError::InvalidCcmpHeader)
    );
    assert!(ccmp.open(&mut frame()).is_ok());
}
