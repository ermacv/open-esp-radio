use std::{vec, vec::Vec};

use oer_bluetooth_ll::security::{
    LeLongTermKey, LePeripheralEncryptionProcedure as Procedure, LePeripheralEncryptionRandom,
    LePeripheralEncryptionReceiveMode as Mode, LePeripheralEncryptionTermination as Termination,
};

use super::{PDU_CAPACITY, decode};

// The Core specification's encryption sample data (Vol 6 Part C, 1),
// little endian as the Link Layer carries it.
const LTK: [u8; 16] = [
    0xbf, 0x01, 0xfb, 0x9d, 0x4e, 0xf3, 0xbc, 0x36, 0xd8, 0x74, 0xf5, 0x39, 0x41, 0x38, 0x68, 0x4c,
];
/// `LL_ENC_REQ`: opcode, Rand, EDIV, SKDm, IVm.
const ENC_REQ: [u8; 23] = [
    0x03, 0x90, 0x78, 0x56, 0x34, 0x12, 0xef, 0xcd, 0xab, 0x74, 0x24, 0x13, 0x02, 0xf1, 0xe0, 0xdf,
    0xce, 0xbd, 0xac, 0x24, 0xab, 0xdc, 0xba,
];
/// The Central's encrypted `LL_START_ENC_RSP` (counter zero) and its first
/// encrypted data PDU (counter one, LLID 10, 27 plaintext bytes).
const START_ENC_RSP: [u8; 5] = [0x9f, 0xcd, 0xa7, 0xf4, 0x48];
const CENTRAL_DATA: [u8; 31] = [
    0x7a, 0x70, 0xd6, 0x64, 0x15, 0x22, 0x6d, 0xf2, 0x6b, 0x17, 0x83, 0x9a, 0x06, 0x04, 0x05, 0x59,
    0x6b, 0xd6, 0x56, 0x4f, 0x79, 0x6b, 0x5b, 0x9c, 0xe6, 0xff, 0x32, 0xf7, 0x5a, 0x6d, 0x33,
];

fn random() -> Option<LePeripheralEncryptionRandom> {
    Some(LePeripheralEncryptionRandom::new(
        [0x79, 0x68, 0x57, 0x46, 0x35, 0x24, 0x13, 0x02],
        [0xbe, 0xba, 0xaf, 0xde],
    ))
}

fn pdu(header: u8, payload: &[u8]) -> Vec<u8> {
    let mut pdu = vec![header, payload.len() as u8];
    pdu.extend_from_slice(payload);
    pdu
}

/// Decode `pdu` and return the dispatched plaintext, if any, as owned bytes.
fn receive(procedure: &mut Procedure, pdu: &[u8]) -> Option<Vec<u8>> {
    let mut decoded = [0; PDU_CAPACITY];
    decode(procedure, pdu, &mut decoded, random).map(<[u8]>::to_vec)
}

/// A procedure that decoded the sample `LL_ENC_REQ`, got its key and
/// awaits the Central's encrypted `LL_START_ENC_RSP`.
fn awaiting_start_response() -> Procedure {
    let mut procedure = Procedure::new();
    assert_eq!(receive(&mut procedure, &pdu(0x03, &ENC_REQ)), None);
    procedure.response_enqueued().unwrap();
    procedure.observe_transmission_completion(true);
    assert!(
        procedure
            .provide_long_term_key(LeLongTermKey::new(LTK))
            .is_ok()
    );
    procedure.response_enqueued().unwrap();
    assert_eq!(procedure.receive_mode(), Mode::EncryptedStartResponse);
    procedure
}

fn active() -> Procedure {
    let mut procedure = awaiting_start_response();
    assert_eq!(receive(&mut procedure, &pdu(0x0f, &START_ENC_RSP)), None);
    procedure.response_enqueued().unwrap();
    assert!(procedure.is_active());
    procedure
}

fn central_counter(procedure: &mut Procedure) -> Option<u64> {
    procedure
        .active_encryption()
        .and_then(|encryption| encryption.next_central_transmit_counter())
}

#[test]
fn plaintext_data_passes_through_unchanged() {
    let mut procedure = Procedure::new();
    let data = pdu(0x02, &[0x01, 0x00, 0x04, 0x00, 0xaa]);
    assert_eq!(receive(&mut procedure, &data), Some(data));
    assert!(procedure.is_idle());
}

