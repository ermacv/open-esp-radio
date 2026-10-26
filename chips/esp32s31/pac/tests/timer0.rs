//! T0 configuration changes preserve the other owners' interrupt lanes.
use oer_esp32s31_pac::timg::Timg0Timer0;

#[test]
fn masking_timer0_preserves_the_timer1_and_watchdog_enables() {
    let mut words = [u32::MAX; 64];
    let mut timer = Timg0Timer0::host(&mut words).expect("host storage covers the owner");
    timer.modify_int_ena_timers(|w| {
        w.set_t0_int_ena(false);
    });
    let masked = timer.read_int_ena_timers();
    assert!(!masked.t0_int_ena());
    assert!(masked.t1_int_ena());
    assert!(masked.wdt_int_ena());
    timer.modify_int_ena_timers(|w| {
        w.set_t0_int_ena(true);
    });
    let enabled = timer.read_int_ena_timers();
    assert!(enabled.t0_int_ena() && enabled.t1_int_ena() && enabled.wdt_int_ena());
}

#[test]
fn the_one_shot_images_select_stopped_counting_and_armed_states() {
    let mut words = [0u32; 64];
    let mut timer = Timg0Timer0::host(&mut words).expect("host storage covers the owner");
    timer.configure_timg0_timer0_stopped();
    let stopped = timer.read_t0config();
    assert!(!stopped.t_en() && !stopped.t_alarm_en() && !stopped.t_autoreload());
    assert!(stopped.t_increase());
    assert_eq!(stopped.t_divider(), 40);
    timer.configure_timg0_timer0_counting();
    assert!(timer.read_t0config().t_en());
    assert!(!timer.read_t0config().t_alarm_en());
    timer.configure_timg0_timer0_armed();
    assert!(timer.read_t0config().t_en() && timer.read_t0config().t_alarm_en());
    assert!(!timer.read_t0config().t_autoreload());
}
