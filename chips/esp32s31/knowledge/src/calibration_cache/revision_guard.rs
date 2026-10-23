//! Host-only revision sentinel: all production PHY source files, including
//! calibration, state/profile, cold replay and target publication adapters.
//! Inputs are embedded only in this test; no digest enters a firmware image.
use super::{CALIBRATION_ALGORITHM_REVISION, CALIBRATION_PROFILE_ID};
use sha2::{Digest, Sha256};

const INPUTS: &[(&str, &str)] = &[
    (
        "analog/crystal_duty.rs",
        include_str!("../../../phy/src/analog/crystal_duty.rs"),
    ),
    (
        "analog/dcode.rs",
        include_str!("../../../phy/src/analog/dcode.rs"),
    ),
    (
        "analog/frequency.rs",
        include_str!("../../../phy/src/analog/frequency.rs"),
    ),
    (
        "analog/i2c.rs",
        include_str!("../../../phy/src/analog/i2c.rs"),
    ),
    (
        "analog/pbus/memory.rs",
        include_str!("../../../phy/src/analog/pbus/memory.rs"),
    ),
    (
        "analog/pbus.rs",
        include_str!("../../../phy/src/analog/pbus.rs"),
    ),
    (
        "analog/rfpll.rs",
        include_str!("../../../phy/src/analog/rfpll.rs"),
    ),
    (
        "analog/temperature.rs",
        include_str!("../../../phy/src/analog/temperature.rs"),
    ),
    ("analog.rs", include_str!("../../../phy/src/analog.rs")),
    (
        "bluetooth_client.rs",
        include_str!("../../../phy/src/bluetooth_client.rs"),
    ),
    (
        "calibration/baseband.rs",
        include_str!("../../../phy/src/calibration/baseband.rs"),
    ),
    (
        "calibration/bluetooth.rs",
        include_str!("../../../phy/src/calibration/bluetooth.rs"),
    ),
    (
        "calibration/cold.rs",
        include_str!("../../../phy/src/calibration/cold.rs"),
    ),
    (
        "calibration/estimator.rs",
        include_str!("../../../phy/src/calibration/estimator.rs"),
    ),
    (
        "calibration/math.rs",
        include_str!("../../../phy/src/calibration/math.rs"),
    ),
    (
        "calibration/registration/diagnostics.rs",
        include_str!("../../../phy/src/calibration/registration/diagnostics.rs"),
    ),
    (
        "calibration/registration.rs",
        include_str!("../../../phy/src/calibration/registration.rs"),
    ),
    (
        "calibration.rs",
        include_str!("../../../phy/src/calibration.rs"),
    ),
    ("channel.rs", include_str!("../../../phy/src/channel.rs")),
    (
        "concurrent.rs",
        include_str!("../../../phy/src/concurrent.rs"),
    ),
    ("domain.rs", include_str!("../../../phy/src/domain.rs")),
    (
        "executor/run.rs",
        include_str!("../../../phy/src/executor/run.rs"),
    ),
    (
        "executor/rx/hardware.rs",
        include_str!("../../../phy/src/executor/rx/hardware.rs"),
    ),
    (
        "executor/rx.rs",
        include_str!("../../../phy/src/executor/rx.rs"),
    ),
    (
        "executor/step/channel_gain.rs",
        include_str!("../../../phy/src/executor/step/channel_gain.rs"),
    ),
    (
        "executor/step/i2c.rs",
        include_str!("../../../phy/src/executor/step/i2c.rs"),
    ),
    (
        "executor/step/io.rs",
        include_str!("../../../phy/src/executor/step/io.rs"),
    ),
    (
        "executor/step/power.rs",
        include_str!("../../../phy/src/executor/step/power.rs"),
    ),
    (
        "executor/step/rfpll.rs",
        include_str!("../../../phy/src/executor/step/rfpll.rs"),
    ),
    (
        "executor/step/temperature.rs",
        include_str!("../../../phy/src/executor/step/temperature.rs"),
    ),
    (
        "executor/step/wifi_i2c.rs",
        include_str!("../../../phy/src/executor/step/wifi_i2c.rs"),
    ),
    (
        "executor/step.rs",
        include_str!("../../../phy/src/executor/step.rs"),
    ),
    (
        "executor/wait/i2c.rs",
        include_str!("../../../phy/src/executor/wait/i2c.rs"),
    ),
    (
        "executor/wait/poll.rs",
        include_str!("../../../phy/src/executor/wait/poll.rs"),
    ),
    (
        "executor/wait/tx.rs",
        include_str!("../../../phy/src/executor/wait/tx.rs"),
    ),
    (
        "executor/wait.rs",
        include_str!("../../../phy/src/executor/wait.rs"),
    ),
    ("executor.rs", include_str!("../../../phy/src/executor.rs")),
    ("hardware.rs", include_str!("../../../phy/src/hardware.rs")),
    (
        "ieee802154_client.rs",
        include_str!("../../../phy/src/ieee802154_client.rs"),
    ),
    ("lib.rs", include_str!("../../../phy/src/lib.rs")),
    (
        "lifecycle.rs",
        include_str!("../../../phy/src/lifecycle.rs"),
    ),
    (
        "rx/dc_offset.rs",
        include_str!("../../../phy/src/rx/dc_offset.rs"),
    ),
    ("rx/gain.rs", include_str!("../../../phy/src/rx/gain.rs")),
    (
        "rx/gain_calibration/quality.rs",
        include_str!("../../../phy/src/rx/gain_calibration/quality.rs"),
    ),
    (
        "rx/gain_calibration.rs",
        include_str!("../../../phy/src/rx/gain_calibration.rs"),
    ),
    ("rx/iq.rs", include_str!("../../../phy/src/rx/iq.rs")),
    (
        "rx/program.rs",
        include_str!("../../../phy/src/rx/program.rs"),
    ),
    (
        "rx/saturation.rs",
        include_str!("../../../phy/src/rx/saturation.rs"),
    ),
    (
        "rx/signal_power.rs",
        include_str!("../../../phy/src/rx/signal_power.rs"),
    ),
    ("rx.rs", include_str!("../../../phy/src/rx.rs")),
    (
        "size_limits.rs",
        include_str!("../../../phy/src/size_limits.rs"),
    ),
    (
        "state/client.rs",
        include_str!("../../../phy/src/state/client.rs"),
    ),
    ("state.rs", include_str!("../../../phy/src/state.rs")),
    (
        "target_executor.rs",
        include_str!("../../../phy/src/target_executor.rs"),
    ),
    (
        "target_port/calibration.rs",
        include_str!("../../../phy/src/target_port/calibration.rs"),
    ),
    (
        "target_port/concurrent.rs",
        include_str!("../../../phy/src/target_port/concurrent.rs"),
    ),
    (
        "target_port/domain.rs",
        include_str!("../../../phy/src/target_port/domain.rs"),
    ),
    (
        "target_port/radio_lifecycle.rs",
        include_str!("../../../phy/src/target_port/radio_lifecycle.rs"),
    ),
    (
        "target_port/rfpll.rs",
        include_str!("../../../phy/src/target_port/rfpll.rs"),
    ),
    (
        "target_port/step.rs",
        include_str!("../../../phy/src/target_port/step.rs"),
    ),
    (
        "target_port/temperature.rs",
        include_str!("../../../phy/src/target_port/temperature.rs"),
    ),
    (
        "target_port.rs",
        include_str!("../../../phy/src/target_port.rs"),
    ),
    (
        "tracking/calibration/decision.rs",
        include_str!("../../../phy/src/tracking/calibration/decision.rs"),
    ),
    (
        "tracking/calibration.rs",
        include_str!("../../../phy/src/tracking/calibration.rs"),
    ),
    (
        "tracking/fail_stop.rs",
        include_str!("../../../phy/src/tracking/fail_stop.rs"),
    ),
    (
        "tracking/i2c.rs",
        include_str!("../../../phy/src/tracking/i2c.rs"),
    ),
    (
        "tracking/inspection.rs",
        include_str!("../../../phy/src/tracking/inspection.rs"),
    ),
    (
        "tracking/observation/calibration.rs",
        include_str!("../../../phy/src/tracking/observation/calibration.rs"),
    ),
    (
        "tracking/observation.rs",
        include_str!("../../../phy/src/tracking/observation.rs"),
    ),
    (
        "tracking/parameters.rs",
        include_str!("../../../phy/src/tracking/parameters.rs"),
    ),
    (
        "tracking/power.rs",
        include_str!("../../../phy/src/tracking/power.rs"),
    ),
    (
        "tracking/rfpll/search.rs",
        include_str!("../../../phy/src/tracking/rfpll/search.rs"),
    ),
    (
        "tracking/rfpll/thermal.rs",
        include_str!("../../../phy/src/tracking/rfpll/thermal.rs"),
    ),
    (
        "tracking/rfpll.rs",
        include_str!("../../../phy/src/tracking/rfpll.rs"),
    ),
    (
        "tracking/schedule.rs",
        include_str!("../../../phy/src/tracking/schedule.rs"),
    ),
    (
        "tracking/temperature.rs",
        include_str!("../../../phy/src/tracking/temperature.rs"),
    ),
    ("tracking.rs", include_str!("../../../phy/src/tracking.rs")),
    (
        "tx/calibration.rs",
        include_str!("../../../phy/src/tx/calibration.rs"),
    ),
    (
        "tx/dc_offset.rs",
        include_str!("../../../phy/src/tx/dc_offset.rs"),
    ),
    (
        "tx/dc_power_detector.rs",
        include_str!("../../../phy/src/tx/dc_power_detector.rs"),
    ),
    ("tx/gain.rs", include_str!("../../../phy/src/tx/gain.rs")),
    ("tx/iq.rs", include_str!("../../../phy/src/tx/iq.rs")),
    ("tx/power.rs", include_str!("../../../phy/src/tx/power.rs")),
    (
        "tx/power_detector.rs",
        include_str!("../../../phy/src/tx/power_detector.rs"),
    ),
    ("tx/sar.rs", include_str!("../../../phy/src/tx/sar.rs")),
    ("tx.rs", include_str!("../../../phy/src/tx.rs")),
    (
        "validation.rs",
        include_str!("../../../phy/src/validation.rs"),
    ),
    (
        "wifi_client.rs",
        include_str!("../../../phy/src/wifi_client.rs"),
    ),
];

