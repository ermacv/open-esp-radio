//! The station's one RFC 3394 key-data unwrap in flight.
//!
//! The station owns one [`KeyUnwrapId`]. A state that needs its EAPOL-Key
//! data unwrapped moves it out in [`super::StaAction::UnwrapKeyData`] with the
//! owned request; the response returns it in
//! [`super::StaInput::KeyDataUnwrapped`]. While it is out the station issues
//! no other request, so a service needs exactly one slot. Each issue advances
//! the ticket's sequence; the waiting state retains the sequence it issued,
//! and a response for any other (an earlier handshake, a left association)
//! returns the ticket without a protocol effect.

use oer_ieee80211_cipher::aes::RSN_KEY_DATA_CAPACITY;

/// The capacity of the station's key-data unwrap buffers.
pub const KEY_DATA_CAPACITY: usize = RSN_KEY_DATA_CAPACITY;

/// The owned unwrap the station asks for.
pub type StaKeyUnwrapRequest = oer_crypto::KeyUnwrapRequest<KEY_DATA_CAPACITY>;

/// The response that completes it.
pub type StaKeyUnwrapResponse = oer_crypto::KeyUnwrapResponse<KEY_DATA_CAPACITY>;

/// The station's sole key-unwrap ticket: neither `Clone` nor `Copy`, and
/// constructed only by the station.
#[must_use = "return the ticket with the unwrap's response"]
#[derive(Debug)]
pub struct KeyUnwrapId {
    sequence: u32,
}

impl KeyUnwrapId {
    /// A ticket of no station, for a service test's engine.
    #[cfg(any(test, feature = "test-support"))]
    pub const fn for_test() -> Self {
        Self { sequence: 0 }
    }
}

/// Where the ticket is: home, or out with one request.
pub(super) struct KeyUnwrapLane {
    held: Option<KeyUnwrapId>,
}

impl KeyUnwrapLane {
    pub(super) const fn new() -> Self {
        Self {
            held: Some(KeyUnwrapId { sequence: 0 }),
        }
    }

    /// Issue the ticket for a new request with its sequence, or nothing while
    /// a request is out.
    pub(super) fn issue(&mut self) -> Option<(KeyUnwrapId, u32)> {
        let mut id = self.held.take()?;
        id.sequence = id.sequence.wrapping_add(1);
        let sequence = id.sequence;
        Some((id, sequence))
    }

    /// Take the ticket back and name the request it was issued for.
    pub(super) fn complete(&mut self, id: KeyUnwrapId) -> u32 {
        let sequence = id.sequence;
        self.held = Some(id);
        sequence
    }

    /// A request is out.
    pub(super) fn in_flight(&self) -> bool {
        self.held.is_none()
    }
}
