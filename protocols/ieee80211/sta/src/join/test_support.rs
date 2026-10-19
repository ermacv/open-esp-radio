//! Synthetic peer management frames for STA state-machine tests.

use oer_ieee80211_mac::management::MANAGEMENT_HEADER_LEN;

const OPEN_AUTHENTICATION_FRAME_CONTROL: u16 = 0x00b0;
const DEAUTHENTICATION_FRAME_CONTROL: u16 = 0x00c0;
const ASSOCIATION_RESPONSE_FRAME_CONTROL: u16 = 0x0010;
const OPEN_SYSTEM_RESPONSE_SEQUENCE: u16 = 2;

pub(crate) const LOCAL: [u8; 6] = [0x02, 0, 0, 0x12, 0x34, 0x56];

pub(crate) const BSSID: [u8; 6] = [0x30, 0x05, 0x5c, 0x11, 0x22, 0x33];

/// The test AP answers only an Open System request addressed to its BSS
/// from the station. Validate the bytes the encoder actually emitted.
pub(crate) fn answer_authentication(request: &[u8], status: u16) -> Option<[u8; 30]> {
    let (frame, _) = request.split_first_chunk::<30>()?;
    let control = u16::from_le_bytes([frame[0], frame[1]]);
    if control & !0x0800 != OPEN_AUTHENTICATION_FRAME_CONTROL
        || frame[4..10] != BSSID
        || frame[10..16] != LOCAL
        || frame[16..22] != BSSID
        || frame[24..30] != [0, 0, 1, 0, 0, 0]
    {
        return None;
    }
    Some(authentication_response(status))
}

pub(crate) fn authentication_response(status_code: u16) -> [u8; 30] {
    let mut frame = [0_u8; 30];
    frame[0..2].copy_from_slice(&OPEN_AUTHENTICATION_FRAME_CONTROL.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[26..28].copy_from_slice(&OPEN_SYSTEM_RESPONSE_SEQUENCE.to_le_bytes());
    frame[28..30].copy_from_slice(&status_code.to_le_bytes());
    frame
}

pub(crate) fn association_response(status_code: u16) -> [u8; 30] {
    let mut frame = [0_u8; 30];
    frame[0..2].copy_from_slice(&ASSOCIATION_RESPONSE_FRAME_CONTROL.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[24..26].copy_from_slice(&0x0431_u16.to_le_bytes());
    frame[26..28].copy_from_slice(&status_code.to_le_bytes());
    frame[28..30].copy_from_slice(&0xc02a_u16.to_le_bytes());
    frame
}

pub(crate) fn deauthentication(reason_code: u16) -> [u8; MANAGEMENT_HEADER_LEN + 2] {
    let mut frame = [0_u8; MANAGEMENT_HEADER_LEN + 2];
    frame[0..2].copy_from_slice(&DEAUTHENTICATION_FRAME_CONTROL.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[24..26].copy_from_slice(&reason_code.to_le_bytes());
    frame
}