#[test]
fn calibration_or_replay_changes_require_a_reviewed_revision() {
    let mut hasher = Sha256::new();
    for (path, source) in INPUTS {
        hasher.update(path.as_bytes());
        hasher.update([0]);
        hasher.update(source.as_bytes());
        hasher.update([0]);
    }
    let actual = std::format!("{:x}", hasher.finalize());
    let revision = (CALIBRATION_ALGORITHM_REVISION, CALIBRATION_PROFILE_ID);
    // Keep previously reviewed pairs immutable. A changed algorithm/profile
    // takes a new pair and a new digest arm after reviewing all input paths.
    let expected = match revision {
        (1, 1) => "e28a4e4c74b0d8ed26a4a390b634ff46175370d20955c29acb109c347e30b16c",
        (2, 1) => "4e9a61acd06a95e0a9c13ee09dfcab05cb6d0799f2eac7a5abcf9f0b0ea8ea68",
        (3, 1) => "da5fb1d69371d0676bfb8cdc55fd0c6df2e2081e9a72fafa17ba3590a0836615",
        _ => panic!(
            "unreviewed calibration revision/profile {revision:?}; digest {actual}; review INPUTS and add a new pair in revision_guard.rs"
        ),
    };
    assert_eq!(
        actual, expected,
        "calibration/replay inputs changed: bump CALIBRATION_ALGORITHM_REVISION or CALIBRATION_PROFILE_ID, review INPUTS, and add the printed digest under the new pair; rerun cargo test -p oer-esp32s31-knowledge calibration_or_replay_changes_require_a_reviewed_revision -- --nocapture"
    );
}
