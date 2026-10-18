use super::*;

#[test]
fn role_transition_is_exact() {
    let evidence = WifiRoleTransitionEvidence {
        previous: WifiRole::Station,
        current: WifiRole::Idle,
        generation: 3,
    };
    assert!(require_transition(evidence, WifiRole::Station, WifiRole::Idle).is_ok());
    assert!(require_transition(evidence, WifiRole::Idle, WifiRole::Station).is_err());
}

#[test]
fn radio_restart_requires_the_next_generation_and_a_closed_and_woken_rf() {
    let stopped = WifiRoleTransitionEvidence {
        previous: WifiRole::Station,
        current: WifiRole::Idle,
        generation: 7,
    };
    let restarted = WifiRadioRestartEvidence {
        generation: 8,
        rf: WifiRadioRestartRf::ClosedAndWoken,
    };
    assert!(require_radio_restart(stopped, restarted, WifiRadioRestartRf::ClosedAndWoken).is_ok());
    assert!(
        require_radio_restart(
            stopped,
            WifiRadioRestartEvidence {
                generation: 7,
                ..restarted
            },
            WifiRadioRestartRf::ClosedAndWoken,
        )
        .is_err()
    );
    assert!(
        require_radio_restart(
            stopped,
            WifiRadioRestartEvidence {
                rf: WifiRadioRestartRf::KeptOpen,
                ..restarted
            },
            WifiRadioRestartRf::ClosedAndWoken,
        )
        .is_err(),
        "the last PHY client's release must close RF"
    );
}

#[test]
fn station_after_radio_restart_requires_the_next_generation() {
    let restarted = WifiRadioRestartEvidence {
        generation: 8,
        rf: WifiRadioRestartRf::ClosedAndWoken,
    };
    let started = WifiRoleTransitionEvidence {
        previous: WifiRole::Idle,
        current: WifiRole::Station,
        generation: 9,
    };
    assert!(require_station_after_lifecycle(restarted.generation, started).is_ok());
    assert!(
        require_station_after_lifecycle(
            restarted.generation,
            WifiRoleTransitionEvidence {
                generation: 8,
                ..started
            }
        )
        .is_err()
    );
}

#[test]
fn lifecycle_stop_requires_the_current_station_link_disconnect() {
    let disconnect = StationLifecycleEvent::Disconnected {
        generation: u32::MAX,
        reason: StationDisconnectReason::LinkPolicy,
    };
    assert!(require_lifecycle_disconnect(disconnect, u32::MAX).is_ok());
    assert!(require_lifecycle_disconnect(disconnect, 0).is_err());
    assert!(
        require_lifecycle_disconnect(
            StationLifecycleEvent::Disconnected {
                generation: u32::MAX,
                reason: StationDisconnectReason::BeaconLoss,
            },
            u32::MAX,
        )
        .is_err()
    );
    assert!(
        require_lifecycle_disconnect(
            StationLifecycleEvent::Connected {
                generation: u32::MAX,
                association_bandwidth_mhz: Some(40),
                security: Some(oer_hil_protocol::wifi::StationLinkSecurity::Wpa2Personal {
                    management_protection: false
                }),
            },
            u32::MAX,
        )
        .is_err()
    );
}

fn link(
    phy: PhyExpectation,
    management_frame_protection: oer_hil_scenario_catalog::link::ManagementFrameProtection,
) -> crate::scenario::LinkExpectation {
    crate::scenario::LinkExpectation {
        phy,
        minimum_mcs: None,
        guard_interval: Default::default(),
        management_frame_protection,
        access_point_security: Default::default(),
    }
}

#[test]
fn a_wpa3_fixture_requires_an_sae_link() {
    use oer_hil_scenario_catalog::link::{
        AccessPointSecurity, ManagementFrameProtection::Disabled,
    };
    let wpa3 = crate::scenario::LinkExpectation {
        access_point_security: AccessPointSecurity::Wpa3Transition,
        ..link(PhyExpectation::Ht40, Disabled)
    };
    let sae = StationConnectionObservation {
        generation: 1,
        association_bandwidth_mhz: Some(40),
        security: Some(oer_hil_protocol::wifi::StationLinkSecurity::Wpa3Personal),
        event_cursor_after: 3,
    };
    assert!(require_station_link(sae, wpa3).is_ok());
    let psk = StationConnectionObservation {
        security: Some(oer_hil_protocol::wifi::StationLinkSecurity::Wpa2Personal {
            management_protection: true,
        }),
        ..sae
    };
    assert!(require_station_link(psk, wpa3).is_err());
}

#[test]
fn lifecycle_connected_link_requires_negotiated_phy_and_security() {
    use oer_hil_scenario_catalog::link::ManagementFrameProtection::{Disabled, Required};
    let ht40_wpa2 = StationConnectionObservation {
        generation: 4,
        association_bandwidth_mhz: Some(40),
        security: Some(oer_hil_protocol::wifi::StationLinkSecurity::Wpa2Personal {
            management_protection: false,
        }),
        event_cursor_after: 3,
    };
    assert!(require_station_link(ht40_wpa2, link(PhyExpectation::Ht40, Disabled)).is_ok());
    assert!(require_station_link(ht40_wpa2, link(PhyExpectation::Ht20, Disabled)).is_err());
    // A fixture offering protection requires the station to negotiate it.
    assert!(require_station_link(ht40_wpa2, link(PhyExpectation::Ht40, Required)).is_err());
    assert!(
        require_station_link(
            StationConnectionObservation {
                security: Some(oer_hil_protocol::wifi::StationLinkSecurity::Wpa2Personal {
                    management_protection: true,
                }),
                ..ht40_wpa2
            },
            link(PhyExpectation::Ht40, Required),
        )
        .is_ok()
    );
    assert!(
        require_station_link(
            StationConnectionObservation {
                security: Some(oer_hil_protocol::wifi::StationLinkSecurity::Open),
                ..ht40_wpa2
            },
            link(PhyExpectation::Ht40, Disabled),
        )
        .is_err()
    );
    assert!(
        require_station_link(
            StationConnectionObservation {
                association_bandwidth_mhz: None,
                ..ht40_wpa2
            },
            link(PhyExpectation::Ht40, Disabled),
        )
        .is_err()
    );
}

#[test]
fn three_restart_cycles_require_continuous_role_generations() {
    let mut role_generation = u32::MAX - 1;
    for _cycle in 1..=3 {
        let stopped = WifiRoleTransitionEvidence {
            previous: WifiRole::Station,
            current: WifiRole::Idle,
            generation: role_generation,
        };
        let restarted = WifiRadioRestartEvidence {
            generation: role_generation.wrapping_add(1),
            rf: WifiRadioRestartRf::ClosedAndWoken,
        };
        assert!(
            require_radio_restart(stopped, restarted, WifiRadioRestartRf::ClosedAndWoken).is_ok()
        );
        let started = WifiRoleTransitionEvidence {
            previous: WifiRole::Idle,
            current: WifiRole::Station,
            generation: role_generation.wrapping_add(2),
        };
        assert!(require_station_after_lifecycle(restarted.generation, started).is_ok());
        role_generation = started.generation;
    }
}

#[test]
fn typed_configuration_preserves_workload_bounds() {
    assert!(
        Config {
            monitor_channel: Some(14),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        Config {
            snapshot_length: 2305,
            ..Default::default()
        }
        .validate()
        .is_err()
    );
}
