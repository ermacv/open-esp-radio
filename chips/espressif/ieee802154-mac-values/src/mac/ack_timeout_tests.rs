// SPDX-License-Identifier: Apache-2.0
use super::{IEEE802154_MAX_ACK_TIMEOUT_MICROSECONDS, Ieee802154AckTimeout};

#[test]
fn timeout_conversion_is_exhaustive_over_every_accepted_microsecond() {
    for microseconds in 0..=IEEE802154_MAX_ACK_TIMEOUT_MICROSECONDS {
        let timeout = Ieee802154AckTimeout::from_microseconds(microseconds)
            .expect("enumerated accepted value");
        let expected_units = microseconds / 16 + u32::from(microseconds % 16 != 0);
        assert_eq!(u32::from(timeout.units()), expected_units);
        assert!(timeout.effective_microseconds() >= microseconds);
        assert!(timeout.effective_microseconds() - microseconds < 16);
    }
}

#[test]
fn timeout_boundaries_and_complete_field_domain_are_honest() {
    for units in 0..=u16::MAX {
        let timeout = Ieee802154AckTimeout::from_units(units);
        assert_eq!(timeout.units(), units);
        assert_eq!(
            Ieee802154AckTimeout::from_microseconds(timeout.effective_microseconds()),
            Ok(timeout)
        );
    }

    assert_eq!(
        Ieee802154AckTimeout::from_microseconds(1)
            .expect("one quantum")
            .effective_microseconds(),
        16
    );
    assert_eq!(
        Ieee802154AckTimeout::from_microseconds(IEEE802154_MAX_ACK_TIMEOUT_MICROSECONDS)
            .expect("maximum")
            .units(),
        u16::MAX
    );
    for microseconds in [1_048_561, 1_048_575, 1_048_576, u32::MAX] {
        assert_eq!(
            Ieee802154AckTimeout::from_microseconds(microseconds)
                .expect_err("overflowing field must reject, not wrap")
                .attempted_microseconds(),
            microseconds
        );
    }
}
