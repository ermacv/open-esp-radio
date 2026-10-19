//! Every entry point that takes received bytes returns, on any input.
//!
//! Each test drives one entry point with [`QUICK`] inputs (random and
//! mutated real frames) and, `#[ignore]`d, with [`FULL`]. The SAE exchange,
//! a scalar multiplication per accepted commit, runs a tenth as many.

use std::vec::Vec;

use oer_ieee80211_mac::{ccmp::GroupKeyId, security::Spp};

use super::*;
use crate::{
    Akm, EapolKeyFrame, OwnedEapolFrame, Pmk, PtkContext, RsnInterface,
    aes::software_aes128_key_unwrap,
    bip::{BipReceiver, BipTransmitter},
    ccmp_mpdu::{CcmpAssociation, test_vector::management_wire},
    element::validate_rsn_element,
    frames::{
        OwnedAssociationSecurityIes, OwnedRsnIe, RsnIgtk,
        captured_m3::{MESSAGE3, RSN_IE, SNONCE},
        parse_group_gtk_key_data, parse_gtk_key_data,
    },
    management_ccmp::ManagementCcmpReceiver,
    sae::{
        SaeComebackTokens, SaeCommit, SaeCommitValues, SaePasswordElement, SaeReceivedCommit,
        anti_clogging_token,
    },
};

/// The captured Message 3's PTK and its decrypted key data.
fn message3() -> (crate::Ptk, Vec<u8>) {
    let frame = EapolKeyFrame::parse(&MESSAGE3).unwrap();
    let pmk = Pmk::derive(b"open-esp-radio-m3", b"oer-m3-784").unwrap();
    let ptk = pmk.derive_ptk(
        Akm::PskSha256,
        PtkContext {
            authenticator_address: [0xbe, 0xfc, 0xe7, 0xae, 0xbb, 0xd4],
            supplicant_address: [0x1c, 0xed, 0x6f, 0xb9, 0xba, 0x3c],
            authenticator_nonce: *frame.nonce(),
            supplicant_nonce: SNONCE,
        },
    );
    let plain = software_aes128_key_unwrap(ptk.kek(), frame.key_data()).unwrap();
    let plain = Vec::from(plain.as_bytes());
    (ptk, plain)
}

fn eapol(count: usize) {
    let (ptk, _) = message3();
    inputs(&[&MESSAGE3], count, 300, |bytes| {
        if let Ok(frame) = EapolKeyFrame::parse(bytes) {
            let _ = (
                frame.protocol_version(),
                frame.key_info(),
                frame.message(),
                frame.key_length(),
                frame.replay_counter(),
                frame.nonce(),
                frame.key_iv(),
                frame.mic(),
                frame.key_receive_sequence(),
                frame.key_identifier(),
                frame.key_data(),
            );
            let _ = frame.verify_mic(&ptk);
        }
        if let Ok(owned) =
            OwnedEapolFrame::<256>::try_copy(RsnInterface::Station, [2, 0, 0, 0, 0, 1], bytes)
        {
            let _ = (owned.as_bytes(), owned.key_frame().replay_counter());
        }
    });
}

#[test]
fn eapol_key_frames_never_panic() {
    eapol(QUICK);
}

#[test]
#[ignore = "the full run"]
fn eapol_key_frames_never_panic_full() {
    eapol(FULL);
}

fn key_data(count: usize) {
    let (_, plain) = message3();
    inputs(&[&plain], count, 200, |bytes| {
        for protection in [false, true] {
            let _ = parse_gtk_key_data(bytes, &RSN_IE, &[], protection);
            let _ = parse_gtk_key_data(bytes, &RSN_IE, &[0xf4, 1, 0x20], protection);
            let _ = parse_group_gtk_key_data(bytes, protection);
        }
    });
}

#[test]
fn key_data_never_panics() {
    key_data(QUICK);
}

#[test]
#[ignore = "the full run"]
fn key_data_never_panics_full() {
    key_data(FULL);
}

