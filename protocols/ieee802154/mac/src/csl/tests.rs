use super::{CSL_UNIT_MICROS, csl_phase};

/// The phase counts ten-symbol units to the next sample time, plus one.
#[test]
fn the_phase_counts_to_the_next_sample_time() {
    let period = 100; // 16 ms
    let period_us = u32::from(period) * CSL_UNIT_MICROS;
    // At the sample time the next one is a whole period away... modulo the
    // period, zero units, plus one.
    assert_eq!(csl_phase(5_000, 5_000, period), Some(1));
    // 1600 us before the sample time: ten units.
    assert_eq!(csl_phase(5_000 - 1_600, 5_000, period), Some(11));
    // Just after the sample time: almost a period.
    assert_eq!(
        csl_phase(5_001, 5_000, period),
        Some(((period_us - 1) / CSL_UNIT_MICROS + 1) as u16)
    );
    // The clock and sample time wrap in 32 bits.
    assert_eq!(
        csl_phase(u32::MAX, 5_000 + period_us, period),
        csl_phase(u32::MAX % period_us, 5_000, period)
    );
    assert_eq!(csl_phase(1, 2, 0), None);
}
