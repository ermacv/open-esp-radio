use super::*;

#[derive(Default)]
struct RegisterHardware {
    configured: Option<oer_esp32s31_ieee80211_mac::MacStaApReceivePlan>,
    disabled: std::vec::Vec<oer_esp32s31_ieee80211_mac::MacInterface>,
}

impl StaApRegisterHardware for RegisterHardware {
    fn apply_sta_ap_receive_registers(
        &mut self,
        plan: oer_esp32s31_ieee80211_mac::MacStaApReceivePlan,
    ) {
        self.configured = Some(plan);
    }

    fn disable_station_receive_registers(&mut self) {
        self.disabled
            .push(oer_esp32s31_ieee80211_mac::MacInterface::Station);
    }

    fn disable_access_point_receive_registers(&mut self) {
        self.disabled
            .push(oer_esp32s31_ieee80211_mac::MacInterface::AccessPoint);
    }

    fn disable_all_role_receive_registers(&mut self) {
        self.disable_station_receive_registers();
        self.disable_access_point_receive_registers();
    }
}

const IDENTITIES: StaApReceiveIdentities = StaApReceiveIdentities {
    station_address: [2, 0, 0, 0, 0, 1],
    station_bssid: [2, 0, 0, 0, 0, 2],
    access_point_address: [2, 0, 0, 0, 0, 3],
};

fn channel(primary: u8) -> WifiChannel {
    WifiChannel::mhz20(primary).unwrap()
}

#[test]
fn station_then_access_point_preserves_station_until_its_own_stop() {
    let mut lifecycle = StaApLifecycle::new();
    let shared = channel(6);

    assert_eq!(
        lifecycle.start_station(shared),
        Ok(StaApTransition::StartStationCold)
    );
    assert_eq!(
        lifecycle.start_access_point(shared),
        Ok(StaApTransition::StartAccessPointPreserveStation)
    );
    assert_eq!(
        lifecycle.stop_access_point(),
        Ok(StaApTransition::StopAccessPointPreserveStation)
    );
    assert_eq!(
        lifecycle.state(),
        StaApLifecycleState::Station { channel: shared }
    );
    assert_eq!(
        lifecycle.stop_station(),
        Ok(StaApTransition::StopStationLastRole)
    );
    assert_eq!(lifecycle.state(), StaApLifecycleState::Idle);
}

#[test]
fn access_point_then_station_preserves_access_point_until_its_own_stop() {
    let mut lifecycle = StaApLifecycle::new();
    let shared = channel(11);

    assert_eq!(
        lifecycle.start_access_point(shared),
        Ok(StaApTransition::StartAccessPointCold)
    );
    assert_eq!(
        lifecycle.start_station(shared),
        Ok(StaApTransition::StartStationPreserveAccessPoint)
    );
    assert_eq!(
        lifecycle.stop_station(),
        Ok(StaApTransition::StopStationPreserveAccessPoint)
    );
    assert_eq!(
        lifecycle.state(),
        StaApLifecycleState::AccessPoint { channel: shared }
    );
    assert_eq!(
        lifecycle.stop_access_point(),
        Ok(StaApTransition::StopAccessPointLastRole)
    );
    assert_eq!(lifecycle.state(), StaApLifecycleState::Idle);
}

#[test]
fn a_second_role_cannot_silently_move_the_shared_radio() {
    let mut lifecycle = StaApLifecycle::new();
    lifecycle.start_station(channel(1)).unwrap();

    assert_eq!(
        lifecycle.start_access_point(channel(6)),
        Err(StaApLifecycleError::ChannelConflict {
            active: channel(1),
            requested: channel(6),
        })
    );
    assert_eq!(
        lifecycle.state(),
        StaApLifecycleState::Station {
            channel: channel(1)
        }
    );
}

#[test]
fn duplicate_and_inactive_operations_fail_without_state_change() {
    let mut lifecycle = StaApLifecycle::new();
    let shared = channel(3);
    assert_eq!(
        lifecycle.stop_station(),
        Err(StaApLifecycleError::RoleInactive(StaApRole::Station))
    );
    lifecycle.start_access_point(shared).unwrap();
    assert_eq!(
        lifecycle.start_access_point(shared),
        Err(StaApLifecycleError::RoleAlreadyActive(
            StaApRole::AccessPoint
        ))
    );
    assert_eq!(
        lifecycle.state(),
        StaApLifecycleState::AccessPoint { channel: shared }
    );
}

#[test]
fn preserve_edges_map_to_one_complete_register_action() {
    let mut hardware = RegisterHardware::default();
    apply_sta_ap_register_action(
        &mut hardware,
        sta_ap_register_action(StaApTransition::StartAccessPointPreserveStation, IDENTITIES),
    );
    let plan = hardware.configured.expect("combined plan");
    assert_eq!(plan.station_address(), IDENTITIES.station_address);
    assert_eq!(plan.station_bssid(), IDENTITIES.station_bssid);
    assert_eq!(plan.access_point_address(), IDENTITIES.access_point_address);

    apply_sta_ap_register_action(
        &mut hardware,
        sta_ap_register_action(StaApTransition::StopAccessPointPreserveStation, IDENTITIES),
    );
    apply_sta_ap_register_action(
        &mut hardware,
        sta_ap_register_action(StaApTransition::StopStationPreserveAccessPoint, IDENTITIES),
    );
    assert_eq!(
        hardware.disabled,
        [
            oer_esp32s31_ieee80211_mac::MacInterface::AccessPoint,
            oer_esp32s31_ieee80211_mac::MacInterface::Station,
        ]
    );
}

#[test]
fn cold_and_last_role_edges_do_not_duplicate_existing_transactions() {
    for transition in [
        StaApTransition::StartStationCold,
        StaApTransition::StartAccessPointCold,
        StaApTransition::StopStationLastRole,
        StaApTransition::StopAccessPointLastRole,
    ] {
        assert_eq!(
            sta_ap_register_action(transition, IDENTITIES),
            StaApRegisterAction::None
        );
    }
}