fn security_elements(count: usize) {
    let rsnxe = [0xf4, 1, 0x20];
    let mut association = Vec::from(RSN_IE);
    association.extend_from_slice(&rsnxe);
    inputs(&[&RSN_IE, &association], count, 120, |bytes| {
        if let Ok(ie) = OwnedRsnIe::<64>::try_copy(bytes) {
            let _ = ie.as_bytes();
        }
        if let Ok(ies) = OwnedAssociationSecurityIes::<128>::try_copy_bytes(bytes) {
            let _ = (ies.as_bytes(), ies.rsn_ie(), ies.rsnxe());
        }
        if let Ok(element) = validate_rsn_element(bytes) {
            let _ = (
                element.lists_akm(Akm::PskSha256),
                element.pmkids().count(),
                element.management_frame_protection(),
            );
        }
    });
}

#[test]
fn security_elements_never_panic() {
    security_elements(QUICK);
}

#[test]
#[ignore = "the full run"]
fn security_elements_never_panic_full() {
    security_elements(FULL);
}

fn key_unwrap(count: usize) {
    let (ptk, _) = message3();
    let wrapped = EapolKeyFrame::parse(&MESSAGE3).unwrap().key_data().to_vec();
    inputs(&[&wrapped], count, 200, |bytes| {
        let _ = software_aes128_key_unwrap(ptk.kek(), bytes);
    });
}

#[test]
fn key_unwrap_never_panics() {
    key_unwrap(QUICK);
}

#[test]
#[ignore = "the full run"]
fn key_unwrap_never_panics_full() {
    key_unwrap(FULL);
}

fn bip(count: usize) {
    let igtk = RsnIgtk::new(4, [1, 0, 0, 0, 0, 0], &[0x5a; 16]).unwrap();
    let mut transmitter = BipTransmitter::new(&igtk);
    let mut frame = [0_u8; 26 + 18];
    frame[0] = 0xc0;
    frame[4] = 0xff;
    let length = transmitter.protect(&mut frame, 26).unwrap();
    let seed = frame[..length].to_vec();
    inputs(&[&seed], count, 120, |bytes| {
        let mut receiver = BipReceiver::new(&igtk);
        let _ = receiver.verify(bytes);
    });
}

#[test]
fn bip_never_panics() {
    bip(QUICK);
}

#[test]
#[ignore = "the full run"]
fn bip_never_panics_full() {
    bip(FULL);
}

fn ccmp(count: usize) {
    let wire = management_wire();
    let key: [u8; 16] = core::array::from_fn(|index| index as u8);
    let mut copy = Vec::new();
    inputs(&[&wire], count, 120, |bytes| {
        for spp in [Spp::NotNegotiated, Spp::Negotiated] {
            copy.clear();
            copy.extend_from_slice(bytes);
            let _ = CcmpAssociation::new(spp).open(&key, &mut copy);
        }
        copy.clear();
        copy.extend_from_slice(bytes);
        let _ = ManagementCcmpReceiver::new(&key).open(&mut copy);
    });
}

#[test]
fn ccmp_never_panics() {
    ccmp(QUICK);
}

#[test]
#[ignore = "the full run"]
fn ccmp_never_panics_full() {
    ccmp(FULL);
}

/// Two commits of one password element and the keys each side derives.
fn sae_peers() -> (SaeCommit, SaeCommit) {
    let a = [2, 0, 0, 0, 0, 1];
    let b = [2, 0, 0, 0, 0, 2];
    let pwe = SaePasswordElement::hunting_and_pecking(b"password", a, b).unwrap();
    (
        SaeCommit::new(pwe, [0x11; 32], [0x22; 32]).unwrap(),
        SaeCommit::new(pwe, [0x33; 32], [0x44; 32]).unwrap(),
    )
}

fn sae_parsers(count: usize) {
    let (_, peer) = sae_peers();
    let mut plain = [0; 98];
    let length = peer.values().encode(None, false, &mut plain).unwrap();
    let mut with_token = [0; 98 + 32 + 3];
    let token_length = peer
        .values()
        .encode(Some(&[7; 32]), true, &mut with_token)
        .unwrap();
    let mut tokens = SaeComebackTokens::new();
    let issued = tokens.issue(
        [2, 0, 0, 0, 0, 3],
        oer_base_time::Instant::from_micros(0),
        || [9; 32],
    );
    inputs(
        &[&plain[..length], &with_token[..token_length], &issued],
        count,
        200,
        |bytes| {
            for h2e in [false, true] {
                let _ = SaeCommitValues::parse(bytes, h2e);
                if let Ok(commit) = SaeReceivedCommit::parse(bytes, h2e) {
                    let _ = commit.rejects_offered_group();
                }
                let _ = anti_clogging_token(bytes, h2e);
            }
            let _ = tokens.check([2, 0, 0, 0, 0, 3], bytes);
        },
    );
}

