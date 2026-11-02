#![allow(clippy::unwrap_used)]

use super::{Line, nominal_ticks, rate_hz};

#[test]
fn nominal_ticks_follow_the_nominal_slow_rate() {
    assert_eq!(nominal_ticks(1_000), Some(136_000));
    assert_eq!(nominal_ticks(10), Some(1_360));
    assert_eq!(nominal_ticks(u64::MAX), None);
}

#[test]
fn the_rate_is_ticks_per_second_of_the_system_timer() {
    assert_eq!(rate_hz(13_600, 100_000), Some(136_000));
    assert_eq!(rate_hz(1, 0), None);
    assert_eq!(rate_hz(u64::MAX, 1), None);
}

#[test]
fn a_line_prints_numbers_and_hex_words() {
    let mut line = Line::new();
    assert_eq!(
        line.text("a=")
            .number(1_024)
            .text(" b=")
            .hex(0x0000_2a0f)
            .end(),
        b"a=1024 b=0x00002a0f\r\n"
    );
}
