use super::*;
use oer_protocols_ieee80211_contracts_port::{ChannelContextId, MacRxMetadata, MonitorTapPoint};

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
    // So do a Trigger and an NDP Announcement.
    for frame_control in [0x24, 0x54] {
        ack[0] = frame_control;
        assert!(filter.accepts(&MonitorFrame {
            bytes: &ack,
            ..frame
        }));
    }
    // A Control Frame Extension's layout is not interpreted: no match.
    ack[0] = 0x64;
    assert!(!filter.accepts(&MonitorFrame {
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

/// A hopping cycle holds its channels in order, at least one and at most
/// its capacity, none twice.
#[test]
fn a_channel_sequence_holds_its_channels_in_order() {
    let channel = |number| Ghz2_4Channel::mhz20(number).unwrap();
    let dwell = NonZeroU16::new(50).unwrap();
    let sequence =
        MonitorChannelSequence::new(&[channel(6), channel(1), channel(11)], dwell).unwrap();
    assert_eq!(sequence.channels(), &[channel(6), channel(1), channel(11)]);
    assert_eq!(sequence.count(), 3);
    assert_eq!(sequence.first(), channel(6));
    assert_eq!(sequence.dwell(), dwell);
    assert_eq!(
        MonitorChannelSequence::new(&[], dwell),
        Err(MonitorChannelSequenceError::Empty)
    );
    assert_eq!(
        MonitorChannelSequence::new(&[channel(1), channel(1)], dwell),
        Err(MonitorChannelSequenceError::Duplicate(channel(1)))
    );
    let full: [Ghz2_4Channel; MONITOR_CHANNEL_SEQUENCE_CAPACITY] =
        core::array::from_fn(|index| channel(u8::try_from(index).unwrap() + 1));
    assert_eq!(
        MonitorChannelSequence::new(&full, dwell).unwrap().count(),
        MONITOR_CHANNEL_SEQUENCE_CAPACITY
    );
    let mut over = [channel(1); MONITOR_CHANNEL_SEQUENCE_CAPACITY + 1];
    over[..MONITOR_CHANNEL_SEQUENCE_CAPACITY].copy_from_slice(&full);
    assert_eq!(
        MonitorChannelSequence::new(&over, dwell),
        Err(MonitorChannelSequenceError::TooMany {
            requested: MONITOR_CHANNEL_SEQUENCE_CAPACITY + 1,
            capacity: MONITOR_CHANNEL_SEQUENCE_CAPACITY,
        })
    );
}
