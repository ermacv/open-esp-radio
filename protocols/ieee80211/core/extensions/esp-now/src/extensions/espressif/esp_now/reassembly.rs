//! Fixed-capacity materialization of a validated ESP-NOW v2 datagram.

use oer_protocols_ieee80211_wire::extensions::espressif::esp_now::{
    ESP_NOW_V2_MAX_PAYLOAD_LEN, EspNowV2Action, EspNowV2WireError,
};

/// Caller-owned, fixed-capacity storage for a reassembled v2 datagram.
///
/// The default capacity accepts the complete public v2 payload. Applications
/// may select a smaller const capacity and receive a fail-closed capacity
/// error before the previous contents are modified.
pub struct EspNowV2Reassembly<const N: usize = ESP_NOW_V2_MAX_PAYLOAD_LEN> {
    bytes: [u8; N],
    length: oer_portable_wire::PrefixLength<N>,
}

impl<const N: usize> EspNowV2Reassembly<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            length: oer_portable_wire::PrefixLength::ZERO,
        }
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    pub const fn len(&self) -> usize {
        self.length.get()
    }

    pub const fn is_empty(&self) -> bool {
        self.length.get() == 0
    }

    /// The retained datagram.
    pub fn payload(&self) -> &[u8] {
        self.length.borrow(&self.bytes)
    }

    /// Copy `action`'s payload in; a payload longer than the capacity is
    /// refused before the retained one changes.
    pub fn reassemble(&mut self, action: EspNowV2Action<'_>) -> Result<&[u8], EspNowV2WireError> {
        let Some(length) = oer_portable_wire::PrefixLength::new(action.payload_len()) else {
            return Err(EspNowV2WireError::ReassemblyCapacityTooSmall {
                required: action.payload_len(),
                capacity: N,
            });
        };
        action.copy_payload(&mut self.bytes)?;
        self.length = length;
        Ok(self.payload())
    }

    pub fn clear(&mut self) {
        self.bytes.fill(0);
        self.length = oer_portable_wire::PrefixLength::ZERO;
    }
}

impl<const N: usize> Default for EspNowV2Reassembly<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_protocols_ieee80211_wire::extensions::espressif::esp_now::EspNowRandomValue;

    #[test]
    fn capacity_refusal_preserves_payload_and_clear_handles_empty_storage() {
        let action = EspNowV2Action::new(EspNowRandomValue::new([1; 4]), &[7, 8]).unwrap();
        let mut buffer = EspNowV2Reassembly::<2>::new();
        assert_eq!((buffer.len(), buffer.is_empty()), (0, true));
        assert_eq!(buffer.reassemble(action), Ok([7, 8].as_slice()));
        assert_eq!((buffer.len(), buffer.is_empty()), (2, false));
        let larger = EspNowV2Action::new(EspNowRandomValue::new([1; 4]), &[3; 3]).unwrap();
        assert_eq!(
            buffer.reassemble(larger),
            Err(EspNowV2WireError::ReassemblyCapacityTooSmall {
                required: 3,
                capacity: 2,
            })
        );
        assert_eq!(buffer.payload(), [7, 8].as_slice());
        buffer.clear();
        assert_eq!(buffer.payload(), [].as_slice());
        assert_eq!((buffer.len(), buffer.is_empty()), (0, true));
        let empty = EspNowV2Action::new(EspNowRandomValue::new([1; 4]), &[]).unwrap();
        assert_eq!(
            EspNowV2Reassembly::<0>::new().reassemble(empty),
            Ok([].as_slice())
        );
    }
}
