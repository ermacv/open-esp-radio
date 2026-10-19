use super::{
    PeripheralConnectionCoexistenceProtection, SchedulerItemCoexistencePriority, lanes_image,
};

fn lane(value: u8) -> SchedulerItemCoexistencePriority {
    SchedulerItemCoexistencePriority::new(value).unwrap()
}

#[test]
fn lanes_fill_five_bits_each_from_lane_zero() {
    assert_eq!(
        lanes_image(&[lane(4), lane(0), lane(13), lane(13)]),
        0x0006_b404
    );
    assert_eq!(lanes_image(&[lane(9), lane(4)]), 0x89);
    assert_eq!(
        lanes_image(&[lane(31), lane(31), lane(31), lane(31)]),
        super::LANES_MASK
    );
}

#[test]
fn values_outside_their_fields_are_refused() {
    assert!(SchedulerItemCoexistencePriority::new(32).is_none());
    assert!(PeripheralConnectionCoexistenceProtection::new(64).is_none());
    assert_eq!(
        PeripheralConnectionCoexistenceProtection::new(63).map(|p| p.value()),
        Some(63)
    );
}
