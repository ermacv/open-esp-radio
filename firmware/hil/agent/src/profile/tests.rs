use super::*;

#[test]
fn only_an_armed_open_window_records_the_selected_harts() {
    let profiler = Profiler::<4>::new();
    profiler.record(0, 1, 2);
    profiler.arm(ProfileHarts::Core1, 1999);
    profiler.record(1, 1, 2);
    profiler.window_begin(100);
    profiler.record(0, 3, 4);
    profiler.record(1, 5, 6);
    profiler.window_end(350);
    profiler.record(1, 7, 8);
    let status = profiler.status();
    assert_eq!(status.samples, [0, 1]);
    assert_eq!(status.window_us, 250);
    assert!(!status.open);
    let page = profiler.page(1, 0).unwrap();
    assert_eq!((page.total, page.samples.as_slice()), (1, &[(5, 6)][..]));
}

#[test]
fn a_full_hart_counts_the_samples_it_drops() {
    let profiler = Profiler::<2>::new();
    profiler.arm(ProfileHarts::Both, 1999);
    profiler.window_begin(0);
    for sample in 0..5 {
        profiler.record(0, sample, sample);
    }
    let status = profiler.status();
    assert_eq!(
        (status.samples[0], status.overflow[0], status.capacity),
        (2, 3, 2)
    );
}

#[test]
fn an_open_window_has_no_pages_and_a_new_window_starts_over() {
    let profiler = Profiler::<4>::new();
    profiler.arm(ProfileHarts::Both, 1999);
    profiler.window_begin(0);
    profiler.record(0, 1, 1);
    assert_eq!(profiler.page(0, 0), Err(PageRefusal::WindowOpen));
    profiler.window_begin(10);
    profiler.window_end(20);
    assert_eq!(profiler.page(0, 0).unwrap().total, 0);
}

#[test]
fn the_window_length_survives_the_microsecond_counter_wrapping() {
    let profiler = Profiler::<1>::new();
    profiler.arm(ProfileHarts::Both, 1999);
    profiler.window_begin(u32::MAX - 9);
    profiler.window_end(10);
    assert_eq!(profiler.status().window_us, 20);
}

#[test]
fn pages_are_bounded_and_rearming_discards_the_previous_profile() {
    let profiler = Profiler::<100>::new();
    profiler.arm(ProfileHarts::Both, 1999);
    profiler.window_begin(0);
    for sample in 0..60 {
        profiler.record(0, sample, 0);
    }
    profiler.window_end(1);
    let first = profiler.page(0, 0).unwrap();
    assert_eq!(first.samples.len(), PROFILE_SAMPLE_PAGE);
    let rest = profiler.page(0, PROFILE_SAMPLE_PAGE as u32).unwrap();
    assert_eq!(rest.samples.len(), 60 - PROFILE_SAMPLE_PAGE);
    profiler.arm(ProfileHarts::Both, 997);
    assert_eq!(profiler.status().samples, [0, 0]);
}
