use super::*;

const LEVELS: TransmitPowerLevels = match TransmitPowerLevels::new(&[-15, -5, 0, 0, 10, 20]) {
    Ok(levels) => levels,
    Err(_) => panic!("valid levels"),
};

/// A level set must be non-empty and non-decreasing.
#[test]
fn level_sets_are_validated() {
    assert_eq!(
        TransmitPowerLevels::new(&[]),
        Err(TransmitPowerLevelsError::Empty)
    );
    assert_eq!(
        TransmitPowerLevels::new(&[0, 5, 3]),
        Err(TransmitPowerLevelsError::Descending { index: 2 })
    );
    assert_eq!(LEVELS.lowest_dbm(), -15);
    assert_eq!(LEVELS.highest_dbm(), 20);
}

/// A request selects the greatest level not above it, never more power
/// than asked.
#[test]
fn a_request_selects_the_greatest_level_not_above_it() {
    assert_eq!(LEVELS.select(-15), Ok(-15));
    assert_eq!(LEVELS.select(-6), Ok(-15));
    assert_eq!(LEVELS.select(3), Ok(0));
    assert_eq!(LEVELS.select(19), Ok(10));
    assert_eq!(LEVELS.select(20), Ok(20));
}

/// A request outside the levels is refused, returning it, instead of being
/// clamped.
#[test]
fn a_request_outside_the_levels_is_refused() {
    for requested_dbm in [-16, 21] {
        assert_eq!(
            LEVELS.select(requested_dbm),
            Err(TransmitPowerError::OutOfRange {
                requested_dbm,
                lowest_dbm: -15,
                highest_dbm: 20,
            })
        );
    }
    assert_eq!(
        TransmitPowerCapability::check(None, 0),
        Err(TransmitPowerError::Unsupported)
    );
}

/// A CCA threshold is accepted inside its range and refused outside it.
#[test]
fn a_cca_threshold_is_checked_against_its_range() {
    let capability = Some(CcaThresholdCapability {
        lowest_dbm: -120,
        highest_dbm: 0,
        reset_dbm: -75,
    });
    assert_eq!(CcaThresholdCapability::check(capability, -120), Ok(()));
    assert_eq!(CcaThresholdCapability::check(capability, 0), Ok(()));
    assert_eq!(
        CcaThresholdCapability::check(capability, 1),
        Err(CcaThresholdError::OutOfRange {
            requested_dbm: 1,
            lowest_dbm: -120,
            highest_dbm: 0,
        })
    );
    assert_eq!(
        CcaThresholdCapability::check(None, -75),
        Err(CcaThresholdError::Unsupported)
    );
}
