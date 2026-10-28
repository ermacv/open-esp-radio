#![allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]

use super::*;

// Bluetooth Core, Vol 6 Part C, Section 1: encryption sample data.
const LTK: [u8; 16] = [
    0xbf, 0x01, 0xfb, 0x9d, 0x4e, 0xf3, 0xbc, 0x36, 0xd8, 0x74, 0xf5, 0x39, 0x41, 0x38, 0x68, 0x4c,
];
const SKD: [u8; 16] = [
    0x13, 0x02, 0xf1, 0xe0, 0xdf, 0xce, 0xbd, 0xac, 0x79, 0x68, 0x57, 0x46, 0x35, 0x24, 0x13, 0x02,
];
const SESSION_KEY: [u8; 16] = [
    0x66, 0xc6, 0xc2, 0x27, 0x8e, 0x3b, 0x8e, 0x05, 0x3e, 0x7e, 0xa3, 0x26, 0x52, 0x1b, 0xad, 0x99,
];
const IV: [u8; 8] = [0x24, 0xab, 0xdc, 0xba, 0xbe, 0xba, 0xaf, 0xde];

fn session_key() -> LeAclSessionKey {
    LeLongTermKey::new(LTK).derive_session_key(LeSessionKeyDiversifier::new(SKD))
}

#[test]
fn session_key_matches_the_core_sample() {
    assert_eq!(session_key().0, SESSION_KEY);
}

#[test]
fn keys_support_explicit_zeroization() {
    let mut ltk = LeLongTermKey::new(LTK);
    let mut key = session_key();
    ltk.zeroize();
    key.zeroize();
    assert_eq!(ltk.0, [0; 16]);
    assert_eq!(key.0, [0; 16]);
}

#[test]
fn nonce_encodes_supplied_counter_direction_and_iv() {
    let iv = LeAclInitializationVector::new(IV);
    assert_eq!(
        iv.nonce(0, false),
        Ok([
            0, 0, 0, 0, 0, 0x24, 0xab, 0xdc, 0xba, 0xbe, 0xba, 0xaf, 0xde
        ])
    );
    assert_eq!(
        iv.nonce(0x0102030405, true),
        Ok([
            5, 4, 3, 2, 0x81, 0x24, 0xab, 0xdc, 0xba, 0xbe, 0xba, 0xaf, 0xde
        ])
    );
    assert_eq!(
        iv.nonce(LE_PACKET_COUNTER_MAX, false),
        Ok([
            0xff, 0xff, 0xff, 0xff, 0x7f, 0x24, 0xab, 0xdc, 0xba, 0xbe, 0xba, 0xaf, 0xde
        ])
    );
    for counter in [LE_PACKET_COUNTER_MAX + 1, u64::MAX] {
        assert_eq!(
            iv.nonce(counter, false),
            Err(LeAclEncryptionError::PacketCounterExhausted)
        );
    }
}

