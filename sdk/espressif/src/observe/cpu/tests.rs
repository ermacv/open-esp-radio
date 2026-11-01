use super::*;

fn at(at_us: u32, phase: Phase) -> Boundary {
    Boundary {
        at_us,
        executor: 2,
        hart: 0,
        level: 3,
        phase,
    }
}

#[test]
fn a_counter_accumulates_closed_and_open_spans_of_each_kind() {
    let counter = CpuCounter::new();
    assert_eq!(
        counter.sample(5),
        Ok(ExecutorSample::default()),
        "nothing seen"
    );
    counter.observe(at(10, Phase::WorkStart));
    counter.observe(at(25, Phase::WorkEnd));
    counter.observe(at(30, Phase::IdleStart));
    counter.observe(at(70, Phase::IdleEnd));
    counter.observe(at(100, Phase::WorkStart));
    let sample = counter.sample(104).unwrap();
    assert_eq!((sample.executor, sample.hart, sample.level), (2, 0, 3));
    assert_eq!(sample.work_us, Some(15 + 4), "the open span counts");
    assert_eq!(sample.idle_us, Some(40));
}

#[test]
fn spans_cross_the_activity_clock_wrap_and_totals_exceed_32_bits() {
    let counter = CpuCounter::new();
    counter.observe(at(u32::MAX - 4, Phase::WorkStart));
    counter.observe(at(5, Phase::WorkEnd));
    assert_eq!(counter.sample(6).unwrap().work_us, Some(10));
    for _ in 0..3 {
        counter.observe(at(0, Phase::WorkStart));
        counter.observe(at(u32::MAX, Phase::WorkEnd));
    }
    assert_eq!(
        counter.sample(6).unwrap().work_us,
        Some(10 + 3 * u64::from(u32::MAX))
    );
}

#[test]
fn an_end_without_its_start_adds_nothing() {
    let counter = CpuCounter::new();
    counter.observe(at(10, Phase::IdleStart));
    counter.observe(at(20, Phase::WorkEnd));
    counter.observe(at(30, Phase::IdleEnd));
    let sample = counter.sample(40).unwrap();
    assert_eq!((sample.work_us, sample.idle_us), (None, Some(0)));
}

#[test]
fn a_sample_during_a_write_is_refused() {
    let counter = CpuCounter::new();
    counter.sequence.store(1, Ordering::Relaxed);
    assert_eq!(counter.sample(0), Err(Concurrent));
}
