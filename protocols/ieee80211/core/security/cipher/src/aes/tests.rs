use super::*;
use oer_portable_crypto::{KeyUnwrapError, KeyUnwrapRequest, conformance};

fn unwrap(kek: &[u8; 16], wrapped: &[u8]) -> Result<RsnKeyData, KeyUnwrapError> {
    oer_portable_crypto::block::unwrap_now(
        &mut SoftwareAes128,
        KeyUnwrapRequest::new(
            Aes128Key::copy_from(kek),
            RsnKeyData::try_copy(wrapped).unwrap(),
        ),
    )
    .into_plaintext()
}

#[test]
fn the_software_engine_passes_the_key_unwrap_conformance_cases() {
    conformance::key_unwrap_rfc3394::<_, RSN_KEY_DATA_CAPACITY>(&mut SoftwareAes128);
}

#[test]
fn wraps_rfc3394_vector_and_round_trips_through_unwrap() {
    let wrapped = software_aes128_key_wrap(
        &conformance::RFC3394_4_1_KEK,
        &conformance::RFC3394_4_1_KEY_DATA,
    )
    .unwrap();
    assert_eq!(wrapped.as_bytes(), conformance::RFC3394_4_1_CIPHERTEXT);
    let unwrapped = unwrap(&conformance::RFC3394_4_1_KEK, wrapped.as_bytes()).unwrap();
    assert_eq!(unwrapped.as_bytes(), conformance::RFC3394_4_1_KEY_DATA);
}

#[test]
fn wrap_rejects_non_rfc3394_lengths() {
    assert_eq!(
        software_aes128_key_wrap(&[0; 16], &[0; 8]).err(),
        Some(KeyWrapError::InvalidLength)
    );
    assert_eq!(
        software_aes128_key_wrap(&[0; 16], &[0; 17]).err(),
        Some(KeyWrapError::InvalidLength)
    );
}

#[test]
fn rfc3394_uses_the_full_bounded_buffer_and_refuses_the_next_block() {
    let plaintext = [0x91; RSN_KEY_DATA_CAPACITY - 8];
    let wrapped = software_aes128_key_wrap(&[0x27; 16], &plaintext).unwrap();
    assert_eq!(wrapped.as_bytes().len(), RSN_KEY_DATA_CAPACITY);
    let unwrapped = unwrap(&[0x27; 16], wrapped.as_bytes()).unwrap();
    assert_eq!(unwrapped.as_bytes(), plaintext);
    assert_eq!(
        software_aes128_key_wrap(&[0; 16], &[0; RSN_KEY_DATA_CAPACITY]).err(),
        Some(KeyWrapError::CapacityExceeded)
    );
    assert!(RsnKeyData::try_copy(&[0; RSN_KEY_DATA_CAPACITY + 8]).is_err());
}