#[test]
fn an_empty_pdu_is_skipped_and_advances_nothing() {
    let mut idle = Procedure::new();
    assert_eq!(receive(&mut idle, &pdu(0x01, &[])), None);
    assert!(idle.is_idle() && idle.termination().is_none());

    // Encrypted, an empty PDU carries no MIC and takes no counter.
    let mut procedure = active();
    let before = central_counter(&mut procedure);
    assert_eq!(receive(&mut procedure, &pdu(0x01, &[])), None);
    assert_eq!(central_counter(&mut procedure), before);
    assert!(procedure.is_active());
}

#[test]
fn a_length_that_disagrees_with_the_header_fails_for_good() {
    let mut procedure = Procedure::new();
    // The header says five payload bytes; three arrive.
    assert_eq!(receive(&mut procedure, &[0x02, 5, 1, 2, 3]), None);
    assert_eq!(procedure.termination(), Some(Termination::Exit));
    // The failed procedure stays failed for the rest of the event.
    assert_eq!(receive(&mut procedure, &pdu(0x02, &[1, 2, 3])), None);
    assert_eq!(procedure.termination(), Some(Termination::Exit));
    assert_eq!(receive(&mut procedure, &[0x02]), None);
}

#[test]
fn an_encryption_request_of_another_length_is_malformed() {
    for length in [1, 22, 24] {
        let mut request = vec![0x03; length];
        request[0] = 0x03;
        let mut procedure = Procedure::new();
        let mut decoded = [0xa5; PDU_CAPACITY];
        assert_eq!(
            decode(&mut procedure, &pdu(0x03, &request), &mut decoded, random),
            None,
            "{length}"
        );
        assert_eq!(procedure.termination(), Some(Termination::Exit), "{length}");
        assert!(decoded.iter().all(|byte| *byte == 0), "{length}");
    }
}

#[test]
fn an_encryption_request_begins_the_procedure_and_is_consumed() {
    let mut procedure = Procedure::new();
    assert_eq!(receive(&mut procedure, &pdu(0x03, &ENC_REQ)), None);
    assert!(!procedure.is_idle());
    assert!(procedure.termination().is_none());
    // Until its response is sent, nothing else may arrive.
    assert_eq!(procedure.receive_mode(), Mode::Blocked);
}

#[test]
fn an_encryption_request_without_a_random_fails() {
    let mut procedure = Procedure::new();
    let mut decoded = [0; PDU_CAPACITY];
    assert_eq!(
        decode(&mut procedure, &pdu(0x03, &ENC_REQ), &mut decoded, || None),
        None
    );
    assert_eq!(procedure.termination(), Some(Termination::Exit));
}

#[test]
fn any_packet_while_blocked_fails_the_procedure() {
    let mut procedure = Procedure::new();
    assert_eq!(receive(&mut procedure, &pdu(0x03, &ENC_REQ)), None);
    assert_eq!(procedure.receive_mode(), Mode::Blocked);
    assert_eq!(receive(&mut procedure, &pdu(0x02, &[1, 2, 3])), None);
    assert_eq!(procedure.termination(), Some(Termination::Exit));
}

#[test]
fn the_encrypted_start_response_is_consumed_and_activates_encryption() {
    let mut procedure = awaiting_start_response();
    assert_eq!(receive(&mut procedure, &pdu(0x0f, &START_ENC_RSP)), None);
    assert!(procedure.termination().is_none());
    procedure.response_enqueued().unwrap();
    assert!(procedure.is_active());
    assert_eq!(central_counter(&mut procedure), Some(1));
}

#[test]
fn a_plaintext_packet_where_the_encrypted_start_response_belongs_fails() {
    let mut procedure = awaiting_start_response();
    assert_eq!(receive(&mut procedure, &pdu(0x03, &[0x06])), None);
    assert_eq!(procedure.termination(), Some(Termination::Exit));
}

#[test]
fn active_encryption_dispatches_the_authenticated_plaintext() {
    let mut procedure = active();
    let plaintext = receive(&mut procedure, &pdu(0x0e, &CENTRAL_DATA)).unwrap();
    // The header stays; the length byte counts the plaintext, not the MIC.
    assert_eq!(plaintext[..2], [0x0e, 27]);
    assert_eq!(&plaintext[2..], b"\x17\x00cdefghijklmnopq1234567890");
    assert_eq!(central_counter(&mut procedure), Some(2));
}

#[test]
fn a_mic_failure_exits_the_connection() {
    let mut procedure = active();
    let mut corrupted = CENTRAL_DATA;
    corrupted[30] ^= 0x01;
    assert_eq!(receive(&mut procedure, &pdu(0x0e, &corrupted)), None);
    assert_eq!(procedure.termination(), Some(Termination::Exit));
}
