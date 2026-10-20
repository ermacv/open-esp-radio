use super::*;
use oer_ieee80211_cipher::ccmp_mpdu::CcmpAssociation;
use oer_ieee80211_wire::{ccmp::CCMP_HEADER_LEN, security::Spp};

const HEADER_LEN: usize = 24;
const MIC_LEN: usize = 8;

use oer_ieee80211_wire::management_ccmp_vector::{MANAGEMENT_BODY, management_wire as frame};

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

fn mode(spp: bool) -> CcmpAssociation {
    CcmpAssociation::new(if spp {
        Spp::Negotiated
    } else {
        Spp::NotNegotiated
    })
}
fn key() -> [u8; 16] {
    core::array::from_fn(|i| i as u8)
}

#[test]
fn management_wire_vector_matches_the_replay_receiver_in_both_spp_modes() {
    use oer_ieee80211_wire::management_ccmp_vector::{MANAGEMENT_BODY, management_wire};
    for spp in [false, true] {
        let wire = management_wire();
        let mut mpdu = wire.clone();
        assert_eq!(mode(spp).open(&key(), &mut mpdu).unwrap(), MANAGEMENT_BODY);
        mode(spp).seal(&key(), &mut mpdu).unwrap();
        assert_eq!(mpdu, wire);
        let mut receiver = crate::management_ccmp::ManagementCcmpReceiver::new(&key());
        assert_eq!(receiver.open(&mut mpdu).unwrap(), MANAGEMENT_BODY);
    }
}

// SOURCE: IEEE 802.11-2020 12.5.3 management CCMP admission rules.
// Independently encrypted with Python cryptography AESCCM (key 00..0f,
// body 08011234, management nonce flag 10, AAD FC/A1/A2/A3/fragment).
fn independent_wire(hex: &str) -> std::vec::Vec<u8> {
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(core::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn management_packet_number_zero_is_never_admitted() {
    let mut ccmp = receiver();
    let mut zero = independent_wire(
        "d040000002000000000102000000000202000000000235120000002000000000ee52738e614e583acdf68edf",
    );
    assert_eq!(ccmp.open(&mut zero), Err(ManagementCcmpError::Replay));
    assert!(ccmp.open(&mut frame()).is_ok());
}

#[test]
fn management_group_key_id_is_rejected_before_replay_or_decryption() {
    let mut ccmp = receiver();
    let mut group_key = independent_wire(
        "d04000000200000000010200000000020200000000023512010000600000000033245c7c758b6815c5da9971",
    );
    let before = group_key.clone();
    assert_eq!(
        ccmp.open(&mut group_key),
        Err(ManagementCcmpError::UnexpectedKeyId)
    );
    assert_eq!(group_key, before);
    assert!(ccmp.open(&mut frame()).is_ok());
}

#[test]
fn management_group_address_is_rejected_before_replay_or_decryption() {
    let mut ccmp = receiver();
    let mut group_address = independent_wire(
        "d04000000300000000010200000000020200000000023512010000200000000033245c7c891ff31ca9453ccf",
    );
    let before = group_address.clone();
    assert_eq!(
        ccmp.open(&mut group_address),
        Err(ManagementCcmpError::GroupAddressed)
    );
    assert_eq!(group_address, before);
    assert!(ccmp.open(&mut frame()).is_ok());
}
