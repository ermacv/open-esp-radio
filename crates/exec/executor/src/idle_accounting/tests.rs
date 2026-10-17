use super::*;

fn samples(entries: &[(usize, usize, u64)]) -> [Option<Sample>; EXECUTORS] {
    let mut samples = [None; EXECUTORS];
    for &(executor, core, idle_cycles) in entries {
        samples[executor] = Some(Sample { core, idle_cycles });
    }
    samples
}

#[test]
fn a_window_holds_each_executor_s_idle_cycles_and_the_opening_core_s_length() {
    let report = Report::between(
        1_000,
        samples(&[(0, 0, 100), (1, 1, 5_000)]),
        11_000,
        samples(&[(0, 0, 2_600), (1, 1, 14_000)]),
    );
    assert_eq!(report.window_cycles, 10_000);
    assert_eq!(report.idle_cycles(0), Some(2_500));
    assert_eq!(report.idle_cycles(1), Some(9_000));
    assert_eq!(report.idle_cycles(2), None);
}

#[test]
fn counters_and_the_cycle_counter_wrap_inside_a_window() {
    let report = Report::between(
        u64::MAX - 99,
        samples(&[(0, 0, u64::MAX - 9)]),
        900,
        samples(&[(0, 0, 290)]),
    );
    assert_eq!(report.window_cycles, 1_000);
    assert_eq!(report.idle_cycles(0), Some(300));
}

#[test]
fn an_executor_missing_at_a_boundary_or_on_another_core_is_unmeasured() {
    let report = Report::between(
        0,
        samples(&[(0, 0, 0), (1, 1, 0), (2, 0, 0)]),
        1_000,
        samples(&[(0, 0, 10), (2, 1, 20), (3, 1, 30)]),
    );
    assert_eq!(report.executors[0].map(|idle| idle.idle_cycles), Some(10));
    assert_eq!(report.executors[1], None);
    assert_eq!(report.executors[2], None);
    assert_eq!(report.executors[3], None);
}
