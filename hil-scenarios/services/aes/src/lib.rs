#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::arithmetic_side_effects,
        clippy::disallowed_macros,
        clippy::disallowed_methods
    )
)]
//! The HIL AES service: one endpoint unwraps RFC 3394 key data under an
//! AES-128 key-encryption key with the image's product engine.
//!
//! The messages are shared by the image, which serves them through
//! [`Request`] and [`Engine`], and the host scenario, which sends them and
//! checks the answers against its own reference.

use oer_crypto::{
    Aes128Key, KeyData, KeyUnwrap, KeyUnwrapError, KeyUnwrapRequest, block::BlockError,
};
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

/// The largest wrapped input: 256-bit key data and its 64-bit integrity
/// block (RFC 3394 §2.2.1).
pub const MAX_WRAPPED: usize = 40;

oer_hil_protocol::messages! {
    endpoint UnwrapKey = "aes/key-unwrap" => Unwrapped;
    topic Unwrapped = "aes/key-unwrapped";
}

/// Unwrap `wrapped` under `kek`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct UnwrapKey {
    pub kek: [u8; 16],
    pub wrapped: heapless::Vec<u8, MAX_WRAPPED>,
}

/// The answer to [`UnwrapKey`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Unwrapped {
    /// The plaintext key data.
    Plaintext(heapless::Vec<u8, MAX_WRAPPED>),
    Refused(UnwrapError),
}

/// Why the image refused an unwrap.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum UnwrapError {
    /// Not 64-bit blocks with at least two blocks of key data (RFC 3394 §2).
    InvalidLength,
    /// The initial value did not verify (RFC 3394 §2.2.3).
    IntegrityCheckFailed,
    /// The engine retains a failed resource and accepts no more blocks.
    EngineUnavailable,
    /// A block did not complete within the engine's budget.
    EngineTimeout { observation: u32 },
    /// The engine observed a status outside the operation's domain.
    EngineInvalidState { observation: u32 },
    /// The engine's clock cannot represent its bound.
    EngineTimeOverflow,
}

impl From<KeyUnwrapError> for UnwrapError {
    fn from(error: KeyUnwrapError) -> Self {
        match error {
            KeyUnwrapError::InvalidLength => Self::InvalidLength,
            KeyUnwrapError::IntegrityCheckFailed => Self::IntegrityCheckFailed,
            KeyUnwrapError::Engine(BlockError::Unavailable) => Self::EngineUnavailable,
            KeyUnwrapError::Engine(BlockError::Timeout { observation }) => {
                Self::EngineTimeout { observation }
            }
            KeyUnwrapError::Engine(BlockError::InvalidState { observation }) => {
                Self::EngineInvalidState { observation }
            }
            KeyUnwrapError::Engine(BlockError::TimeOverflow) => Self::EngineTimeOverflow,
        }
    }
}

oer_hil_agent::service! {
    /// The AES service's requests.
    pub enum Request (sessions = false, handler = Handler) {
        UnwrapKey(UnwrapKey) => unwrap_key,
    }
}

/// The handler of the service over a product engine `E`, which moves into
/// each operation and comes back with its response.
pub struct Engine<E> {
    engine: Option<E>,
}

impl<E> Engine<E> {
    pub const fn new(engine: E) -> Self {
        Self {
            engine: Some(engine),
        }
    }
}

impl<E: KeyUnwrap<MAX_WRAPPED>> Handler for Engine<E> {
    async fn unwrap_key(&mut self, request: UnwrapKey) -> Unwrapped {
        // A request never exceeds the engine's buffer: both are MAX_WRAPPED.
        let Ok(wrapped) = KeyData::<MAX_WRAPPED>::try_copy(&request.wrapped) else {
            return Unwrapped::Refused(UnwrapError::InvalidLength);
        };
        // An engine lost with an unfinished operation stays lost.
        let Some(engine) = self.engine.take() else {
            return Unwrapped::Refused(UnwrapError::EngineUnavailable);
        };
        let request = KeyUnwrapRequest::new(Aes128Key::copy_from(&request.kek), wrapped);
        let done = engine.unwrap_key(request).await;
        self.engine = Some(done.engine);
        match done.response.plaintext() {
            Ok(plaintext) => match heapless::Vec::from_slice(plaintext) {
                Ok(plaintext) => Unwrapped::Plaintext(plaintext),
                Err(_) => Unwrapped::Refused(UnwrapError::InvalidLength),
            },
            Err(error) => Unwrapped::Refused(error.into()),
        }
    }
}

/// Reviewed paths, schemas (through keys) and message kinds.
#[cfg(feature = "descriptors")]
pub const MESSAGES_LOCK: &str = include_str!("../messages.lock");

#[cfg(test)]
mod tests;
