use super::*;
use crate::tracking::calibration::PhyCalibrationTrackingRequest;
use crate::{
    PhyConfig,
    analog::temperature::PhyTemperatureOutcome,
    state::client::PhyClientState,
    tracking::{
        calibration::{PhyCalibrationTrackingAction, PhyCalibrationTrackingTransition},
        i2c::{PhyWifiI2cTrackingAction, PhyWifiI2cTrackingTransition},
    },
};
fn clients(active: &[RadioClient]) -> PhyClientSnapshot {
    let mut owner = PhyClientState::without_registration(1000);
    for client in active {
        owner = owner
            .acquire(*client, &oer_time_virtual::VirtualClock::<8>::new())
            .unwrap_or_else(|_| panic!("acquire"))
            .into_owner()
            .unwrap_or_else(|_| panic!("unexpected due work"));
    }
    owner.snapshot()
}
fn state(temperature: i16) -> PhyState {
    let mut state = PhyState::new(PhyConfig::production());
    state.apply_temperature_outcome(PhyTemperatureOutcome {
        temperature,
        sensor_index: 0,
        next_dac: 0,
    });
    state
}
#[test]
fn due_evaluation_does_not_imply_due_calibration_or_commit_references() {
    let state = state(0);
    let registered = RegisteredPhyState::from_wrapper_test_model(state);
    let clients = clients(&[RadioClient::Wifi]);
    let before = registered.state().calibration_tracking_parameters(None);
    let first = Inspection::registered(&registered, clients, 1001).unwrap();
    let later = Inspection::registered(&registered, clients, 2000).unwrap();
    assert_eq!(first, later);
    assert!(matches!(first.schedule, Schedule::Due(_)));
    let decision = first.wifi.unwrap().calibration.unwrap();
    assert!(!decision.common.is_due() && !decision.transmit.is_due());
    assert_eq!(
        registered.state().calibration_tracking_parameters(None),
        before
    );
    assert!(
        !first.rfpll.expect("registered RFPLL predicate").is_due(),
        "a due scheduler evaluation must not invent thermal RFPLL demand"
    );
}
#[test]
fn thermal_conditions_and_scheduler_deadline_are_independent() {
    let state = state(30);
    let policy = PhyParamTrackingPolicy::for_registered_state(&state);
    let inspection =
        Inspection::inspect(&state, policy, clients(&[RadioClient::Wifi]), 1000).unwrap();
    assert_eq!(inspection.schedule, Schedule::At(1001));
    let decision = inspection.wifi.unwrap().calibration.unwrap();
    assert!(decision.common.is_due() && decision.transmit.is_due());
    let transition = PhyCalibrationTrackingTransition::new(
        PhyCalibrationTrackingRequest {
            clients: (Class::Wifi).clients(),
        },
        state.calibration_tracking_parameters(None),
    );
    assert_eq!(
        transition.action(),
        PhyCalibrationTrackingAction::SetGrantProtect { enabled: true }
    );
}
#[test]
fn inactive_inhibited_and_shared_clients_do_not_invent_work() {
    let state = state(50);
    let policy = PhyParamTrackingPolicy::for_registered_state(&state);
    let empty = Inspection::inspect(&state, policy, clients(&[]), 1001).unwrap();
    assert_eq!(empty.schedule, Schedule::Inactive);
    assert!(empty.wifi.is_none() && empty.bluetooth_ieee802154.is_none() && empty.rfpll.is_none());
    for active in [
        &[RadioClient::Bluetooth][..],
        &[RadioClient::Ieee802154][..],
        &[RadioClient::Bluetooth, RadioClient::Ieee802154][..],
    ] {
        let shared = Inspection::inspect(&state, policy, clients(active), 1001).unwrap();
        assert!(shared.wifi.is_none() && shared.wifi_i2c.is_none());
        assert!(shared.bluetooth_ieee802154.is_some());
    }
    let inhibited = Inspection::inspect(
        &state,
        PhyParamTrackingPolicy {
            tracking_inhibited: true,
            ..policy
        },
        clients(&[RadioClient::Wifi]),
        1001,
    )
    .unwrap();
    assert!(inhibited.inhibited);
    assert!(matches!(inhibited.schedule, Schedule::Due(_))); // Due is not execution.
    assert!(inhibited.wifi.is_none() && inhibited.wifi_i2c.is_none());
}
#[test]
fn optional_children_follow_policy_and_real_predicates() {
    for (temperature, due) in [
        (-15, true),
        (-14, false),
        (5, false),
        (14, false),
        (15, true),
        (30, true),
        (95, true),
    ] {
        let state = state(temperature);
        let mut policy = PhyParamTrackingPolicy::for_registered_state(&state);
        policy.rfpll_cap_tracking_enabled = true;
        policy.calibration_tracking_enabled = false;
        let inspection =
            Inspection::inspect(&state, policy, clients(&[RadioClient::Wifi]), 1001).unwrap();
        assert!(inspection.wifi.unwrap().calibration.is_none());
        let rfpll = inspection.rfpll.unwrap();
        assert_eq!(rfpll.is_due(), due);
        assert_eq!(
            rfpll.is_due(),
            matches!(
                crate::tracking::rfpll::thermal::Transition::new(rfpll).action(),
                crate::tracking::rfpll::thermal::Action::SetGrantProtect { enabled: true }
            )
        );
        let i2c = inspection.wifi_i2c.unwrap();
        assert_eq!(
            i2c.update_required(),
            matches!(
                PhyWifiI2cTrackingTransition::new(i2c).action(),
                PhyWifiI2cTrackingAction::MaskedWrite(_)
            )
        );
    }
}
#[test]
fn inspection_rejects_reversed_clock_and_uses_updated_state_on_reinspection() {
    let mut state = state(0);
    let policy = PhyParamTrackingPolicy::for_registered_state(&state);
    let clients = clients(&[RadioClient::Wifi]);
    let before = Inspection::inspect(&state, policy, clients, 1001).unwrap();
    state.apply_temperature_outcome(PhyTemperatureOutcome {
        temperature: 30,
        sensor_index: 0,
        next_dac: 0,
    });
    let after = Inspection::inspect(&state, policy, clients, 1001).unwrap();
    assert!(!before.wifi.unwrap().calibration.unwrap().common.is_due());
    assert!(after.wifi.unwrap().calibration.unwrap().common.is_due());
    let later = PhyClientState::without_registration(1000)
        .acquire(
            RadioClient::Wifi,
            &oer_time_virtual::VirtualClock::<8>::starting_at(oer_time::Instant::from_micros(2000)),
        )
        .unwrap_or_else(|_| panic!("acquire"))
        .into_owner()
        .err()
        .expect("initial tracking must be due");
    assert!(Inspection::inspect(&state, policy, later.snapshot(), 1999).is_err());
}
