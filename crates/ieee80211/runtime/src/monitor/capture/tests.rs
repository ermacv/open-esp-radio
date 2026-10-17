use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use oer_ieee80211_softmac::{
    MacRxMetadata,
    interface::{ChannelContextId, MonitorTapPoint},
};

use super::*;

fn frame(bytes: &[u8]) -> MonitorFrame<'_, ()> {
    MonitorFrame {
        tap: MonitorTapPoint::Normalized,
        channel_context: ChannelContextId::PRIMARY,
        bytes,
        metadata: MacRxMetadata::unavailable(),
        logical_length: bytes.len(),
    }
}

#[test]
fn capture_is_independent_from_the_borrowed_source() {
    let pool = MonitorCapturePool::<16, 1>::new();
    let resources = MonitorCaptureResources::<NoopRawMutex, (), 1, 16, 1>::new(&pool);
    let (mut sink, receiver) = resources.split();
    let mut source = [1, 2, 3, 4];

    assert_eq!(
        sink.try_publish(frame(&source)),
        MonitorPublishOutcome::Published
    );
    source.fill(9);
    let captured = receiver.try_receive().expect("one retained capture");
    assert_eq!(captured.bytes(), &[1, 2, 3, 4]);
    assert!(captured.is_complete());
    assert_eq!(pool.claimed_slots(), 1);
    drop(captured);
    assert_eq!(pool.claimed_slots(), 0);
}

#[test]
fn full_queue_drops_the_new_capture_and_restores_its_pool_slot() {
    let pool = MonitorCapturePool::<16, 2>::new();
    let resources = MonitorCaptureResources::<NoopRawMutex, (), 1, 16, 2>::new(&pool);
    let (mut sink, receiver) = resources.split();

    assert_eq!(
        sink.try_publish(frame(&[1])),
        MonitorPublishOutcome::Published
    );
    assert_eq!(
        sink.try_publish(frame(&[2])),
        MonitorPublishOutcome::Dropped(MonitorDropReason::Full)
    );
    assert_eq!(pool.claimed_slots(), 1);
    drop(
        receiver
            .try_receive()
            .expect("first capture remains queued"),
    );
    assert_eq!(pool.claimed_slots(), 0);
}

#[test]
fn epoch_cleanup_discards_queued_frames_and_restores_all_credits() {
    let pool = MonitorCapturePool::<16, 2>::new();
    let resources = MonitorCaptureResources::<NoopRawMutex, (), 2, 16, 2>::new(&pool);
    let (mut sink, receiver) = resources.split();
    assert_eq!(
        sink.try_publish(frame(&[1])),
        MonitorPublishOutcome::Published
    );
    assert_eq!(
        sink.try_publish(frame(&[2])),
        MonitorPublishOutcome::Published
    );
    assert_eq!(pool.claimed_slots(), 2);

    assert_eq!(receiver.discard_queued(), 2);
    assert_eq!(pool.claimed_slots(), 0);
    assert_eq!(receiver.discard_queued(), 0);
}

#[test]
fn exhausted_pool_and_oversized_frames_have_distinct_reasons() {
    let pool = MonitorCapturePool::<4, 1>::new();
    let resources = MonitorCaptureResources::<NoopRawMutex, (), 1, 4, 1>::new(&pool);
    let (mut sink, receiver) = resources.split();

    assert_eq!(
        sink.try_publish(frame(&[1, 2, 3, 4, 5])),
        MonitorPublishOutcome::Dropped(MonitorDropReason::TooLong)
    );
    assert_eq!(
        sink.try_publish(frame(&[1])),
        MonitorPublishOutcome::Published
    );
    let retained = receiver.try_receive().expect("pool owner");
    assert_eq!(
        sink.try_publish(frame(&[2])),
        MonitorPublishOutcome::Dropped(MonitorDropReason::Full)
    );
    drop(retained);
}

#[test]
fn incomplete_normalized_capture_preserves_logical_length() {
    let pool = MonitorCapturePool::<8, 1>::new();
    let resources = MonitorCaptureResources::<NoopRawMutex, (), 1, 8, 1>::new(&pool);
    let (mut sink, receiver) = resources.split();
    let bytes = [1, 2, 3, 4];
    let mut observed = frame(&bytes);
    observed.logical_length = 12;

    assert_eq!(sink.try_publish(observed), MonitorPublishOutcome::Published);
    let captured = receiver.try_receive().expect("one capture");
    assert!(!captured.is_complete());
    assert_eq!(captured.metadata().logical_length, 12);
    assert_eq!(captured.captured_length(), 4);
}

#[test]
fn epoch_policy_tags_and_truncates_without_changing_logical_length() {
    let pool = MonitorCapturePool::<8, 1>::new();
    let resources = MonitorCaptureResources::<NoopRawMutex, (), 1, 8, 1>::new(&pool);
    let (mut sink, receiver) = resources.split();
    sink.configure(17, Some(3));

    assert_eq!(
        sink.try_publish(frame(&[1, 2, 3, 4, 5])),
        MonitorPublishOutcome::Published
    );
    let captured = receiver.try_receive().expect("one truncated capture");
    assert_eq!(captured.bytes(), &[1, 2, 3]);
    assert_eq!(captured.metadata().generation, 17);
    assert_eq!(captured.metadata().logical_length, 5);
    assert!(!captured.is_complete());
}

#[test]
fn reported_payload_storage_excludes_queue_metadata() {
    assert_eq!(
        MonitorCapturePool::<2_048, 12>::payload_storage_bytes(),
        24_576
    );
    assert_eq!(MonitorCapturePool::<2_048, 12>::slot_capacity(), 2_048);
    assert_eq!(MonitorCapturePool::<2_048, 12>::slot_count(), 12);
}
