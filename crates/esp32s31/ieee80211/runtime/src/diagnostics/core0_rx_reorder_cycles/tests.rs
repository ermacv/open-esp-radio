use super::Core0ReorderSnapshot;

#[test]
fn interval_snapshot_uses_wrapping_deltas() {
    let earlier = Core0ReorderSnapshot {
        calls: u32::MAX,
        total: 80,
        ..Core0ReorderSnapshot::default()
    };
    let current = Core0ReorderSnapshot {
        calls: 2,
        total: 130,
        ..Core0ReorderSnapshot::default()
    };
    let delta = current.wrapping_delta_since(earlier);
    assert_eq!(delta.calls, 3);
    assert_eq!(delta.total, 50);
}
