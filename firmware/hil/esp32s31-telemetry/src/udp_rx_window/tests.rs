use super::*;

fn window() -> RxWindow {
    RxWindow::new(100, 16_000, 2_000, 750, 750)
}

#[test]
fn silence_does_not_finish_and_resumed_data_is_counted() {
    let mut window = window();
    assert!(window.data(200));
    assert!(!window.finished(1_500, false));
    assert!(window.data(10_000));
    assert!(window.data(16_199));
    assert_eq!(window.end(), 16_200);
    assert_eq!(window.summary().pauses, 2);
    assert_eq!(window.summary().maximum_silence_micros, 9_800);
    assert_eq!(window.summary().maximum_silence_start_micros, 0);
}

#[test]
fn zero_delivery_is_bounded_and_accounts_for_the_full_window() {
    let window = window();
    assert!(!window.finished(18_100, false));
    assert!(window.finished(18_850, false));
    assert_eq!(window.summary().first_delay_micros, None);
    assert_eq!(window.summary().maximum_silence_micros, 16_000);
    assert_eq!(window.summary().pauses, 1);
}

#[test]
fn early_terminal_does_not_shorten_the_window() {
    let mut window = window();
    assert!(window.data(200));
    assert!(!window.finished(500, true));
    assert!(window.finished(16_200, true));
    assert_eq!(window.summary().trailing_silence_micros, 16_000);
}

#[test]
fn late_first_data_preserves_the_startup_bound_and_initial_silence() {
    let mut window = window();
    assert!(window.data(10_000));
    assert_eq!(window.start(), 2_100);
    assert_eq!(window.end(), 18_100);
    assert_eq!(window.summary().first_delay_micros, Some(9_900));
    assert_eq!(window.summary().maximum_silence_micros, 8_100);
    assert_eq!(window.summary().maximum_silence_start_micros, 7_900);
}

#[test]
fn grace_accepts_completion_but_excludes_late_payload() {
    let mut window = window();
    assert!(window.data(200));
    assert!(!window.data(16_200));
    assert!(!window.finished(16_300, false));
    assert!(window.finished(16_300, true));
    assert!(window.finished(16_950, false));
    assert_eq!(window.next_deadline(16_300), 16_950);
}

#[test]
fn delivery_start_requires_data_and_is_published_only_once() {
    let mut start = super::DeliveryStart::default();
    for _ in 0..300 {
        assert!(!start.observe(true, Some(-1)));
        assert!(!start.observe(false, Some(0)));
        assert!(!start.observe(true, None));
    }
    for n in 0..255 {
        assert!(!start.observe(true, Some(n)));
    }
    assert!(start.observe(true, Some(255)));
    for n in 256..1000 {
        assert!(!start.observe(true, Some(n)));
    }
}
