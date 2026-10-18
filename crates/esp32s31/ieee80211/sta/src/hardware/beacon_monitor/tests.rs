use super::*;

const BSSID: [u8; 6] = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];

fn binding() -> StationBeaconMonitorBinding {
    StationBeaconMonitorBinding::new(BSSID, StaAssociationId::new(7).unwrap())
}

fn policy(miss_limit: u8) -> StaBeaconLossConfig {
    StaBeaconLossConfig::new(100, miss_limit, oer_time::Duration::from_micros(1_024_000)).unwrap()
}

fn snapshot() -> MacStaReceivePolicySnapshot {
    MacStaReceivePolicySnapshot {
        bssid: BSSID,
        association_id: 7,
        minimum_mpdu_start_spacing: 0,
        bssid_address_check_enabled: true,
        interface_is_soft_ap: false,
        interface_rx_policy_enabled: true,
        beacon_filter_control: 0,
    }
}

#[test]
fn exact_binding_reaches_the_first_missing_physical_oracle() {
    let frontier = evaluate_hardware_beacon_monitor(binding(), policy(10), Some(snapshot()));
    assert_eq!(
        frontier.reached(),
        StationHardwareBeaconMonitorStage::BeaconMissLimitRepresentable
    );
    assert_eq!(
        frontier.blocker(),
        StationHardwareBeaconMonitorBlocker::MissingBeaconMissTimeoutUnitConversion
    );
    assert!(!frontier.automatic_monitor_active());
}

#[test]
fn association_identity_and_existing_filter_owner_fail_closed() {
    let mut stale = snapshot();
    stale.association_id = 8;
    assert!(matches!(
        evaluate_hardware_beacon_monitor(binding(), policy(10), Some(stale)).blocker(),
        StationHardwareBeaconMonitorBlocker::AssociationBindingMismatch {
            expected_association_id: 7,
            observed_association_id: 8,
            ..
        }
    ));

    let mut already_enabled = snapshot();
    already_enabled.beacon_filter_control = 0b111;
    assert_eq!(
        evaluate_hardware_beacon_monitor(binding(), policy(10), Some(already_enabled)).blocker(),
        StationHardwareBeaconMonitorBlocker::BeaconFilterAlreadyEnabled { control: 0b111 }
    );
}

#[test]
fn four_bit_limit_is_checked_without_truncation() {
    let frontier = evaluate_hardware_beacon_monitor(binding(), policy(16), Some(snapshot()));
    assert_eq!(
        frontier.blocker(),
        StationHardwareBeaconMonitorBlocker::BeaconMissLimitNotRepresentable {
            requested: 16,
            maximum: 15,
        }
    );
}

#[test]
fn affine_epoch_evaluates_once_and_stops_without_a_restore_claim() {
    let mut epoch = StationHardwareBeaconMonitorEpoch::new(binding(), policy(10));
    let first = epoch.evaluate_once(Some(snapshot()));

    let mut contradictory = snapshot();
    contradictory.bssid = [0; 6];
    assert_eq!(epoch.evaluate_once(Some(contradictory)), first);

    let stopped = epoch.stop();
    assert_eq!(stopped.binding, binding());
    assert_eq!(stopped.frontier, Some(first));
}