#[test]
fn peripheral_and_central_core_vectors_use_supplied_counters() {
    let key = session_key();
    let iv = LeAclInitializationVector::new(IV);
    let mut peripheral_start = [0x06, 0, 0, 0, 0];
    assert_eq!(
        encrypt_packet(&key, &iv, 0, false, 0x07, &mut peripheral_start, 1),
        Ok(5)
    );
    assert_eq!(peripheral_start, [0xa3, 0x4c, 0x13, 0xa4, 0x15]);

    let mut central_start = [0x9f, 0xcd, 0xa7, 0xf4, 0x48];
    assert_eq!(
        decrypt_packet(&key, &iv, 0, true, 0x0f, &mut central_start),
        Ok(1)
    );
    assert_eq!(central_start, [0x06, 0, 0, 0, 0]);

    let mut peripheral_data = [0; 31];
    peripheral_data[..27].copy_from_slice(b"\x17\x0076543210ABCDEFGHIJKLMNOPQ");
    assert_eq!(
        encrypt_packet(&key, &iv, 1, false, 0x06, &mut peripheral_data, 27),
        Ok(31)
    );
    assert_eq!(
        peripheral_data,
        [
            0xf3, 0x88, 0x81, 0xe7, 0xbd, 0x94, 0xc9, 0xc3, 0x69, 0xb9, 0xa6, 0x68, 0x46, 0xdd,
            0x47, 0x86, 0xaa, 0x8c, 0x39, 0xce, 0x54, 0x0d, 0x0d, 0xae, 0x3a, 0xdc, 0xdf, 0x89,
            0xb9, 0x60, 0x88,
        ]
    );
    let mut central_data = [
        0x7a, 0x70, 0xd6, 0x64, 0x15, 0x22, 0x6d, 0xf2, 0x6b, 0x17, 0x83, 0x9a, 0x06, 0x04, 0x05,
        0x59, 0x6b, 0xd6, 0x56, 0x4f, 0x79, 0x6b, 0x5b, 0x9c, 0xe6, 0xff, 0x32, 0xf7, 0x5a, 0x6d,
        0x33,
    ];
    assert_eq!(
        decrypt_packet(&key, &iv, 1, true, 0x0e, &mut central_data),
        Ok(27)
    );
    assert_eq!(&central_data[..27], b"\x17\x00cdefghijklmnopq1234567890");
    assert_eq!(&central_data[27..], &[0; 4]);
}

#[test]
fn sequence_bits_do_not_enter_aad() {
    let key = session_key();
    let iv = LeAclInitializationVector::new(IV);
    for header in [0x03, 0x07, 0x0b, 0x0f, 0x13, 0x17, 0x1b, 0x1f] {
        let mut packet = [0x06, 0, 0, 0, 0];
        assert_eq!(
            encrypt_packet(&key, &iv, 0, false, header, &mut packet, 1),
            Ok(5)
        );
        assert_eq!(packet, [0xa3, 0x4c, 0x13, 0xa4, 0x15]);
    }
}

#[test]
fn wrong_mic_clears_complete_receive_payload() {
    let key = session_key();
    let iv = LeAclInitializationVector::new(IV);
    // Tamper with the ciphertext and with each of the four MIC octets.
    for index in 0..5 {
        let mut packet = [0x9f, 0xcd, 0xa7, 0xf4, 0x48];
        packet[index] ^= 1;
        assert_eq!(
            decrypt_packet(&key, &iv, 0, true, 0x0f, &mut packet),
            Err(LeAclEncryptionError::MicMismatch)
        );
        assert_eq!(packet, [0; 5]);
    }
}

#[test]
fn direction_counter_and_authenticated_header_are_not_interchangeable() {
    let key = session_key();
    let iv = LeAclInitializationVector::new(IV);
    for (counter, central, header) in [
        (1, true, 0x03),
        (0, false, 0x03),
        (0, true, 0x02),
        (0, true, 0x23),
    ] {
        let mut packet = [0x9f, 0xcd, 0xa7, 0xf4, 0x48];
        assert_eq!(
            decrypt_packet(&key, &iv, counter, central, header, &mut packet),
            Err(LeAclEncryptionError::MicMismatch)
        );
        assert_eq!(packet, [0; 5]);
    }
}

#[test]
fn empty_pdus_bypass_ccm_without_touching_storage() {
    let key = session_key();
    let iv = LeAclInitializationVector::new(IV);
    for header in [0x01, 0x05, 0x09, 0x1d] {
        let mut storage = [0xa5; 4];
        assert_eq!(
            encrypt_packet(&key, &iv, u64::MAX, false, header, &mut storage, 0),
            Ok(0)
        );
        assert_eq!(storage, [0xa5; 4]);
        assert_eq!(
            decrypt_packet(&key, &iv, u64::MAX, true, header, &mut []),
            Ok(0)
        );
    }
}