#[test]
fn sae_parsers_never_panic() {
    sae_parsers(QUICK);
}

#[test]
#[ignore = "the full run"]
fn sae_parsers_never_panic_full() {
    sae_parsers(FULL);
}

fn sae_exchange(count: usize) {
    let (own, peer) = sae_peers();
    let keys = own.process(peer.values()).unwrap();
    let peer_keys = peer.process(own.values()).unwrap();
    let confirm = peer_keys.own_confirm(1);
    let mut commit = [0; 98];
    let length = peer.values().encode(None, false, &mut commit).unwrap();
    inputs(&[&confirm, &commit[..length]], count, 120, |bytes| {
        let _ = keys.verify_peer_confirm(bytes);
        if let Ok(values) = SaeCommitValues::parse(bytes, false) {
            let _ = own.process(values);
        }
    });
}

#[test]
fn sae_exchange_never_panics() {
    sae_exchange(QUICK / 10);
}

#[test]
#[ignore = "the full run"]
fn sae_exchange_never_panics_full() {
    sae_exchange(FULL / 10);
}

#[test]
fn the_seeds_are_real_frames() {
    // The mutations start from frames the parsers accept, so they reach
    // the parsers' depths rather than stopping at the first check.
    assert!(EapolKeyFrame::parse(&MESSAGE3).is_ok());
    let (_, plain) = message3();
    assert!(parse_gtk_key_data(&plain, &RSN_IE, &[], true).is_ok());
    assert!(validate_rsn_element(&RSN_IE).is_ok());
    let _ = GroupKeyId::new(1);
}

/// The views above expect what their constructors checked; the constructors
/// refuse every input those expectations would not hold for. The loops above
/// also call each view on every input a constructor accepted.
#[test]
fn constructors_refuse_what_the_views_rely_on() {
    use crate::{
        EapolCopyError,
        frames::RsnFrameError,
        sae::{SaeError, SaePasswordElement},
    };
    let station = [2, 0, 0, 0, 0, 1];
    assert!(matches!(
        OwnedEapolFrame::<256>::try_copy(RsnInterface::Station, station, &MESSAGE3[..120]),
        Err(EapolCopyError::Invalid(_))
    ));
    assert_eq!(
        OwnedEapolFrame::<100>::try_copy(RsnInterface::Station, station, &MESSAGE3),
        Err(EapolCopyError::CapacityExceeded)
    );
    let owned =
        OwnedEapolFrame::<256>::try_copy(RsnInterface::Station, station, &MESSAGE3).unwrap();
    assert_eq!(owned.key_frame().bytes(), MESSAGE3.as_slice());

    let mut long = Vec::from(RSN_IE);
    long[1] += 1;
    assert_eq!(
        OwnedRsnIe::<64>::try_copy(&long),
        Err(RsnFrameError::InvalidRsnIe)
    );
    assert_eq!(
        OwnedRsnIe::<16>::try_copy(&RSN_IE),
        Err(RsnFrameError::CapacityExceeded)
    );
    assert_eq!(
        OwnedAssociationSecurityIes::<128>::try_copy_bytes(&long),
        Err(RsnFrameError::InvalidRsnIe)
    );
    assert!(validate_rsn_element(&RSN_IE[..RSN_IE.len() - 1]).is_err());

    let pwe =
        SaePasswordElement::hunting_and_pecking(b"password", station, [2, 0, 0, 0, 0, 2]).unwrap();
    let one = core::array::from_fn(|index| u8::from(index == 31));
    for rand in [[0; 32], one, [0xff; 32]] {
        assert!(matches!(
            SaeCommit::new(pwe, rand, [0x22; 32]),
            Err(SaeError::UnsuitableRandom)
        ));
    }
}
