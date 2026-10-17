use super::*;
use crate::EapolKeyMessage;
use crate::state::RsnApAction;

fn rsn_ie() -> OwnedRsnIe<22> {
    let mut bytes = [0; 22];
    bytes[0] = RSN_ELEMENT_ID;
    bytes[1] = 20;
    OwnedRsnIe::try_copy(&bytes).unwrap()
}

fn association_ies() -> OwnedAssociationSecurityIes<22> {
    OwnedAssociationSecurityIes::try_copy(&rsn_ie(), &[]).unwrap()
}

#[test]
fn contiguous_association_security_ies_retain_exact_rsn_and_rsnxe() {
    let rsn = rsn_ie();
    let rsnxe = [RSNXE_ELEMENT_ID, 2, 0x20, 0x00];
    let mut bytes = [0_u8; 26];
    bytes[..22].copy_from_slice(rsn.as_bytes());
    bytes[22..].copy_from_slice(&rsnxe);
    let owned = OwnedAssociationSecurityIes::<128>::try_copy_bytes(&bytes).unwrap();
    assert_eq!(owned.as_bytes(), &bytes);
    assert_eq!(owned.rsn_ie(), rsn.as_bytes());

    bytes[23] = 3;
    assert_eq!(
        OwnedAssociationSecurityIes::<128>::try_copy_bytes(&bytes),
        Err(RsnFrameError::InvalidRsnxe)
    );
}

#[test]
fn gtk_key_data_round_trips_with_key_wrap_padding() {
    let rsn = rsn_ie();
    let gtk = RsnGtk::new(
        oer_ieee80211_mac::ccmp::GroupKeyId::new(2).unwrap(),
        false,
        &[0x5a; 16],
    );
    let data = RsnPlainKeyData::<64>::build(rsn.as_bytes(), &gtk, None).unwrap();
    assert_eq!(data.as_bytes().len() % 8, 0);
    let parsed = parse_gtk_key_data(data.as_bytes(), rsn.as_bytes(), &[], false)
        .map(|keys| keys.gtk)
        .unwrap();
    assert_eq!(parsed.key_id().get(), 2);
    assert!(!parsed.transmit());
    assert_eq!(parsed.key(), &[0x5a; 16]);
}

#[test]
fn protected_key_data_carries_the_rsnxe_and_an_igtk_the_station_reads() {
    let rsn = rsn_ie();
    let rsnxe = [RSNXE_ELEMENT_ID, 1, 0x20];
    let mut elements = [0_u8; 25];
    elements[..22].copy_from_slice(rsn.as_bytes());
    elements[22..].copy_from_slice(&rsnxe);
    let gtk = RsnGtk::new(
        oer_ieee80211_mac::ccmp::GroupKeyId::new(1).unwrap(),
        false,
        &[0x5a; 16],
    );
    let igtk = RsnIgtk::new(4, [1, 0, 0, 0, 0, 0], &[0x6b; 16]).unwrap();
    let data = RsnPlainKeyData::<96>::build(&elements, &gtk, Some(&igtk)).unwrap();
    assert_eq!(data.as_bytes().len() % 8, 0);
    let keys = parse_gtk_key_data(data.as_bytes(), rsn.as_bytes(), &rsnxe, true).unwrap();
    assert_eq!(keys.gtk.key(), &[0x5a; 16]);
    let parsed = keys.igtk.unwrap();
    assert_eq!(parsed.key_id(), 4);
    assert_eq!(parsed.packet_number(), [1, 0, 0, 0, 0, 0]);
    assert_eq!(parsed.key(), &[0x6b; 16]);
}

