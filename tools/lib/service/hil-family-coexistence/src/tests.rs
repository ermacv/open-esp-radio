use super::*;

fn scenario() -> CoexistenceScenario {
    CoexistenceScenario {
        phy: PhyExpectation::Ht20,
        duration_seconds: 12,
        rx_bps: 20_000_000,
        payload_bytes: 1_472,
        minimum_rx_bps: 10_000_000,
        minimum_echoes: 20,
        phy_tracking_hold: None,
    }
}

#[test]
fn the_joint_image_needs_the_station_fixture_and_the_adapter() {
    let plan = scenario().plan();
    assert_eq!(plan.image, ImageClass::WifiBleCoex);
    assert!(plan.requirements.station_network && plan.requirements.bluetooth_adapter);
    assert_eq!(plan.wifi.link, Some(PhyExpectation::Ht20));
    assert_eq!(plan.checks, CHECKS);
}

#[test]
fn traffic_stays_within_one_bounded_phase() {
    assert!(scenario().validate().is_ok());
    for invalid in [
        CoexistenceScenario {
            duration_seconds: 17,
            ..scenario()
        },
        CoexistenceScenario {
            minimum_rx_bps: 30_000_000,
            ..scenario()
        },
        CoexistenceScenario {
            minimum_echoes: 0,
            ..scenario()
        },
    ] {
        assert!(invalid.validate().is_err(), "{invalid:?}");
    }
}

/// A tracking hold selects the joint image that times ticks, and needs
/// traffic long enough for every branch set's due ticks.
#[test]
fn a_tracking_hold_selects_its_image_and_needs_its_window() {
    let hold = CoexistenceScenario {
        phy_tracking_hold: Some(2),
        ..scenario()
    };
    assert!(hold.validate().is_ok());
    assert_eq!(
        hold.plan().image,
        ImageClass::DiagnosticWifiBleCoexTrackingHold
    );
    for invalid in [
        CoexistenceScenario {
            phy_tracking_hold: Some(3),
            ..scenario()
        },
        CoexistenceScenario {
            phy_tracking_hold: Some(0),
            ..scenario()
        },
    ] {
        assert!(invalid.validate().is_err(), "{invalid:?}");
    }
}
