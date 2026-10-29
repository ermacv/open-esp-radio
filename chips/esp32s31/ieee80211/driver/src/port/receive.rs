//! The protocol port's RX boundary, distinct from the raw monitor view.
use crate::mac::rx::{
    NormalizedRxFrame, RxError, RxIngressConfig, RxSegment, view_ccmp_data,
    view_ccmp_data_fragment, view_normalized_rx_frame,
};
use oer_ieee80211_port::{RxCryptoStatus, RxEvidence};
use oer_ieee80211_wire::{
    frame::{FrameControlFields, FrameType},
    sequence::SequenceControl,
};

/// Keep the MAC header/CCMP PN for protocol replay admission, but expose only
/// authenticated plaintext after it. The validator establishes MIC success;
/// the Protected bit alone never establishes integrity.
pub(super) fn normalize(
    segment: RxSegment<'_>,
    config: RxIngressConfig,
) -> Result<NormalizedRxFrame<'_>, RxError> {
    let mut frame = view_normalized_rx_frame(&segment, config)?;
    let control =
        FrameControlFields::from_bytes(*frame.mpdu.first_chunk::<2>().ok_or(RxError::Bounds)?);
    if control.kind() == FrameType::Data && control.protected() {
        let sequence = SequenceControl::from_bytes(
            frame
                .mpdu
                .get(22..24)
                .ok_or(RxError::Bounds)?
                .try_into()
                .map_err(|_| RxError::Bounds)?,
        );
        let data = if control.more_fragments() || sequence.fragment() != 0 {
            view_ccmp_data_fragment(&segment, config)?
        } else {
            view_ccmp_data(&segment, config)?
        };
        frame.mpdu = data
            .mpdu
            .get(..data.frame.mic_offset)
            .ok_or(RxError::Bounds)?;
        frame.metadata.crypto =
            RxEvidence::HardwareObserved(RxCryptoStatus::DecryptedAndIntegrityVerified);
    }
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_esp32s31_hal::ieee80211::dma::descriptor::rx_landed_word;
    use oer_ieee80211_wire::{
        ap::ApProtectedDataFrame,
        ccmp::{CcmpHeader, CcmpKeyId, CcmpPacketNumber},
        data::EthernetFrameParts,
        sequence::SequenceNumber,
    };

    fn payload(storage: &mut [u8; 192], key_id: u8, mic_bytes: usize) -> (usize, usize) {
        let access_point = [2, 0, 0, 0, 0, 2];
        let receiver = if key_id == 0 {
            [2, 0, 0, 0, 0, 1]
        } else {
            [0xff; 6]
        };
        let mut ethernet = [0; 64];
        let len = EthernetFrameParts {
            source: access_point,
            destination: receiver,
            ether_type: if key_id == 0 { 0x888e } else { 0x0800 },
            payload: b"protocol-payload",
        }
        .copy_to(&mut ethernet)
        .expect("Ethernet");
        let frame = ApProtectedDataFrame {
            access_point,
            peer: receiver,
            sequence_number: SequenceNumber::new(7).expect("SN"),
            user_priority: 0,
            peer_qos: true,
            more_data: false,
            ccmp_header: CcmpHeader::new(
                CcmpPacketNumber::new(7).expect("PN"),
                CcmpKeyId::new(key_id).expect("KeyID"),
            )
            .encode(),
            ethernet: &ethernet[..len],
        };
        // Same completed-prefix fixture as mac/tests/cases/rx_frames.rs.
        // Emulate a decrypted DMA image with 0..=8 retained MIC bytes.
        const TAIL: usize = 0x38;
        const FRAME: usize = TAIL + 8;
        let len = frame.encode(&mut storage[FRAME..]).expect("MPDU");
        storage[FRAME + len..FRAME + len + mic_bytes].fill(0xa5);
        let signal = u16::try_from(len + 8 + 4).expect("fixture length");
        storage[TAIL..TAIL + 2].copy_from_slice(&signal.to_le_bytes());
        storage[TAIL + 2..TAIL + 4].copy_from_slice(&(signal + 4).to_le_bytes());
        (len, FRAME + len + mic_bytes)
    }
    fn segment(storage: &[u8; 192], received: usize) -> RxSegment<'_> {
        RxSegment {
            descriptor_address: 0x2f00_4000,
            descriptor_word0: rx_landed_word(
                192,
                received.try_into().expect("fixture length"),
                true,
            ),
            buffer: storage,
            next_descriptor_address: 0,
        }
    }
    const CONFIG: RxIngressConfig = RxIngressConfig {
        ring_entry_limit: 1,
        csi_config: 0,
        flags: 0,
    };

    #[test]
    fn protocol_port_preserves_verified_ptk_and_gtk_evidence_and_strips_only_mic() {
        for key_id in 0..=3 {
            for mic_bytes in 0..=8 {
                let mut storage = [0; 192];
                let (len, received) = payload(&mut storage, key_id, mic_bytes);
                let segment = segment(&storage, received);
                let monitor = view_normalized_rx_frame(&segment, CONFIG).expect("raw monitor");
                assert_eq!(monitor.metadata.crypto, RxEvidence::Unavailable);
                assert_eq!(monitor.mpdu.len(), len + mic_bytes);
                let protocol = normalize(segment, CONFIG).expect("verified protocol frame");
                assert_eq!(
                    protocol.metadata.crypto,
                    RxEvidence::HardwareObserved(RxCryptoStatus::DecryptedAndIntegrityVerified)
                );
                assert_eq!(protocol.mpdu, &monitor.mpdu[..len]);
                assert_eq!(
                    protocol.mpdu.as_ptr(),
                    monitor.mpdu.as_ptr(),
                    "same retained DMA allocation"
                );
                assert_eq!(
                    protocol.logical_length, monitor.logical_length,
                    "on-air length is retained"
                );
                assert_eq!(protocol.stamp, monitor.stamp);
                assert!(protocol.mpdu.ends_with(b"protocol-payload"));
            }
        }
    }
    #[test]
    fn failed_mic_invalid_extiv_and_truncated_plaintext_never_gain_integrity() {
        for mutation in 0..3 {
            let mut storage = [0; 192];
            let (len, mut received) = payload(&mut storage, 1, 8);
            match mutation {
                0 => storage[0x38 + 4] = 0xf5, // Same hardware MIC-failure fixture as the MAC tests.
                1 => storage[0x40 + 24 + 3] = 0, // Legacy broadcast CCMP ExtIV missing.
                _ => received = 0x40 + len - 1, // Missing payload, beyond a consumed MIC trailer.
            }
            assert!(normalize(segment(&storage, received), CONFIG).is_err());
        }
    }
}
