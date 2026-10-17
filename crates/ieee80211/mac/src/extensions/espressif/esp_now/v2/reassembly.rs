//! Fixed-capacity materialization of a validated ESP-NOW v2 datagram.

use super::{ESP_NOW_V2_MAX_PAYLOAD_LEN, EspNowV2Action, EspNowV2WireError};

/// Caller-owned, fixed-capacity storage for a reassembled v2 datagram.
///
/// The default capacity accepts the complete public v2 payload. Applications
/// may select a smaller const capacity and receive a fail-closed capacity
/// error before the previous contents are modified.
pub struct EspNowV2Reassembly<const N: usize = ESP_NOW_V2_MAX_PAYLOAD_LEN> {
    bytes: [u8; N],
    length: usize,
}

impl<const N: usize> EspNowV2Reassembly<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            length: 0,
        }
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    pub const fn len(&self) -> usize {
        self.length
    }

    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    pub fn payload(&self) -> &[u8] {
        &self.bytes[..self.length]
    }

    pub fn reassemble(&mut self, action: EspNowV2Action<'_>) -> Result<&[u8], EspNowV2WireError> {
        if N < action.payload_len() {
            return Err(EspNowV2WireError::ReassemblyCapacityTooSmall {
                required: action.payload_len(),
                capacity: N,
            });
        }
        let length = action.copy_payload(&mut self.bytes)?;
        self.length = length;
        Ok(&self.bytes[..length])
    }

    pub fn clear(&mut self) {
        self.bytes.fill(0);
        self.length = 0;
    }
}

impl<const N: usize> Default for EspNowV2Reassembly<N> {
    fn default() -> Self {
        Self::new()
    }
}
