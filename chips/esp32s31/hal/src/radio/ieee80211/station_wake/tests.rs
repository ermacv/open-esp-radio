use super::StationWakeState;

#[test]
fn a_route_starts_without_a_station_tbtt_schedule() {
    assert!(!StationWakeState::default().tbtt_scheduled);
}
