use super::*;

fn channel() -> Ieee802154Channel {
    Ieee802154Channel::new(20).expect("standard channel")
}

fn reference_index(levels: &[i8], requested_dbm: i8) -> usize {
    if requested_dbm <= levels[0] {
        return 0;
    }
    if requested_dbm >= levels[levels.len() - 1] {
        return levels.len() - 1;
    }

    let mut index = levels.len() - 1;
    while index != 0 {
        if levels[index] <= requested_dbm {
            break;
        }
        index -= 1;
    }
    index
}

#[test]
fn level_validation_fails_closed_before_resolution() {
    assert_eq!(
        Ieee802154TxPowerLevels::new(&[]),
        Err(Ieee802154TxPowerLevelsError::Empty)
    );

    let too_long = [0; MAX_PROVIDER_LEVEL_COUNT + 1];
    assert_eq!(
        Ieee802154TxPowerLevels::new(&too_long),
        Err(Ieee802154TxPowerLevelsError::TooLong {
            length: MAX_PROVIDER_LEVEL_COUNT + 1,
            maximum: MAX_PROVIDER_LEVEL_COUNT,
        })
    );

    let descending = [-91, -12, 6, -4, 77];
    assert_eq!(
        Ieee802154TxPowerLevels::new(&descending),
        Err(Ieee802154TxPowerLevelsError::Descending {
            index: 3,
            previous_dbm: 6,
            current_dbm: -4,
        })
    );
}

#[test]
fn complete_source_length_domain_and_duplicate_levels_are_accepted() {
    let single = [9];
    let duplicates = [-83, -7, -7, 42];
    let maximum = [5; MAX_PROVIDER_LEVEL_COUNT];

    assert_eq!(
        Ieee802154TxPowerLevels::new(&single)
            .expect("single provider level")
            .len(),
        1
    );
    assert_eq!(
        Ieee802154TxPowerLevels::new(&duplicates)
            .expect("non-decreasing provider levels")
            .len(),
        duplicates.len()
    );
    let maximum = Ieee802154TxPowerLevels::new(&maximum).expect("largest public provider length");
    assert_eq!(maximum.len(), MAX_PROVIDER_LEVEL_COUNT);
    assert!(!maximum.is_empty());
    assert_eq!(
        maximum
            .resolve(channel(), i8::MAX)
            .selected_provider_index(),
        u8::MAX - 1
    );
}

#[test]
fn resolution_matches_the_public_scan_for_every_i8_request() {
    let synthetic_sets: [&[i8]; 4] = [
        &[11],
        &[i8::MIN, -95, -13, 2, 37, i8::MAX],
        &[-103, -8, -8, 19, 71],
        &[-64, -31, 0, 1, 56, 92],
    ];
    let channel = channel();

    for raw_levels in synthetic_sets {
        let levels =
            Ieee802154TxPowerLevels::new(raw_levels).expect("synthetic levels are non-decreasing");
        for requested_dbm in i8::MIN..=i8::MAX {
            let expected_index = reference_index(raw_levels, requested_dbm);
            let resolved = levels.resolve(channel, requested_dbm);

            assert_eq!(resolved.channel(), channel);
            assert_eq!(resolved.requested_dbm(), requested_dbm);
            assert_eq!(resolved.selected_provider_index(), expected_index as u8);
            assert_eq!(resolved.effective_dbm(), raw_levels[expected_index]);
        }
    }
}

#[test]
fn equal_levels_select_the_greatest_matching_index() {
    let raw_levels = [-83, -7, -7, -7, 42];
    let levels = Ieee802154TxPowerLevels::new(&raw_levels)
        .expect("duplicates preserve non-decreasing order");
    let resolved = levels.resolve(channel(), -7);

    assert_eq!(resolved.selected_provider_index(), 3);
    assert_eq!(resolved.effective_dbm(), -7);
}

#[test]
fn duplicated_boundary_levels_follow_public_branch_precedence() {
    let raw_levels = [-7, -7, 0, 42, 42];
    let levels = Ieee802154TxPowerLevels::new(&raw_levels)
        .expect("duplicates preserve non-decreasing order");

    let minimum = levels.resolve(channel(), -7);
    assert_eq!(minimum.selected_provider_index(), 0);
    assert_eq!(minimum.effective_dbm(), -7);

    let above_minimum = levels.resolve(channel(), -6);
    assert_eq!(above_minimum.selected_provider_index(), 1);
    assert_eq!(above_minimum.effective_dbm(), -7);

    let maximum = levels.resolve(channel(), 42);
    assert_eq!(maximum.selected_provider_index(), 4);
    assert_eq!(maximum.effective_dbm(), 42);
}