#[test]
fn empty_exception_rejects_control_headers_and_mic_only_packets() {
    let key = session_key();
    let iv = LeAclInitializationVector::new(IV);
    for header in [0x00, 0x02, 0x03] {
        assert_eq!(
            encrypt_packet(&key, &iv, 0, false, header, &mut [], 0),
            Err(LeAclEncryptionError::InvalidEmptyPdu)
        );
        assert_eq!(
            decrypt_packet(&key, &iv, 0, true, header, &mut []),
            Err(LeAclEncryptionError::InvalidEmptyPdu)
        );
    }
    // Even a correctly calculated CCM tag for zero payload is not an ACL PDU.
    let mut tag = [0; 4];
    encrypt_ccm(&SESSION_KEY, &IV, 0, true, 0x01, &mut tag, 0).unwrap();
    assert_eq!(
        decrypt_packet(&key, &iv, 0, true, 0x01, &mut tag),
        Err(LeAclEncryptionError::BufferTooSmall)
    );
}

#[test]
fn invalid_extents_and_counters_leave_storage_unchanged() {
    let key = session_key();
    let iv = LeAclInitializationVector::new(IV);
    let mut short = [0xa5; 4];
    assert_eq!(
        encrypt_packet(&key, &iv, 0, false, 0x01, &mut short, 1),
        Err(LeAclEncryptionError::BufferTooSmall)
    );
    assert_eq!(short, [0xa5; 4]);
    let mut long = [0xa5; 256];
    for length in [LE_ENCRYPTED_PLAINTEXT_BYTES + 1, usize::MAX] {
        assert_eq!(
            encrypt_packet(&key, &iv, 0, false, 0x01, &mut long, length),
            Err(LeAclEncryptionError::PayloadTooLong)
        );
        assert_eq!(long, [0xa5; 256]);
    }
    assert_eq!(
        decrypt_packet(&key, &iv, 0, true, 0x01, &mut long),
        Err(LeAclEncryptionError::PayloadTooLong)
    );
    assert_eq!(long, [0xa5; 256]);
    for length in 1..=LE_ACL_MIC_BYTES {
        assert_eq!(
            decrypt_packet(&key, &iv, 0, true, 0x01, &mut short[..length]),
            Err(LeAclEncryptionError::BufferTooSmall)
        );
        assert_eq!(short, [0xa5; 4]);
    }
    let mut packet = [0xa5; 5];
    for counter in [LE_PACKET_COUNTER_MAX + 1, u64::MAX] {
        assert_eq!(
            encrypt_packet(&key, &iv, counter, false, 0x01, &mut packet, 1),
            Err(LeAclEncryptionError::PacketCounterExhausted)
        );
        assert_eq!(packet, [0xa5; 5]);
        assert_eq!(
            decrypt_packet(&key, &iv, counter, true, 0x01, &mut packet),
            Err(LeAclEncryptionError::PacketCounterExhausted)
        );
        assert_eq!(packet, [0xa5; 5]);
    }
}

#[test]
fn largest_plaintext_round_trips_at_the_last_counter_in_both_directions() {
    let key = session_key();
    let iv = LeAclInitializationVector::new(IV);
    for central in [false, true] {
        let mut packet = [0xa5; LE_ENCRYPTED_PLAINTEXT_BYTES + LE_ACL_MIC_BYTES + 2];
        assert_eq!(
            encrypt_packet(
                &key,
                &iv,
                LE_PACKET_COUNTER_MAX,
                central,
                0x02,
                &mut packet,
                LE_ENCRYPTED_PLAINTEXT_BYTES
            ),
            Ok(255)
        );
        assert_eq!(&packet[255..], &[0xa5; 2]);
        assert_eq!(
            decrypt_packet(
                &key,
                &iv,
                LE_PACKET_COUNTER_MAX,
                central,
                0x02,
                &mut packet[..255]
            ),
            Ok(251)
        );
        assert_eq!(&packet[..251], &[0xa5; 251]);
        assert_eq!(&packet[251..255], &[0; 4]);
        assert_eq!(&packet[255..], &[0xa5; 2]);
    }
}
