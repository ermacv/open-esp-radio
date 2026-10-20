use super::*;

#[test]
fn unwraps_rfc3394_vector_and_rejects_changed_integrity() {
    let kek = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f,
    ];
    let encrypted = [
        0x1f, 0xa6, 0x8b, 0x0a, 0x81, 0x12, 0xb4, 0x47, 0xae, 0xf3, 0x4b, 0xd8, 0xfb, 0x5a, 0x7b,
        0x82, 0x9d, 0x3e, 0x86, 0x23, 0x71, 0xd2, 0xcf, 0xe5,
    ];
    let expected = [
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        0xff,
    ];
    let plain = software_aes128_key_unwrap(&kek, &encrypted).unwrap();
    assert_eq!(plain.as_bytes(), &expected);

    let mut changed = encrypted;
    changed[23] ^= 1;
    assert_eq!(
        software_aes128_key_unwrap(&kek, &changed).err(),
        Some(SoftwareAesKeyUnwrapError::IntegrityCheckFailed)
    );
}

#[test]
fn wraps_rfc3394_vector_and_round_trips_through_unwrap() {
    let kek = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f,
    ];
    let plaintext = [
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        0xff,
    ];
    let expected = [
        0x1f, 0xa6, 0x8b, 0x0a, 0x81, 0x12, 0xb4, 0x47, 0xae, 0xf3, 0x4b, 0xd8, 0xfb, 0x5a, 0x7b,
        0x82, 0x9d, 0x3e, 0x86, 0x23, 0x71, 0xd2, 0xcf, 0xe5,
    ];
    let wrapped = software_aes128_key_wrap(&kek, &plaintext).unwrap();
    assert_eq!(wrapped.as_bytes(), &expected);
    let unwrapped = software_aes128_key_unwrap(&kek, wrapped.as_bytes()).unwrap();
    assert_eq!(unwrapped.as_bytes(), &plaintext);
}

#[test]
fn wrap_rejects_non_rfc3394_lengths() {
    assert_eq!(
        software_aes128_key_wrap(&[0; 16], &[0; 8]).err(),
        Some(SoftwareAesKeyWrapError::InvalidLength)
    );
    assert_eq!(
        software_aes128_key_wrap(&[0; 16], &[0; 17]).err(),
        Some(SoftwareAesKeyWrapError::InvalidLength)
    );
}

#[test]
fn rejects_non_rfc3394_lengths() {
    assert_eq!(
        software_aes128_key_unwrap(&[0; 16], &[0; 16]).err(),
        Some(SoftwareAesKeyUnwrapError::InvalidLength)
    );
    assert_eq!(
        software_aes128_key_unwrap(&[0; 16], &[0; 25]).err(),
        Some(SoftwareAesKeyUnwrapError::InvalidLength)
    );
}

#[test]
fn external_unwrap_backends_can_construct_only_bounded_plaintext() {
    let bytes = [0x5a; RSN_UNWRAPPED_KEY_DATA_CAPACITY];
    let owned = RsnUnwrappedKeyData::try_copy(&bytes).unwrap();
    assert_eq!(owned.as_bytes(), &bytes);

    let oversized = [0; RSN_UNWRAPPED_KEY_DATA_CAPACITY + 1];
    assert_eq!(
        RsnUnwrappedKeyData::try_copy(&oversized).err(),
        Some(RsnUnwrappedKeyDataCapacityError)
    );
}
