use super::*;

#[test]
fn a_span_scales_to_cycles_per_10ms() {
    assert_eq!(per_10ms(3_203_000, 10_000), Some(3_203_000));
    assert_eq!(per_10ms(320_300, 1_000), Some(3_203_000));
    assert_eq!(per_10ms(5, 0), None);
    assert_eq!(per_10ms(u64::MAX, 1), None);
}

#[test]
fn the_offset_removes_the_time_between_the_samples() {
    // Hart 1 sampled 2 µs later at 320 cycles/µs and leads by 100 cycles.
    let rate = 3_200_000;
    assert_eq!(offset((1_000, 50_000), (1_002, 50_740), rate), Some(100));
    // The same counter sampled at the same microsecond.
    assert_eq!(offset((7, 9_000), (7, 9_000), rate), Some(0));
    // Hart 1 behind, sampled earlier.
    assert_eq!(offset((1_002, 50_740), (1_000, 50_000), rate), Some(-100));
}

#[test]
fn a_line_prints_text_and_signed_decimals() {
    let mut line = Line::new();
    line.text("OER-PROBE mcycle x=")
        .number(0)
        .text(" y=")
        .number(18_446_744_073_709_551_615)
        .text(" z=")
        .signed(-42);
    assert_eq!(
        line.end(),
        b"OER-PROBE mcycle x=0 y=18446744073709551615 z=-42\r\n"
    );
}

#[test]
fn a_line_stops_at_its_capacity() {
    let mut line = Line::new();
    for _ in 0..50 {
        line.text("abcde");
    }
    assert_eq!(line.as_bytes().len(), 192);
}
