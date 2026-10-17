use super::*;
use crate::MacRxMetadata;

#[test]
fn capture_completeness_is_distinct_from_sink_overflow() {
    let bytes = [0_u8; 24];
    let complete = MonitorFrame::<()> {
        tap: MonitorTapPoint::Normalized,
        channel_context: ChannelContextId::PRIMARY,
        bytes: &bytes,
        metadata: MacRxMetadata::unavailable(),
        logical_length: bytes.len(),
    };
    let hardware_consumed_trailer = MonitorFrame {
        logical_length: bytes.len() + 8,
        ..complete
    };

    assert!(complete.is_complete());
    assert!(!hardware_consumed_trailer.is_complete());
    assert_ne!(
        MonitorPublishOutcome::Dropped(MonitorDropReason::Full),
        MonitorPublishOutcome::Published
    );
}

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
            rssi_dbm: MacRxEvidence::HardwareObserved(-51),
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
