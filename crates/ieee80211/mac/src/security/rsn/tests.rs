use super::*;

const CCMP: [u8; 4] = ieee_suite(RSN_CIPHER_CCMP);
const PSK: [u8; 4] = ieee_suite(RSN_AKM_PSK);
const SAE: [u8; 4] = ieee_suite(8);
const BIP: [u8; 4] = ieee_suite(6);

/// Frame `body` as an RSN element.
fn element(body: &[u8]) -> ([u8; 64], usize) {
    let mut element = [0; 64];
    element[0] = RSN_ELEMENT_ID;
    element[1] = body.len() as u8;
    element[2..2 + body.len()].copy_from_slice(body);
    (element, body.len() + 2)
}

/// Version, CCMP group, one CCMP pairwise and the given AKM list.
fn base_body(akms: &[[u8; 4]]) -> ([u8; 48], usize) {
    let mut body = [0; 48];
    let mut length = 0;
    let mut push = |bytes: &[u8]| {
        body[length..length + bytes.len()].copy_from_slice(bytes);
        length += bytes.len();
    };
    push(&RSN_VERSION.to_le_bytes());
    push(&CCMP);
    push(&1_u16.to_le_bytes());
    push(&CCMP);
    push(&(akms.len() as u16).to_le_bytes());
    for akm in akms {
        push(akm);
    }
    (body, length)
}

fn with_tail(akms: &[[u8; 4]], tail: &[u8]) -> ([u8; 64], usize) {
    let (mut body, length) = base_body(akms);
    body[length..length + tail.len()].copy_from_slice(tail);
    element(&body[..length + tail.len()])
}

#[test]
fn minimal_element_exposes_suites_without_optional_fields() {
    let (bytes, length) = with_tail(&[SAE, PSK], &[]);
    let parsed = RsnElement::parse(&bytes[..length]).unwrap();

    assert_eq!(parsed.group_data_cipher(), CCMP);
    assert!(parsed.pairwise_ciphers().contains(CCMP));
    assert_eq!(parsed.akm_suites().len(), 2);
    assert!(parsed.akm_suites().iter().eq([SAE, PSK]));
    assert!(parsed.akm_suites().contains(PSK));
    assert_eq!(parsed.capabilities(), None);
    assert_eq!(parsed.pmkid_count(), None);
    assert!(parsed.pmkids().next().is_none());
    assert_eq!(parsed.group_management_cipher(), None);
}

#[test]
fn complete_optional_tail_is_exposed_without_policy() {
    let mut tail = [0; 2 + 2 + RSN_PMKID_LEN + 4];
    tail[..2].copy_from_slice(&(RSN_CAPABILITY_MFPR | RSN_CAPABILITY_MFPC).to_le_bytes());
    tail[2..4].copy_from_slice(&1_u16.to_le_bytes());
    tail[4..4 + RSN_PMKID_LEN].fill(0xa5);
    tail[4 + RSN_PMKID_LEN..].copy_from_slice(&BIP);
    let (bytes, length) = with_tail(&[PSK], &tail);
    let parsed = RsnElement::parse(&bytes[..length]).unwrap();

    assert_eq!(
        parsed.capabilities(),
        Some(RSN_CAPABILITY_MFPR | RSN_CAPABILITY_MFPC)
    );
    assert_eq!(parsed.pmkid_count(), Some(1));
    assert!(parsed.pmkids().eq([[0xa5; RSN_PMKID_LEN]]));
    assert_eq!(parsed.group_management_cipher(), Some(BIP));
}

#[test]
fn empty_suite_lists_are_syntactically_valid() {
    let mut body = [0; 10];
    body[..2].copy_from_slice(&RSN_VERSION.to_le_bytes());
    body[2..6].copy_from_slice(&CCMP);
    let (bytes, length) = element(&body);
    let parsed = RsnElement::parse(&bytes[..length]).unwrap();

    assert!(parsed.pairwise_ciphers().is_empty());
    assert!(parsed.akm_suites().is_empty());
}

#[test]
fn unknown_version_is_reported_before_the_body_layout() {
    let (bytes, length) = element(&[2, 0, 0xa5]);
    assert_eq!(
        RsnElement::parse(&bytes[..length]),
        Err(RsnSyntaxError::UnsupportedVersion)
    );
}

#[test]
fn incomplete_or_trailing_fields_are_malformed() {
    let mut truncated_pmkid = [0; 2 + 2 + RSN_PMKID_LEN - 1];
    truncated_pmkid[2..4].copy_from_slice(&1_u16.to_le_bytes());
    let tails: [&[u8]; 5] = [
        &[0xa5],
        &[0, 0, 0xa5],
        &truncated_pmkid,
        &[0, 0, 0, 0, 0x00, 0x0f, 0xac],
        &[0, 0, 0, 0, 0x00, 0x0f, 0xac, 6, 0xa5],
    ];
    for tail in tails {
        let (bytes, length) = with_tail(&[PSK], tail);
        assert_eq!(
            RsnElement::parse(&bytes[..length]),
            Err(RsnSyntaxError::Malformed)
        );
    }

    let (bytes, length) = with_tail(&[PSK], &[]);
    let mut short_list = bytes;
    short_list[1] -= 1;
    assert_eq!(
        RsnElement::parse(&short_list[..length - 1]),
        Err(RsnSyntaxError::Malformed)
    );
}

#[test]
fn header_must_name_an_rsn_element_of_exact_length() {
    let (bytes, length) = with_tail(&[PSK], &[]);
    let mut wrong_id = bytes;
    wrong_id[0] = 0xdd;
    assert_eq!(
        RsnElement::parse(&wrong_id[..length]),
        Err(RsnSyntaxError::Malformed)
    );
    assert_eq!(
        RsnElement::parse(&bytes[..length + 1]),
        Err(RsnSyntaxError::Malformed)
    );
    assert_eq!(RsnElement::parse(&[]), Err(RsnSyntaxError::Malformed));
}