#[test]
fn builders_produce_classified_m1_to_m4() {
    let rsn = rsn_ie();
    let m1 = RsnTxFrame::<128>::message1(crate::Akm::Psk, [2; 6], 7, [3; 32]).unwrap();
    let m2 = RsnTxFrame::<128>::message2(crate::Akm::Psk, [1; 6], 7, [4; 32], &rsn).unwrap();
    let m3 =
        RsnTxFrame::<128>::message3(crate::Akm::Psk, [2; 6], 8, [3; 32], [5; 8], &[6; 24]).unwrap();
    let m4 = RsnTxFrame::<128>::message4(crate::Akm::Psk, [1; 6], 8).unwrap();
    assert_eq!(m1.key_frame().message(), EapolKeyMessage::PairwiseMessage1);
    assert_eq!(m2.key_frame().message(), EapolKeyMessage::PairwiseMessage2);
    assert_eq!(m3.key_frame().message(), EapolKeyMessage::PairwiseMessage3);
    assert!(m3.key_frame().key_info().encrypted_key_data());
    assert_eq!(m4.key_frame().message(), EapolKeyMessage::PairwiseMessage4);
    assert_eq!(m1.key_frame().key_length(), RSN_GTK_LEN as u16);
    assert_eq!(m2.key_frame().key_length(), 0);
    assert_eq!(m3.key_frame().key_length(), RSN_GTK_LEN as u16);
    assert_eq!(m4.key_frame().key_length(), 0);
    assert_eq!(m1.key_frame().protocol_version(), 2);
    assert_eq!(m2.key_frame().protocol_version(), 1);
    assert_eq!(m3.key_frame().protocol_version(), 2);
    assert_eq!(m4.key_frame().protocol_version(), 1);
}

#[test]
fn state_actions_are_bound_to_role_peer_and_nonce_context() {
    let security_ies = association_ies();
    let sta = RsnStaState::new(crate::Akm::Psk, [1; 6], [2; 6], [3; 32]).unwrap();
    let m2_action = RsnTransmit {
        message: RsnTxMessage::PairwiseMessage2,
        replay_counter: 7,
        retransmission: false,
    };
    let m2 = build_sta_action_frame::<128, _>(&sta, m2_action, &security_ies).unwrap();
    assert_eq!(m2.peer(), &[2; 6]);
    assert_eq!(m2.key_frame().nonce(), &[3; 32]);
    assert_eq!(
        build_ap_action_frame::<128>(
            &RsnApState::new(crate::Akm::Psk, [2; 6], [1; 6], [4; 32], 7).unwrap(),
            m2_action,
            [0; 8],
            &[]
        )
        .err(),
        Some(RsnFrameError::UnexpectedTransmitAction)
    );

    let ap = RsnApState::new(crate::Akm::Psk, [2; 6], [1; 6], [4; 32], 7).unwrap();
    let RsnApAction::Transmit(m1_action) = ap.message1(false).unwrap() else {
        panic!("message1 must produce a transmit action")
    };
    let m1 = build_ap_action_frame::<128>(&ap, m1_action, [0; 8], &[]).unwrap();
    assert_eq!(m1.peer(), &[1; 6]);
    assert_eq!(m1.key_frame().nonce(), &[4; 32]);
}

#[test]
fn parser_rejects_changed_rsn_ie_and_duplicate_gtk() {
    let rsn = rsn_ie();
    let gtk = RsnGtk::new(
        oer_ieee80211_mac::ccmp::GroupKeyId::new(1).unwrap(),
        false,
        &[7; 16],
    );
    let data = RsnPlainKeyData::<64>::build(rsn.as_bytes(), &gtk, None).unwrap();
    let mut other = [0; 22];
    other[0] = RSN_ELEMENT_ID;
    other[1] = 20;
    other[2] = 1;
    assert_eq!(
        parse_gtk_key_data(data.as_bytes(), &other, &[], false)
            .map(|keys| keys.gtk)
            .err(),
        Some(RsnFrameError::RsnIeMismatch)
    );

    let mut duplicate = [0; 70];
    let source = data.as_bytes();
    duplicate[..46].copy_from_slice(&source[..46]);
    duplicate[46..70].copy_from_slice(&source[22..46]);
    assert_eq!(
        parse_gtk_key_data(&duplicate, rsn.as_bytes(), &[], false)
            .map(|keys| keys.gtk)
            .err(),
        Some(RsnFrameError::DuplicateGtk)
    );
}

