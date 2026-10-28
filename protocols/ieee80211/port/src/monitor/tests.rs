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
