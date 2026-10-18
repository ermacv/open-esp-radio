use super::{Core0PerformanceSample, Core0RxCyclePhase, Core0RxCycleProfile, Core0RxCycleSnapshot};

#[test]
fn interval_snapshot_uses_wrapping_deltas() {
    let earlier = Core0RxCycleSnapshot {
        services: u32::MAX,
        units: 7,
        total: 100,
        telemetry_protocol_frame_record: u32::MAX - 4,
        ..Core0RxCycleSnapshot::default()
    };
    let current = Core0RxCycleSnapshot {
        services: 1,
        units: 11,
        total: 140,
        telemetry_protocol_frame_record: 3,
        ..Core0RxCycleSnapshot::default()
    };
    let delta = current.wrapping_delta_since(earlier);
    assert_eq!(delta.services, 2);
    assert_eq!(delta.units, 4);
    assert_eq!(delta.total, 40);
    assert_eq!(delta.telemetry_protocol_frame_record, 8);
}

#[test]
fn stage_total_is_derived_without_becoming_an_exclusive_phase() {
    let mut profile = Core0RxCycleProfile {
        performance_started: Core0PerformanceSample::default(),
        started: 0,
        last: 0,
        phase: Core0RxCyclePhase::StageTake,
        sample: Core0RxCycleSnapshot::default(),
    };
    profile.add_current(7);
    profile.phase = Core0RxCyclePhase::StagePool;
    profile.add_current(11);

    assert_eq!(profile.sample.stage_take, 7);
    assert_eq!(profile.sample.stage_pool, 11);
    assert_eq!(profile.sample.stage_total, 18);
}
