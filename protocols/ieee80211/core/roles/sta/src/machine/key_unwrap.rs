//! The station's RFC 3394 key-data unwrap: in its step, a few AES blocks per
//! Message 3 or Group Message 1, over the synchronous AES-128 block
//! primitive the composition supplies ([`super::StaCallbacks::KeyUnwrapBlocks`]).

use oer_crypto::block::{Aes128Blocks, BlockKeyUnwrap};
use oer_ieee80211_core_security_cipher::aes::RSN_KEY_DATA_CAPACITY;

/// Unwrap `request` to completion over `blocks`.
pub(super) fn unwrap_now<B: Aes128Blocks>(
    blocks: &mut B,
    request: oer_crypto::KeyUnwrapRequest<RSN_KEY_DATA_CAPACITY>,
) -> oer_crypto::KeyUnwrapResponse<RSN_KEY_DATA_CAPACITY> {
    BlockKeyUnwrap::new(blocks).unwrap_now(request)
}
