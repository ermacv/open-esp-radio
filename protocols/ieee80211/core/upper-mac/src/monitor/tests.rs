use super::*;
use oer_ieee80211_contracts_port::{ChannelContextId, MacRxMetadata, MonitorTapPoint};

#[test]
fn bounded_filter_combines_type_rssi_and_address_without_payload_matching() {
    let selected = [0x02, 1, 2, 3, 4, 5];
    let mut bytes = [0_u8; 32];
    bytes[0] = 0x08; // data
    bytes[4..10].copy_from_slice(&selected);
    let frame = MonitorFrame::<()> {
        tap: MonitorTapPoint::Normalized,
        channel_context: ChannelContextId::PRIMARY,
        bytes: &bytes,
        metadata: MacRxMetadata {
            rssi_dbm: RxEvidence::HardwareObserved(-51),
            ..MacRxMetadata::unavailable()
        },
        logical_length: bytes.len(),
    };
    let filter = MonitorFilter::all()
        .frame_types(MonitorFrameTypeMask::DATA)
        .minimum_rssi_dbm(-60)
        .any_address(selected);

    assert!(filter.accepts(&frame));
    assert!(!filter.minimum_rssi_dbm(-40).accepts(&frame));
    assert!(
        !filter
            .frame_types(MonitorFrameTypeMask::MANAGEMENT)
            .accepts(&frame)
    );

    let mut four_address_bytes = bytes;
    four_address_bytes[4..10].fill(0);
    four_address_bytes[24..30].copy_from_slice(&selected);
    assert!(!filter.accepts(&MonitorFrame {
        bytes: &four_address_bytes,
        ..frame
    }));
    four_address_bytes[1] = 0x03;
    assert!(filter.accepts(&MonitorFrame {
        bytes: &four_address_bytes,
        ..frame
    }));
}

#[test]
fn qos_control_and_payload_of_a_three_address_frame_are_no_address4() {
    let selected = [0x02, 1, 2, 3, 4, 5];
    // QoS Data, From DS: QoS Control at 24..26, then the body.
    let mut bytes = [0_u8; 40];
    bytes[..2].copy_from_slice(&[0x88, 0x02]);
    bytes[24..30].copy_from_slice(&selected);
    let frame = MonitorFrame::<()> {
        tap: MonitorTapPoint::Normalized,
        channel_context: ChannelContextId::PRIMARY,
        bytes: &bytes,
        metadata: MacRxMetadata::unavailable(),
        logical_length: bytes.len(),
    };
    let filter = MonitorFilter::all().any_address(selected);
    assert!(!filter.accepts(&frame));
    // An Ack holds only its RA: octets after it are not a TA.
    let mut ack = [0_u8; 16];
    ack[..2].copy_from_slice(&[0xd4, 0x00]);
    ack[10..16].copy_from_slice(&selected);
    assert!(!filter.accepts(&MonitorFrame {
        bytes: &ack,
        ..frame
    }));
    // An RTS holds its TA there.
    ack[0] = 0xb4;
    assert!(filter.accepts(&MonitorFrame {
        bytes: &ack,
        ..frame
    }));
}

#[test]
fn unavailable_rssi_does_not_satisfy_a_threshold() {
    let bytes = [
        0x80_u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    let frame = MonitorFrame::<()> {
        tap: MonitorTapPoint::Normalized,
        channel_context: ChannelContextId::PRIMARY,
        bytes: &bytes,
        metadata: MacRxMetadata::unavailable(),
        logical_length: bytes.len(),
    };

    assert!(!MonitorFilter::all().minimum_rssi_dbm(-100).accepts(&frame));
}
