#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::panic,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing
)]
//! The shared conformance cases of every AES-128 engine's RFC 3394 unwrap
//! ([`crate::block::unwrap_now`]): [`key_unwrap_rfc3394`]. An engine's tests
//! call it with the engine; an engine that breaks a case panics with what it
//! observed, so the test fails.

use crate::{
    Aes128Key, KeyData, KeyUnwrapError, KeyUnwrapRequest,
    block::{Aes128Engine, unwrap_now},
};

/// The key-encryption key of RFC 3394 §4.1.
pub const RFC3394_4_1_KEK: [u8; 16] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
];
/// The key data of RFC 3394 §4.1.
pub const RFC3394_4_1_KEY_DATA: [u8; 16] = [
    0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
];
/// The ciphertext of RFC 3394 §4.1.
pub const RFC3394_4_1_CIPHERTEXT: [u8; 24] = [
    0x1f, 0xa6, 0x8b, 0x0a, 0x81, 0x12, 0xb4, 0x47, 0xae, 0xf3, 0x4b, 0xd8, 0xfb, 0x5a, 0x7b, 0x82,
    0x9d, 0x3e, 0x86, 0x23, 0x71, 0xd2, 0xcf, 0xe5,
];

fn request<const N: usize>(wrapped: &[u8]) -> KeyUnwrapRequest<N> {
    KeyUnwrapRequest::new(
        Aes128Key::copy_from(&RFC3394_4_1_KEK),
        KeyData::try_copy(wrapped).expect("the case fits the engine's buffer"),
    )
}

/// An AES-128 RFC 3394 unwrap engine of buffers of `N >= 32` bytes:
/// - unwraps the §4.1 vector to its key data;
/// - refuses a changed ciphertext as `IntegrityCheckFailed` with no plaintext;
/// - refuses data of fewer than three semiblocks, or not semiblocks, as
///   `InvalidLength`;
/// - takes the next request after a refusal.
///
/// # Panics
///
/// The engine breaks a case.
pub fn key_unwrap_rfc3394<E: Aes128Engine, const N: usize>(engine: &mut E) {
    const {
        if N < 32 {
            panic!("the conformance cases need 32-byte buffers");
        }
    }
    let response = unwrap_now(engine, request::<N>(&RFC3394_4_1_CIPHERTEXT));
    assert_eq!(
        response.plaintext(),
        Ok(&RFC3394_4_1_KEY_DATA[..]),
        "RFC 3394 §4.1 unwraps to its key data"
    );

    let mut changed = RFC3394_4_1_CIPHERTEXT;
    changed[23] ^= 1;
    let response = unwrap_now(engine, request::<N>(&changed));
    assert_eq!(
        response.plaintext(),
        Err(KeyUnwrapError::IntegrityCheckFailed),
        "a changed ciphertext fails the integrity check"
    );

    let response = unwrap_now(engine, request::<N>(&RFC3394_4_1_CIPHERTEXT[..16]));
    assert_eq!(
        response.plaintext(),
        Err(KeyUnwrapError::InvalidLength),
        "one semiblock of key data is outside RFC 3394"
    );

    let mut odd = [0; 25];
    odd[..24].copy_from_slice(&RFC3394_4_1_CIPHERTEXT);
    let response = unwrap_now(engine, request::<N>(&odd));
    assert_eq!(
        response.plaintext(),
        Err(KeyUnwrapError::InvalidLength),
        "data that is not semiblocks is refused"
    );

    let response = unwrap_now(engine, request::<N>(&RFC3394_4_1_CIPHERTEXT));
    assert_eq!(
        response
            .into_plaintext()
            .map(|data| data.as_bytes() == RFC3394_4_1_KEY_DATA),
        Ok(true),
        "the engine serves its next request after a refusal"
    );
}
