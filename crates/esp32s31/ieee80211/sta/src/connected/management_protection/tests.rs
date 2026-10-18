use super::*;

use std::vec::Vec;

/// A protected SA Query Response, transaction 0x3412, under the temporal key
/// 00..0f; the vector of the RSN crate's management CCMP tests.
const SA_QUERY_RESPONSE: &str =
    "d04800000200000000010200000000020200000000023512a10000200000000012714ee3c7f2391ea46ccabd";

/// A group-addressed Deauthentication, reason 7, under IGTK 4 at IPN 9; the
/// vector of the RSN crate's BIP tests.
fn group_deauthentication() -> Vec<u8> {
    let mut frame = Vec::new();
    frame.extend_from_slice(&[0xc0, 0x18, 0, 0]);
    frame.extend_from_slice(&[0xff; 6]);
    frame.extend_from_slice(&[2, 0, 0, 0, 0, 1]);
    frame.extend_from_slice(&[2, 0, 0, 0, 0, 1]);
    frame.extend_from_slice(&[0x10, 0x00]);
    frame.extend_from_slice(&[7, 0]);
    frame.extend_from_slice(&[76, 16, 4, 0, 9, 0, 0, 0, 0, 0]);
    frame.extend_from_slice(&[0xeb, 0x79, 0x2a, 0x77, 0x23, 0x5b, 0xf1, 0x7a]);
    frame
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap())
        .collect()
}

fn protection() -> StationManagementProtection {
    StationManagementProtection::new(
        core::array::from_fn(|index| index as u8),
        &RsnIgtk::new(4, [8, 0, 0, 0, 0, 0], [0x77; 16]).unwrap(),
    )
}

#[test]
fn an_individual_frame_opens_to_its_action() {
    let mut protection = protection();
    let mut frame = ProtectedManagementFrame::try_copy(&hex(SA_QUERY_RESPONSE), false).unwrap();
    let replay = frame;
    assert_eq!(
        protection.receive(&mut frame),
        Ok(Some(ConnectedRxControlEvent::SaQuery(SaQuery::Response {
            transaction: [0x12, 0x34],
        })))
    );
    let mut replay = replay;
    assert_eq!(
        protection.receive(&mut replay),
        Err(ProtectedManagementDrop::Individual(
            ManagementCcmpError::Replay
        ))
    );
}

#[test]
fn a_group_disconnect_verifies_under_bip() {
    let mut protection = protection();
    let mut frame = ProtectedManagementFrame::try_copy(&group_deauthentication(), true).unwrap();
    assert_eq!(
        protection.receive(&mut frame),
        Ok(Some(ConnectedRxControlEvent::PeerDisconnect(
            StaDisconnect {
                kind: StaDisconnectKind::Deauthentication,
                reason_code: 7,
            }
        )))
    );
    let mut forged = group_deauthentication();
    forged[24] = 3;
    let mut forged = ProtectedManagementFrame::try_copy(&forged, true).unwrap();
    assert_eq!(
        self::protection().receive(&mut forged),
        Err(ProtectedManagementDrop::Group(BipError::InvalidMic))
    );
}

#[test]
fn an_unprotected_individual_frame_is_dropped() {
    let mut protection = protection();
    let mut unprotected = hex(SA_QUERY_RESPONSE);
    unprotected[1] &= !0x40;
    let mut frame = ProtectedManagementFrame::try_copy(&unprotected, false).unwrap();
    assert_eq!(
        protection.receive(&mut frame),
        Err(ProtectedManagementDrop::Individual(
            ManagementCcmpError::NotProtected
        ))
    );
    assert!(ProtectedManagementFrame::try_copy(&[0; 97], false).is_none());
}
