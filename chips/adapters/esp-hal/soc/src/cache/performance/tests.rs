use super::{L1CacheBusSnapshot, L1CacheCounterEnable, L1CachePerformanceSnapshot};

#[test]
fn interval_snapshot_keeps_status_and_wraps_events() {
    let earlier = L1CachePerformanceSnapshot {
        ibus0: L1CacheBusSnapshot {
            hit: u32::MAX,
            miss: 10,
            ..L1CacheBusSnapshot::default()
        },
        ..L1CachePerformanceSnapshot::default()
    };
    let current = L1CachePerformanceSnapshot {
        trace_enabled: true,
        counter_enable: L1CacheCounterEnable {
            ibus0: true,
            ..L1CacheCounterEnable::default()
        },
        ibus0: L1CacheBusSnapshot {
            hit: 3,
            miss: 15,
            ..L1CacheBusSnapshot::default()
        },
        ..L1CachePerformanceSnapshot::default()
    };

    let delta = current.wrapping_delta_since(earlier);
    assert!(delta.trace_enabled);
    assert!(delta.counter_enable.ibus0);
    assert_eq!(delta.ibus0.hit, 4);
    assert_eq!(delta.ibus0.miss, 5);
}
