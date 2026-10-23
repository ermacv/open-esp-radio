use super::*;
#[test]
fn utc_dates_preserve_day_boundaries_and_the_full_timestamp_domain() {
    for (seconds, expected) in [
        (86_399, "1970-01-01"),
        (86_400, "1970-01-02"),
        (951_782_400, "2000-02-29"),
        (4_107_542_400, "2100-03-01"),
        (253_402_300_800, "10000-01-01"),
        (u64::MAX, "584554051223-11-09"),
    ] {
        assert_eq!(utc_date(seconds), expected);
    }
}
