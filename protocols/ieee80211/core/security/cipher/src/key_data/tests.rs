use super::*;
use core::convert::Infallible;
use oer_portable_aes_software::SoftwareAes128;
use oer_portable_crypto::Aes128Key;
use oer_portable_crypto::{
    conformance,
    rfc3394::{KeyUnwrapError, KeyWrapError, unwrap, wrap},
};
use std::vec::Vec;

/// RFC 3394 unwrap over the software engine in a step's buffer: the
/// plaintext.
fn software_unwrap(kek: &[u8; 16], wrapped: &[u8]) -> Result<Vec<u8>, KeyUnwrapError<Infallible>> {
    let mut buffer = RsnKeyData::new();
    let wrapped = buffer.wrapped(wrapped).unwrap();
    unwrap(&mut SoftwareAes128, &Aes128Key::copy_from(kek), wrapped).map(|plain| plain.to_vec())
}

/// RFC 3394 wrap over the software engine in a step's buffer, as a
/// composition runs it: the ciphertext.
fn software_wrap(kek: &[u8; 16], plaintext: &[u8]) -> Result<Vec<u8>, KeyWrapError<Infallible>> {
    let mut buffer = RsnKeyData::new();
    let buffer = buffer.plaintext(plaintext).unwrap();
    wrap(&mut SoftwareAes128, &Aes128Key::copy_from(kek), buffer)?;
    Ok(buffer.to_vec())
}

#[test]
fn wraps_rfc3394_vector_and_round_trips_through_unwrap() {
    let wrapped = software_wrap(
        &conformance::RFC3394_4_1_KEK,
        &conformance::RFC3394_4_1_KEY_DATA,
    )
    .unwrap();
    assert_eq!(wrapped, conformance::RFC3394_4_1_CIPHERTEXT);
    let unwrapped = software_unwrap(&conformance::RFC3394_4_1_KEK, &wrapped).unwrap();
    assert_eq!(unwrapped, conformance::RFC3394_4_1_KEY_DATA);
}

#[test]
fn wrap_rejects_non_rfc3394_lengths() {
    assert_eq!(
        software_wrap(&[0; 16], &[0; 8]).err(),
        Some(KeyWrapError::InvalidLength)
    );
    assert_eq!(
        software_wrap(&[0; 16], &[0; 17]).err(),
        Some(KeyWrapError::InvalidLength)
    );
}

#[test]
fn rfc3394_uses_the_full_bounded_buffer_and_refuses_the_next_block() {
    let plaintext = [0x91; RSN_KEY_DATA_CAPACITY - 8];
    let wrapped = software_wrap(&[0x27; 16], &plaintext).unwrap();
    assert_eq!(wrapped.len(), RSN_KEY_DATA_CAPACITY);
    assert_eq!(software_unwrap(&[0x27; 16], &wrapped).unwrap(), plaintext);
    let mut buffer = RsnKeyData::new();
    assert_eq!(
        buffer.plaintext(&[0; RSN_KEY_DATA_CAPACITY - 7]).err(),
        Some(RsnKeyDataTooLong)
    );
    assert_eq!(
        buffer.wrapped(&[0; RSN_KEY_DATA_CAPACITY + 1]).err(),
        Some(RsnKeyDataTooLong)
    );
    assert_eq!(
        buffer
            .wrapped(&[0; RSN_KEY_DATA_CAPACITY])
            .map(|bytes| bytes.len()),
        Ok(RSN_KEY_DATA_CAPACITY)
    );
}

/// The buffer lends the bytes it was given, after a semiblock of room for a
/// plaintext; a refused unwrap leaves those bytes cleared.
#[test]
fn the_buffer_lends_what_it_was_given_and_a_refusal_clears_it() {
    let mut buffer = RsnKeyData::new();
    let plaintext = buffer.plaintext(&[7; 16]).unwrap();
    assert_eq!(plaintext.len(), 24);
    assert_eq!(plaintext[8..], [7; 16]);
    let mut flipped = conformance::RFC3394_4_1_CIPHERTEXT;
    flipped[0] ^= 1;
    let mut buffer = RsnKeyData::new();
    let wrapped = buffer.wrapped(&flipped).unwrap();
    assert_eq!(
        unwrap(
            &mut SoftwareAes128,
            &Aes128Key::copy_from(&conformance::RFC3394_4_1_KEK),
            wrapped
        )
        .err(),
        Some(KeyUnwrapError::IntegrityCheckFailed)
    );
    assert!(buffer.0.iter().all(|byte| *byte == 0));
}
