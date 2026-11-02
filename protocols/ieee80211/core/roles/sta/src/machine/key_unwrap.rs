//! The station's RFC 3394 key-data unwrap: in its step, a few AES blocks per
//! Message 3 or Group Message 1, over the synchronous AES-128 engine the
//! composition supplies ([`super::StaCallbacks::KeyUnwrapEngine`]).

use oer_crypto::block::Aes128Engine;
use oer_ieee80211_core_security_cipher::aes::RSN_KEY_DATA_CAPACITY;

/// Unwrap `request` to completion over `engine`.
pub(super) fn unwrap_now<E: Aes128Engine>(
    engine: &mut E,
    request: oer_crypto::KeyUnwrapRequest<RSN_KEY_DATA_CAPACITY>,
) -> oer_crypto::KeyUnwrapResponse<RSN_KEY_DATA_CAPACITY> {
    oer_crypto::block::unwrap_now(engine, request)
}
