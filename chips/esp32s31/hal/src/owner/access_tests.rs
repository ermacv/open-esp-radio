use super::*;
use crate::{
    ieee80211::{
        client::{WifiClocked, WifiCold},
        station_wake::StaTbttScheduleError,
    },
    root::RadioHardware,
};

#[test]
fn moving_the_radio_owner_preserves_its_runtime_state_through_handoff() {
    let (_domain, partitions) = RadioHardware::for_host_test().split(());
    let clocked = WifiClocked::for_host_test(WifiCold::from_partition(partitions.wifi));
    let (mut owner, setup, clocks) = clocked.into_running();
    assert!(!owner.initialized);
    owner.initialized = true;

    // The unscheduled outcome is decided from owned software state and does
    // not touch MMIO on the host. Each borrow returns before the owner moves.
    assert_eq!(
        owner.station_wake_hal().set_station_tbtt_interval(100),
        Err(StaTbttScheduleError::NotScheduled),
    );
    drop(owner.wifi_mac_hal());
    let clocked = WifiClocked::from_running(owner, setup, clocks);
    assert!(clocked.initialized());
    let (mut owner, _setup, _clocks) = clocked.into_running();
    assert!(owner.initialized);
    assert_eq!(
        owner.station_wake_hal().set_station_tbtt_interval(100),
        Err(StaTbttScheduleError::NotScheduled),
    );
}