#[test]
fn parser_validates_authenticator_rsnxe_without_ignoring_unknown_elements() {
    let rsn = rsn_ie();
    let gtk = RsnGtk::new(
        oer_ieee80211_mac::ccmp::GroupKeyId::new(1).unwrap(),
        false,
        &[7; 16],
    );
    let data = RsnPlainKeyData::<64>::build(rsn.as_bytes(), &gtk, None).unwrap();
    let rsnxe = [RSNXE_ELEMENT_ID, 2, 0x20, 0x00];
    let mut with_rsnxe = [0; 64];
    let source = data.as_bytes();
    with_rsnxe[..22].copy_from_slice(&source[..22]);
    with_rsnxe[22..26].copy_from_slice(&rsnxe);
    with_rsnxe[26..50].copy_from_slice(&source[22..46]);
    with_rsnxe[50] = VENDOR_ELEMENT_ID;

    let parsed = parse_gtk_key_data(&with_rsnxe, rsn.as_bytes(), &rsnxe, false)
        .map(|keys| keys.gtk)
        .unwrap();
    assert_eq!(parsed.key_id().get(), 1);
    assert_eq!(parsed.key(), &[7; 16]);

    assert_eq!(
        parse_gtk_key_data(&with_rsnxe, rsn.as_bytes(), &[], false)
            .map(|keys| keys.gtk)
            .err(),
        Some(RsnFrameError::UnexpectedRsnxe)
    );
    assert_eq!(
        parse_gtk_key_data(source, rsn.as_bytes(), &rsnxe, false)
            .map(|keys| keys.gtk)
            .err(),
        Some(RsnFrameError::MissingRsnxe)
    );

    let changed = [RSNXE_ELEMENT_ID, 2, 0x21, 0x00];
    assert_eq!(
        parse_gtk_key_data(&with_rsnxe, rsn.as_bytes(), &changed, false)
            .map(|keys| keys.gtk)
            .err(),
        Some(RsnFrameError::RsnxeMismatch)
    );
}

fn igtk_kde(key_id: u16) -> [u8; 30] {
    let mut kde = [0; 30];
    kde[0] = 0xdd;
    kde[1] = 28;
    kde[2..5].copy_from_slice(&[0x00, 0x0f, 0xac]);
    kde[5] = 9;
    kde[6..8].copy_from_slice(&key_id.to_le_bytes());
    kde[8..14].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
    kde[14..30].copy_from_slice(&[0x77; 16]);
    kde
}

#[test]
fn protected_management_requires_exactly_one_igtk_beside_the_gtk() {
    let gtk = RsnGtk::new(
        oer_ieee80211_mac::ccmp::GroupKeyId::new(1).unwrap(),
        false,
        &[0x55; 16],
    );
    let rsn = rsn_ie();
    let data = RsnPlainKeyData::<96>::build(rsn.as_bytes(), &gtk, None).unwrap();
    let mut with_igtk = std::vec::Vec::from(&data.as_bytes()[..rsn.as_bytes().len() + 24]);
    with_igtk.extend_from_slice(&igtk_kde(4));

    let keys = parse_gtk_key_data(&with_igtk, rsn.as_bytes(), &[], true).unwrap();
    assert_eq!(keys.gtk.key_id().get(), 1);
    let igtk = keys.igtk.expect("IGTK KDE");
    assert_eq!(igtk.key_id(), 4);
    assert_eq!(igtk.packet_number(), [1, 2, 3, 4, 5, 6]);
    assert_eq!(igtk.key(), &[0x77; 16]);

    assert_eq!(
        parse_gtk_key_data(&with_igtk, rsn.as_bytes(), &[], false).err(),
        Some(RsnFrameError::UnexpectedIgtk)
    );
    assert_eq!(
        parse_gtk_key_data(data.as_bytes(), rsn.as_bytes(), &[], true).err(),
        Some(RsnFrameError::MissingIgtk)
    );
    let mut duplicate = with_igtk.clone();
    duplicate.extend_from_slice(&igtk_kde(5));
    assert_eq!(
        parse_gtk_key_data(&duplicate, rsn.as_bytes(), &[], true).err(),
        Some(RsnFrameError::DuplicateIgtk)
    );
    let mut wrong_id = std::vec::Vec::from(&data.as_bytes()[..rsn.as_bytes().len() + 24]);
    wrong_id.extend_from_slice(&igtk_kde(1));
    assert_eq!(
        parse_gtk_key_data(&wrong_id, rsn.as_bytes(), &[], true).err(),
        Some(RsnFrameError::InvalidKeyId)
    );
}

#[test]
fn a_group_rekey_carries_the_igtk_of_a_protected_association() {
    let mut body = std::vec::Vec::new();
    body.extend_from_slice(&[0xdd, 22, 0x00, 0x0f, 0xac, 1, 2, 0]);
    body.extend_from_slice(&[0x66; 16]);
    body.extend_from_slice(&igtk_kde(5));
    let keys = parse_group_gtk_key_data(&body, true).unwrap();
    assert_eq!(keys.gtk.key_id().get(), 2);
    assert_eq!(keys.igtk.unwrap().key_id(), 5);
    assert_eq!(
        parse_group_gtk_key_data(&body, false).err(),
        Some(RsnFrameError::UnexpectedIgtk)
    );
}
