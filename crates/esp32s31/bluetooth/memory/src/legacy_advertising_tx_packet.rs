//! Legacy advertising has an address inserted by the controller before TX data.
//!
//! Keep the canonical PDU for CPU-side validation and cancellation separately
//! from the controller allocation. The latter retains the on-air length but
//! omits AdvA from the bytes consumed after the header.

#![forbid(unsafe_code)]

use crate::le_tx_packet::{
    LeTxPacketPrepareError, LeTxPacketPreparedInput, LeTxPacketPreparedLength, LeTxPacketStorage,
};

const MAX_PDU_BYTES: usize = 39;

// The controller allocation is the first member: graph bindings target its
// existing allocation extent, not the CPU-only canonical copy following it.
#[repr(C, align(4))]
pub(crate) struct LegacyAdvertisingTxPacketStorage<const N: usize> {
    controller: LeTxPacketStorage<N>,
    canonical: [u8; MAX_PDU_BYTES],
}

impl<const N: usize> LegacyAdvertisingTxPacketStorage<N> {
    pub(crate) const fn new() -> Self {
        Self {
            controller: LeTxPacketStorage::new(),
            canonical: [0; MAX_PDU_BYTES],
        }
    }

    pub(crate) fn prepare_encoded_pdu(
        &mut self,
        pdu: &[u8],
    ) -> Result<LeTxPacketPreparedLength<N>, LeTxPacketPrepareError> {
        let packet = LeTxPacketPreparedInput::new(pdu)?;
        Ok(self.prepare_validated_encoded_pdu(packet))
    }

    pub(crate) fn prepare_validated_encoded_pdu(
        &mut self,
        packet: LeTxPacketPreparedInput<'_, N>,
    ) -> LeTxPacketPreparedLength<N> {
        let pdu = packet.as_bytes();
        self.canonical.fill(0);
        self.canonical[..pdu.len()].copy_from_slice(pdu);
        self.controller.prepare_validated_encoded_pdu(packet)
    }

    /// Apply only after the role has established a six-byte advertiser address.
    pub(crate) fn lower_advertiser_address(&mut self, length: LeTxPacketPreparedLength<N>) {
        self.controller.omit_legacy_advertiser_address(length);
    }

    pub(crate) fn prepared_pdu(&self, length: LeTxPacketPreparedLength<N>) -> &[u8] {
        &self.canonical[..length.pdu_bytes()]
    }

    pub(crate) fn clear(&mut self) {
        self.controller.clear();
        self.canonical.fill(0);
    }

    #[cfg(test)]
    pub(crate) fn model_transmitted_pdu(
        &self,
        length: LeTxPacketPreparedLength<N>,
        advertiser: [u8; 6],
    ) -> std::vec::Vec<u8> {
        let stored = self.controller.prepared_pdu(length);
        let mut pdu = stored[..2].to_vec();
        pdu.extend(advertiser);
        pdu.extend_from_slice(&stored[2..length.pdu_bytes() - 6]);
        pdu
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::le_tx_packet::BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES;

    #[test]
    fn controller_insertion_preserves_complete_advertising_data_and_scan_response() {
        let mut packet =
            LegacyAdvertisingTxPacketStorage::<{ BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + 37 }>::new();
        for (header, data) in [
            (0x00, &[2, 1, 6, 4, 9, b'A', b'B', b'C'][..]),
            (0x44, &[][..]),
            (0x42, &[7; 31][..]),
        ] {
            let address = [1, 2, 3, 4, 5, 0xc6];
            let mut expected = std::vec![header, (6 + data.len()) as u8];
            expected.extend(address);
            expected.extend_from_slice(data);
            let length = packet.prepare_encoded_pdu(&expected).unwrap();
            packet.lower_advertiser_address(length);
            assert_eq!(packet.model_transmitted_pdu(length, address), expected);
            assert_eq!(packet.prepared_pdu(length), expected);
            packet.clear();
        }
    }
}
