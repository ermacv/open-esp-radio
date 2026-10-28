use super::{
    BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES, LeTxBufferHeaderStorage, LeTxPacketAddress,
    LeTxPacketPrepareError, LeTxPacketStorage,
};
use bytemuck::Zeroable;

#[test]
fn encoded_pdu_round_trips_at_the_role_boundary() {
    let mut packet = LeTxPacketStorage::<32>::zeroed();
    let length = packet
        .prepare_encoded_pdu(&[0x42, 3, 1, 2, 3])
        .expect("the encoded PDU fits the controller allocation");

    assert_eq!(
        packet.prepared_pdu(length).collect::<std::vec::Vec<_>>(),
        [0x42, 3, 1, 2, 3]
    );
    assert_eq!(length.payload_bytes(), 3);
}

#[test]
fn malformed_or_oversized_pdu_is_rejected_before_mutation() {
    let mut packet = LeTxPacketStorage::<{ BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + 2 }>::zeroed();
    let before = packet.snapshot();

    assert_eq!(
        packet.prepare_encoded_pdu(&[0x02]),
        Err(LeTxPacketPrepareError::EncodedPduTooShort { available: 1 })
    );
    assert_eq!(packet.snapshot(), before);
    assert_eq!(
        packet.prepare_encoded_pdu(&[0x02, 2, 0]),
        Err(LeTxPacketPrepareError::EncodedPduLengthMismatch {
            declared: 2,
            actual: 1,
        })
    );
    assert_eq!(packet.snapshot(), before);
    assert_eq!(
        packet.prepare_encoded_pdu(&[0x02, 3, 0, 1, 2]),
        Err(LeTxPacketPrepareError::PayloadTooLong {
            length: 3,
            capacity: 2,
        })
    );
    assert_eq!(packet.snapshot(), before);
}

#[test]
fn buffer_header_retains_the_bound_packet_and_its_role_capacity() {
    const LEGACY_ADVERTISING_ALLOCATION_BYTES: usize = BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + 37;
    const DTM_ALLOCATION_BYTES: usize = BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + 255;

    let (storage, placement) = crate::bluetooth::engine::Placement::<
        crate::bluetooth::engine::HostDma<0x2f00_0100>,
    >::take(crate::dma_static::Placed::host(std::boxed::Box::leak(
        std::boxed::Box::new(LeTxPacketStorage::<DTM_ALLOCATION_BYTES>::zeroed()),
    )));
    let advertising =
        LeTxPacketAddress::<LEGACY_ADVERTISING_ALLOCATION_BYTES>::from_placed(&placement, storage);
    let dtm = LeTxPacketAddress::<DTM_ALLOCATION_BYTES>::from_placed(&placement, storage);
    let header = LeTxBufferHeaderStorage::zeroed();

    header.initialize_bound_tx(advertising);

    assert_eq!(header.packet_base_link(), Some(advertising.base_link()));
    assert_eq!(
        header.pdu_target_link(),
        Some(advertising.pdu_target_link())
    );
    assert!(header.retains_allocation_extent(advertising));
    assert!(!header.retains_allocation_extent(dtm));
}
