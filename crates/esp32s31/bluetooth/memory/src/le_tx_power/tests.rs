use super::LeTxPower;

#[test]
fn a_request_selects_the_highest_provider_level_not_above_it() {
    let cases = [
        (-24, -24),
        (-22, -24),
        (-16, -18),
        (-1, -3),
        (0, 0),
        (2, 0),
        (3, 3),
        (19, 18),
        (20, 18),
        (21, 21),
        (i8::MAX, 21),
    ];

    for (dbm, level) in cases {
        let power = LeTxPower::from_dbm(dbm).expect("the table covers the request");
        assert_eq!(power.level_dbm(), level, "{dbm} dBm");
    }
}

#[test]
fn a_request_below_the_lowest_level_is_refused() {
    assert_eq!(LeTxPower::from_dbm(-25), None);
    assert_eq!(LeTxPower::from_dbm(i8::MIN), None);
}

#[test]
fn the_index_orders_like_the_levels() {
    let low = LeTxPower::from_dbm(-24).expect("lowest level");
    let high = LeTxPower::from_dbm(21).expect("highest level");
    assert_eq!(low.index(), 0);
    assert_eq!(high.index(), 15);
}
