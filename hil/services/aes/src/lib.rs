#![no_std]
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]
//! The HIL AES service: one endpoint unwraps RFC 3394 key data under an
//! AES-128 key-encryption key with the image's product engine.
//!
//! The messages are shared by the image, which serves them through
//! [`Request`] and [`Engine`], and the host scenario, which sends them and
//! checks the answers against its own reference.

use oer_portable_crypto::{
    Aes128Key, KeyData, KeyUnwrapError, KeyUnwrapRequest,
    block::{Aes128Engine, EngineStopped, unwrap_now},
};
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

/// The largest wrapped input: 256-bit key data and its 64-bit integrity
/// block (RFC 3394 §2.2.1).
pub const MAX_WRAPPED: usize = 40;

oer_protocols_hil_protocol::messages! {
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
    /// The engine stopped. `last` is what its driver's record holds of the
    /// last stop: a stop the driver did not record reads `None`.
    EngineStopped { last: Option<EngineObservation> },
}

/// What an engine's driver observed when it stopped: the record the image
/// reads from its engine and the service answers.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum EngineObservation {
    /// A block did not complete within the engine's budget; the status it
    /// last read.
    Timeout { observation: u32 },
    /// A status outside the operation's domain.
    InvalidState { observation: u32 },
}

oer_protocols_hil_agent::service! {
    /// The AES service's requests.
    pub enum Request (sessions = false, observer = Observer, commands = Command, commander = Commander) {
        UnwrapKey(UnwrapKey) => command unwrap_key,
    }
}

/// The handler of the service over a product AES-128 engine `E`, which it
/// lends to each unwrap for the length of the call, and `record`, which
/// reads what `E`'s driver observed at its last stop.
pub struct Engine<E, R> {
    engine: E,
    record: R,
}

impl<E, R: Fn(&E) -> Option<EngineObservation>> Engine<E, R> {
    pub const fn new(engine: E, record: R) -> Self {
        Self { engine, record }
    }
}

impl<E, R> Commander for Engine<E, R>
where
    E: Aes128Engine<Stop = EngineStopped>,
    R: Fn(&E) -> Option<EngineObservation>,
{
    async fn unwrap_key(&mut self, request: UnwrapKey) -> Unwrapped {
        // A request never exceeds the buffer: both hold MAX_WRAPPED bytes.
        let Ok(wrapped) = KeyData::<MAX_WRAPPED>::try_copy(&request.wrapped) else {
            return Unwrapped::Refused(UnwrapError::InvalidLength);
        };
        let request = KeyUnwrapRequest::new(Aes128Key::copy_from(&request.kek), wrapped);
        let response = unwrap_now(&mut self.engine, request);
        match response.plaintext() {
            Ok(plaintext) => match heapless::Vec::from_slice(plaintext) {
                Ok(plaintext) => Unwrapped::Plaintext(plaintext),
                Err(_) => Unwrapped::Refused(UnwrapError::InvalidLength),
            },
            Err(KeyUnwrapError::InvalidLength) => Unwrapped::Refused(UnwrapError::InvalidLength),
            Err(KeyUnwrapError::IntegrityCheckFailed) => {
                Unwrapped::Refused(UnwrapError::IntegrityCheckFailed)
            }
            Err(KeyUnwrapError::Engine(EngineStopped)) => {
                Unwrapped::Refused(UnwrapError::EngineStopped {
                    last: (self.record)(&self.engine),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests;
