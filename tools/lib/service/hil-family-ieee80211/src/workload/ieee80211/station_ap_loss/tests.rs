use super::*;

#[test]
fn link_policy_disconnect_cannot_qualify_beacon_loss() {
    let error = validate_event(
        StationLifecycleEvent::Disconnected {
            generation: 0,
            reason: StationDisconnectReason::LinkPolicy,
        },
        StationLifecycleEvent::Disconnected {
            generation: 0,
            reason: StationDisconnectReason::BeaconLoss,
        },
        "beacon loss",
    )
    .unwrap_err();
    assert!(error.to_string().contains("LinkPolicy"));
}

#[test]
fn typed_configuration_preserves_workload_bounds() {
    assert!(
        Config {
            timeout: Duration::from_secs(29),
            ..Config::default()
        }
        .validate()
        .is_err()
    );
}
