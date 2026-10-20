use super::{
    PeripheralConnectionCoexistenceProtection, SchedulerItemCoexistencePriority, with_priorities,
};
use oer_esp32s31_hal::layouts::BleSchedulerItemCoexistencePriority;
fn lane(value: u8) -> SchedulerItemCoexistencePriority {
    SchedulerItemCoexistencePriority::new(value).expect("test priority fits")
}
#[test]
fn priorities_replace_each_lane_and_preserve_the_other_fields() {
    let original = BleSchedulerItemCoexistencePriority::ZERO
        .with_bits_20_31_opaque(0xa5a)
        .expect("test opaque image fits");
    for lanes in [[4, 0, 13, 13], [31; 4], [0; 4]] {
        let image = with_priorities(original, &lanes.map(lane));
        assert_eq!(
            [
                image.lane_0(),
                image.lane_1(),
                image.lane_2(),
                image.lane_3()
            ],
            lanes
        );
        assert_eq!(image.bits_20_31_opaque(), original.bits_20_31_opaque());
    }
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
