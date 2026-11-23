use super::*;

const RATE: Rate = Rate {
    ticks: 160_000,
    cycles: 3_200_000,
};

#[test]
fn a_target_s_cycle_is_counted_from_the_reference_at_the_boot_s_rate() {
    assert_eq!(RATE.cycles_of(1_600), 32_000);
    assert_eq!(RATE.expected(100, 800), 16_100);
    // The counter wraps: the expected cycle wraps with it.
    assert_eq!(RATE.expected(u32::MAX - 9, 1), 10);
    // A rate of no ticks has no cycles: the greatest, never a division by zero.
    assert_eq!(
        Rate {
            ticks: 0,
            cycles: 1
        }
        .cycles_of(1),
        u32::MAX
    );
}

#[test]
fn an_offset_is_signed_across_the_counter_s_wrap() {
    assert_eq!(offset(5, u32::MAX - 4), 10);
    assert_eq!(offset(u32::MAX - 4, 5), -10);
    assert!(before(u32::MAX, 3));
    assert!(!before(3, u32::MAX));
    assert!(!before(7, 7));
}

#[test]
fn a_window_keeps_its_count_and_its_first_two_alarms() {
    let one = Seen::NONE.with(10);
    assert_eq!(
        (one.count, one.first, one.second, one.bin()),
        (1, Some(10), None, 1)
    );
    let three = one.with(20).with(30);
    assert_eq!(
        (three.count, three.first, three.second, three.bin()),
        (3, Some(10), Some(20), 2)
    );
    assert_eq!(Seen::NONE.bin(), 0);
}
