#[cfg(feature = "core0-rx-coarse-telemetry")]
use super::Core0PerformanceCounters;
use super::Core0PerformanceSnapshot;

#[test]
fn interval_snapshot_uses_wrapping_deltas() {
    let earlier = Core0PerformanceSnapshot {
        rx_interrupt_posts: u32::MAX,
        radio_polls: u32::MAX,
        radio_cycles: 80,
        radio_instructions: 90,
        poll_to_runner_cycles: u32::MAX,
        poll_to_runner_instructions: u32::MAX,
        dma_calls: u32::MAX,
        ..Core0PerformanceSnapshot::default()
    };
    let current = Core0PerformanceSnapshot {
        rx_interrupt_posts: 2,
        radio_polls: 2,
        radio_cycles: 130,
        radio_instructions: 120,
        poll_to_runner_cycles: 3,
        poll_to_runner_instructions: 4,
        dma_calls: 2,
        ..Core0PerformanceSnapshot::default()
    };
    let delta = current.wrapping_delta_since(earlier);
    assert_eq!(delta.rx_interrupt_posts, 3);
    assert_eq!(delta.radio_polls, 3);
    assert_eq!(delta.radio_cycles, 50);
    assert_eq!(delta.radio_instructions, 30);
    assert_eq!(delta.poll_to_runner_cycles, 4);
    assert_eq!(delta.poll_to_runner_instructions, 5);
    assert_eq!(delta.dma_calls, 3);
}

#[cfg(feature = "core0-rx-coarse-telemetry")]
#[test]
fn exhaustion_episode_requires_a_proven_nonzero_resolution() {
    let counters = Core0PerformanceCounters::new();
    counters.record_dma_entry_remaining(Some(0));
    counters.record_dma_entry_remaining(Some(0));
    counters.record_dma_entry_remaining(None);
    let active = counters.snapshot();
    assert_eq!(active.dma_exhaustion_episodes, 1);
    assert_eq!(active.dma_exhaustion_resolved_le_64us, 0);

    counters.record_dma_entry_remaining(Some(8));
    counters.record_dma_entry_remaining(Some(9));
    let resolved = counters.snapshot();
    assert_eq!(resolved.dma_exhaustion_episodes, 1);
    assert_eq!(resolved.dma_exhaustion_resolved_le_64us, 1);
    assert_eq!(resolved.dma_exhaustion_resolved_le_256us, 0);
    assert_eq!(resolved.dma_exhaustion_resolved_le_1024us, 0);
    assert_eq!(resolved.dma_exhaustion_resolved_gt_1024us, 0);
}
