use core::cell::Cell;

use super::*;

struct TestRunner<'a> {
    link_up: &'a Cell<u32>,
}

impl StationNetworkLink for TestRunner<'_> {
    fn publish_link_up(&self) {
        self.link_up.set(self.link_up.get() + 1);
    }
}

#[test]
fn network_device_starts_once_and_running_owner_is_reused() {
    let link_up = Cell::new(0);
    let starts = Cell::new(0);
    let unstarted = StationNetworkResources::Unstarted {
        device: 7_u8,
        runner: TestRunner { link_up: &link_up },
    };
    let started = start_station_network(unstarted, |device| {
        starts.set(starts.get() + 1);
        (u16::from(device) + 100, 9_u32)
    });
    let (stack, runner, initial_task) = started.into_parts();
    assert_eq!(stack, 107);
    assert_eq!(initial_task, Some(9));
    assert_eq!(starts.get(), 1);
    assert_eq!(link_up.get(), 1);

    let running =
        StationNetworkResources::<u8, _, _>::Running(RunningStationNetwork::new(stack, runner));
    let started = start_station_network(running, |_device| -> (u16, u32) {
        panic!("a reconnect must not construct another network stack")
    });
    let (stack, _runner, initial_task) = started.into_parts();
    assert_eq!(stack, 107);
    assert_eq!(initial_task, None);
    assert_eq!(starts.get(), 1);
    assert_eq!(link_up.get(), 2);
}
