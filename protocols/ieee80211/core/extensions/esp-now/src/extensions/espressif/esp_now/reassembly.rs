//! Fixed-capacity materialization of a validated ESP-NOW v2 datagram.

use oer_ieee80211_wire::extensions::espressif::esp_now::{
    ESP_NOW_V2_MAX_PAYLOAD_LEN, EspNowV2Action, EspNowV2WireError,
};

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

    /// Borrow the retained datagram, reporting an inconsistent retained length.
    pub fn payload(&self) -> Result<&[u8], EspNowV2WireError> {
        self.bytes
            .get(..self.length)
            .ok_or(EspNowV2WireError::ReassemblyCapacityTooSmall {
                required: self.length,
                capacity: N,
            })
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
        self.payload()
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

#[cfg(test)]
#[allow(clippy::disallowed_macros)]
mod tests {
    use super::*;
    use oer_ieee80211_wire::extensions::espressif::esp_now::EspNowRandomValue;

    #[test]
    fn invalid_retained_length_returns_error_instead_of_panicking() {
        let buffer = EspNowV2Reassembly::<2> {
            bytes: [7, 8],
            length: usize::MAX,
        };
        assert_eq!(
            buffer.payload(),
            Err(EspNowV2WireError::ReassemblyCapacityTooSmall {
                required: usize::MAX,
                capacity: 2,
            })
        );
    }

    #[test]
    fn capacity_refusal_preserves_payload_and_clear_handles_empty_storage() {
        let action = EspNowV2Action::new(EspNowRandomValue::new([1; 4]), &[7, 8]).unwrap();
        let mut buffer = EspNowV2Reassembly::<2>::new();
        assert_eq!(buffer.reassemble(action), Ok([7, 8].as_slice()));
        let larger = EspNowV2Action::new(EspNowRandomValue::new([1; 4]), &[3; 3]).unwrap();
        assert_eq!(
            buffer.reassemble(larger),
            Err(EspNowV2WireError::ReassemblyCapacityTooSmall {
                required: 3,
                capacity: 2,
            })
        );
        assert_eq!(buffer.payload(), Ok([7, 8].as_slice()));
        buffer.clear();
        assert_eq!(buffer.payload(), Ok([].as_slice()));
        let empty = EspNowV2Action::new(EspNowRandomValue::new([1; 4]), &[]).unwrap();
        assert_eq!(
            EspNowV2Reassembly::<0>::new().reassemble(empty),
            Ok([].as_slice())
        );
    }
}
