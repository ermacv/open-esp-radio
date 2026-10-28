use super::*;

#[test]
fn invalid_public_divisors_retain_the_input_in_a_typed_error() {
    for (selector, minimum) in [
        (CoexClockSelector::Selector1, 3),
        (CoexClockSelector::Selector2, 40),
        (CoexClockSelector::Selector4, 50),
        (CoexClockSelector::Selector8, 1),
    ] {
        for divisor in 0..minimum {
            assert!(matches!(TimerClockConfig::new(selector, divisor),
                Err(TimerClockError::InvalidDivisor { selector: got, divisor: raw }) if got == selector && raw == divisor));
        }
        assert!(TimerClockConfig::new(selector, minimum).is_ok());
    }
    assert!(matches!(
        TimerClockConfig::new(CoexClockSelector::Selector8, 2),
        Err(TimerClockError::InvalidDivisor { divisor: 2, .. })
    ));
}

#[test]
fn admitted_divisors_preserve_the_vendor_field_encoding_at_wrap_boundaries() {
    let modulus = CoexTimerClockDividerMinusOne::MAX + 1;
    for selector in [
        CoexClockSelector::Selector1,
        CoexClockSelector::Selector2,
        CoexClockSelector::Selector4,
    ] {
        for divisor in [50, modulus, modulus + 1, modulus * 2, u32::MAX] {
            let config = TimerClockConfig::new(selector, divisor)
                .ok()
                .expect("admitted divisor");
            assert_eq!(
                config.divider.get(),
                divisor.wrapping_sub(1) & CoexTimerClockDividerMinusOne::MAX
            );
        }
    }
}
