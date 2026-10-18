use super::*;
use crate::air::test_support::{frame, mac};

const TARGET: &str = "02:00:00:00:00:01";
const PEER: &str = "02:00:00:00:00:02";

/// Target data (`D`) and peer BlockAck (`B`) records at microsecond times.
fn egress(records: &[(u64, char)]) -> TargetEgressAirTiming {
    let frames = records
        .iter()
        .map(|&(time, kind)| {
            let mut record = frame(
                time,
                if kind == 'D' {
                    FrameKind(0x28)
                } else {
                    FrameKind::BLOCK_ACK
                },
            );
            if kind == 'D' {
                record.transmitter = Some(mac(TARGET));
                record.destination = Some(mac(PEER));
            } else {
                record.transmitter = Some(mac(PEER));
                record.receiver = Some(mac(TARGET));
            }
            record
        })
        .collect::<Vec<_>>();
    target_egress_timing(&frames, mac(TARGET))
}

#[test]
fn target_egress_timing_pairs_ppdu_tail_block_ack_and_next_ppdu() {
    let evidence = egress(&[
        (100_000_000, 'D'),
        (100_000_100, 'D'),
        (100_000_140, 'B'),
        (100_000_300, 'D'),
        (100_000_500, 'B'),
        (100_000_900, 'D'),
    ]);

    assert_eq!(evidence.target_data_frames, 4);
    assert_eq!(evidence.peer_block_ack_frames, 2);
    assert!(evidence.target_data_pairing_available);
    assert_eq!(
        evidence.peer_block_ack_interarrival,
        Some(AirIntervalSummary {
            samples: 1,
            total_micros: 360,
            minimum_micros: 360,
            p50_micros: 360,
            p95_micros: 360,
            p99_micros: 360,
            maximum_micros: 360,
        })
    );
    assert_eq!(
        evidence.data_to_block_ack,
        Some(AirIntervalSummary {
            samples: 2,
            total_micros: 240,
            minimum_micros: 40,
            p50_micros: 40,
            p95_micros: 200,
            p99_micros: 200,
            maximum_micros: 200,
        })
    );
    assert_eq!(
        evidence.block_ack_to_next_data,
        Some(AirIntervalSummary {
            samples: 2,
            total_micros: 560,
            minimum_micros: 160,
            p50_micros: 160,
            p95_micros: 400,
            p99_micros: 400,
            maximum_micros: 400,
        })
    );
}

#[test]
fn block_ack_cadence_survives_missing_target_data_decode() {
    let evidence = egress(&[
        (100_000_000, 'D'),
        (100_000_100, 'B'),
        (100_000_500, 'B'),
        (100_001_100, 'B'),
    ]);

    assert_eq!(evidence.target_data_frames, 1);
    assert_eq!(evidence.peer_block_ack_frames, 3);
    assert!(!evidence.target_data_pairing_available);
    assert_eq!(evidence.data_to_block_ack, None);
    assert_eq!(evidence.block_ack_to_next_data, None);
    assert_eq!(
        evidence.peer_block_ack_interarrival,
        Some(AirIntervalSummary {
            samples: 2,
            total_micros: 1_000,
            minimum_micros: 400,
            p50_micros: 400,
            p95_micros: 600,
            p99_micros: 600,
            maximum_micros: 600,
        })
    );
}

#[test]
fn unpaired_block_ack_disables_pair_derived_timing_even_with_many_data_records() {
    let evidence = egress(&[
        (100_000_000, 'D'),
        (100_000_010, 'D'),
        (100_000_020, 'D'),
        (100_000_100, 'B'),
        (100_000_500, 'B'),
    ]);

    assert_eq!(evidence.target_data_frames, 3);
    assert_eq!(evidence.peer_block_ack_frames, 2);
    assert!(!evidence.target_data_pairing_available);
    assert_eq!(evidence.data_to_block_ack, None);
    assert_eq!(evidence.block_ack_to_next_data, None);
}

#[test]
fn retry_grouping_counts_one_logical_mpdu() {
    let data = |time, retry| {
        let mut record = frame(time, FrameKind(0x28));
        record.transmitter = Some(mac(PEER));
        record.destination = Some(mac(TARGET));
        record.sequence = Some(12);
        record.fragment = Some(0);
        record.tid = Some(0);
        record.retry = Some(retry);
        record
    };
    let mut unsequenced = data(1_500_000, false);
    unsequenced.sequence = None;
    let evidence = analyze(
        &[
            data(1_000_000, false),
            data(1_020_000, true),
            data(1_200_000, true),
            unsequenced,
        ],
        mac(TARGET),
    );
    assert_eq!(evidence.logical_data_units, 2);
    assert_eq!(evidence.retry_attempts, 2);
    assert_eq!(evidence.missing_mac_metadata, 1);
}

#[test]
fn monitor_action_follows_openwrt_primary_channel() {
    assert_eq!(
        resolve_observer_action_from_iw(
            "type AP\n\tchannel 6 (2437 MHz), width: 40 MHz, center1: 2447 MHz\n"
        )
        .unwrap(),
        Geometry {
            frequency: 2437,
            width: 40,
            center: 2447
        }
    );
    assert_eq!(
        resolve_observer_action_from_iw(
            "type AP\n\tchannel 13 (2472 MHz), width: 40 MHz, center1: 2462 MHz\n"
        )
        .unwrap(),
        Geometry {
            frequency: 2472,
            width: 40,
            center: 2462
        }
    );
    assert!(resolve_observer_action_from_iw("channel 3 (2422 MHz)").is_err());
}

#[test]
fn block_ack_tracker_unwraps_windows_and_deduplicates_overlap() {
    let mut tracker = BlockAckTracker::default();
    tracker.observe(4_090, [u8::MAX; 8]);
    tracker.observe(10, [u8::MAX; 8]);
    assert_eq!(tracker.frames, 2);
    assert_eq!(tracker.full_frames, 2);
    assert_eq!(tracker.tail_frames, 0);
    assert_eq!(tracker.hole_frames, 0);
    assert_eq!(tracker.backward_starts, 0);
    assert_eq!(tracker.acknowledged.len(), 80);

    tracker.observe(9, [1; 8]);
    assert_eq!(tracker.backward_starts, 1);
    assert_eq!(tracker.frames, 2);
}

#[test]
fn block_ack_bitmap_bytes_use_little_bit_order() {
    let mut tracker = BlockAckTracker::default();
    tracker.observe(1, [0x7f, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(tracker.tail_frames, 1);
    assert_eq!(tracker.hole_frames, 0);
    assert_eq!(tracker.acknowledged.len(), 7);
}

#[test]
fn distinguishes_normal_block_ack_tail_from_internal_loss_hole() {
    assert!(!block_ack_bitmap_has_internal_hole([
        0xff, 0x7f, 0, 0, 0, 0, 0, 0,
    ]));
    assert!(block_ack_bitmap_has_internal_hole([
        0xff, 0xf7, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    ]));

    let mut tracker = BlockAckTracker::default();
    tracker.observe(1, [0xff, 0x7f, 0, 0, 0, 0, 0, 0]);
    tracker.observe(16, [0xff, 0xf7, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
    assert_eq!(tracker.full_frames, 0);
    assert_eq!(tracker.tail_frames, 1);
    assert_eq!(tracker.hole_frames, 1);
}
