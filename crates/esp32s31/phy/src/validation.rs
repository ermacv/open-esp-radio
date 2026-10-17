//! ABI-only bridges for compiled production comparison.
//!
//! This module is absent from ordinary builds. It performs argument/result
//! conversion and semantic fixture construction only. Hardware operations and
//! state publication use the same transitions and executors as production.

#![cfg(feature = "validation-probes")]

/// Execute the shipping wake-path I2C reset, including its finite completion bound.
/// The caller retains the shared PHY owner; no hardware state is fabricated here.
#[cfg(target_arch = "riscv32")]
pub fn reset_i2c_master(
    platform: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
) -> Result<(), crate::PhyTargetPortError> {
    crate::target_port::reset_wake_i2c_master(platform)
}

/// Execute the accredited-domain production behavior of
/// `phy_get_i2c_hostid_new` and project its typed host to the vendor ABI.
#[cfg(target_arch = "riscv32")]
pub fn configure_and_select_phy_i2c_host(
    platform: &mut impl oer_esp32s31_hal::owner::SharedPhyAccess,
    block: u8,
) -> u32 {
    let Some(block) = crate::analog::i2c::PhyI2cBlock::from_vendor_abi(block) else {
        return u32::MAX;
    };

    match oer_esp32s31_hal::phy::i2c::configure_and_select_host(platform, block) {
        oer_esp32s31_hal::phy::i2c::PhyI2cHost::Host0 => 0,
        oer_esp32s31_hal::phy::i2c::PhyI2cHost::Host1 => 1,
    }
}

/// Execute the production channel selection over an unregistered fixture
/// state for compiled vendor comparison. Ordinary builds reach channel
/// selection only through a registered owner.
#[cfg(target_arch = "riscv32")]
pub async fn select_channel<D: crate::PhyShortDelay, P, O: crate::PhyTargetObserver>(
    timer: &impl oer_time::Timer,
    state: &mut crate::PhyState,
    channel_or_frequency: u16,
    cbw: u8,
    channel: &mut oer_esp32s31_hal::ieee80211::channel::RadioChannelHal<'_, P>,
    observer: &mut O,
) -> Result<(), crate::PhyTargetPortError> {
    crate::target_port::select_phy_channel_with_hal::<D, _, _>(
        timer,
        state,
        channel_or_frequency,
        cbw,
        channel,
        observer,
    )
    .await
}

/// Construct ordinary semantic inputs without a registration or admission proof.
pub fn calibration_tracking_state(
    parameters: crate::tracking::calibration::PhyCalibrationTrackingParameters,
) -> crate::PhyState {
    crate::PhyState::calibration_tracking_fixture(parameters)
}

/// Select the real combined child directly for an isolated compiled probe.
/// This does not manufacture a registered owner or a successful completion.
pub fn calibration_tracking(
    state: &mut crate::PhyState,
    clients: crate::tracking::parameters::PhyParamTrackRequest,
) -> crate::tracking::parameters::PhyParamTrackingCalibrationTransition<'_> {
    use crate::tracking::parameters::{
        PhyParamTrackingAction, PhyParamTrackingCalibrationTransition, PhyParamTrackingPolicy,
        PhyTrackingDiagnostics,
    };
    PhyParamTrackingCalibrationTransition::lower(
        PhyParamTrackingAction::CalibrationTrack {
            clients,
            diagnostics: PhyTrackingDiagnostics::Disabled,
        },
        PhyParamTrackingPolicy::for_registered_state(state),
        state,
    )
    .expect("calibration action selects the combined child")
}

/// Parent policy choices a vendor comparison seeds as `phy_param` guards.
/// Every other registered policy field is unchanged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParameterTrackingChoices {
    /// RFPLL capacitance tracking (`phy_param[0xa]`).
    pub rfpll: bool,
    /// Either vendor tracking guard (`phy_param[0x17]` or `[0x195]`).
    pub inhibited: bool,
    /// Calibration tracking, disabled by a nonzero `phy_param[0x192]`.
    pub calibration: bool,
}

/// Select the complete production parent under `choices`; the returned model
/// owns no physical capability.
pub fn parameter_tracking(
    state: &crate::PhyState,
    clients: crate::tracking::parameters::PhyParamTrackRequest,
    choices: ParameterTrackingChoices,
) -> crate::state::client::PhyPendingTracking {
    let mut policy =
        crate::tracking::parameters::PhyParamTrackingPolicy::for_registered_state(state);
    policy.rfpll_cap_tracking_enabled = choices.rfpll;
    policy.tracking_inhibited = choices.inhibited;
    policy.calibration_tracking_enabled = choices.calibration;
    crate::state::client::PhyPendingTracking::for_validation(clients, policy)
}

/// Seed ordinary state for comparison of the whole parameter parent.
/// Gain adjustment is an independent retained input, not a derived attenuation.
pub fn parameter_tracking_state(
    parameters: crate::tracking::calibration::PhyCalibrationTrackingParameters,
    gain_adjustment: i8,
    relaxed_threshold: bool,
) -> crate::PhyState {
    crate::PhyState::parameter_tracking_fixture(parameters, gain_adjustment, relaxed_threshold)
}

/// Seed the retained shared and Wi-Fi RX-gain table last indices that the
/// tracking children consume, as the vendor comparison seeds its own state.
pub fn seed_rx_table_last_indices(state: &mut crate::PhyState, shared: u8, wifi: u8) {
    state.seed_rx_table_last_indices(shared, wifi);
}

/// Read the semantic RFPLL reference without exposing a mutable ABI image.
pub fn rfpll_reference_temperature(state: &crate::PhyState) -> i16 {
    state.rfpll_tracking_request(None).reference_temperature
}

/// The calibration state `state` retains, as persistence would capture it,
/// under an all-zero identity, so a comparison can read committed fields.
pub fn calibration_snapshot(state: &crate::PhyState) -> crate::state::PhyCalibrationSnapshot {
    state.calibration_snapshot(crate::calibration::registration::PhyCalibrationIdentity {
        rf_cal_version: 0,
        base_mac_address: [0; 6],
        mac_extension: 0,
    })
}

/// A state retaining exactly `snapshot`, so a snapshot captured on hardware
/// projects through the same state accessors the comparison probes read.
/// Rejects snapshots a cold replay would reject.
pub fn retained_calibration_state(
    snapshot: crate::state::PhyCalibrationSnapshot,
) -> Result<crate::PhyState, crate::state::PhyCalibrationCacheError> {
    let cache = crate::state::PhyCalibrationCache::from_snapshot(snapshot)
        .ok_or(crate::state::PhyCalibrationCacheError::SchemaMismatch)?;
    crate::PhyState::retained_calibration(crate::state::PhyConfig::production(), &cache)
}

#[cfg(test)]
mod tests {
    #[test]
    fn calibration_snapshot_reads_committed_state() {
        let mut state = crate::PhyState::default();
        let codes = [1, 2, 3, 4, 5, 6, 7, 8];
        state.apply_dcode_outcome(crate::analog::dcode::PhyDcodeOutcome { codes });
        assert_eq!(super::calibration_snapshot(&state).common.dcode, codes);
    }

    #[test]
    fn retained_calibration_state_keeps_every_snapshot_field() {
        use crate::calibration::baseband::{
            PHY_SHARED_RX_GAIN_LAST_INDEX, PHY_WIFI_RX_GAIN_LAST_INDEX,
        };
        let mut snapshot = super::calibration_snapshot(&crate::PhyState::default());
        let (common, wifi, bluetooth) = (
            &mut snapshot.common,
            &mut snapshot.wifi,
            &mut snapshot.bluetooth,
        );
        common.temperature = 31;
        common.rfpll_reference_temperature = 29;
        common.rxcal_reference_temperature = 28;
        common.txcal_reference_temperature = 27;
        common.power_reference_temperature = 26;
        common.rc_calibrated = true;
        common.dcode = [1, 2, 3, 4, 5, 6, 7, 8];
        common.xtal_duty = [9, 10, 11];
        wifi.baseband_calibrated = true;
        wifi.pwdet_calibrated = true;
        wifi.tx_power_calibrated = true;
        wifi.tx_iq_calibrated = true;
        wifi.rx_gain_dc_calibrated = true;
        wifi.rx_gain_tables_initialized = true;
        wifi.wifi_rx_table_last_index = PHY_WIFI_RX_GAIN_LAST_INDEX;
        wifi.shared_rx_table_last_index = PHY_SHARED_RX_GAIN_LAST_INDEX;
        wifi.tx_dco[2] = [12, 13, 14, 15];
        wifi.shared_index_dc[10] = [16, 17];
        wifi.calibration_channel = 13;
        bluetooth.tx_dc_calibrated = true;
        bluetooth.tx_power_calibrated = true;
        bluetooth.tx_dco[1] = [18, 19, 20, 21];
        common.power_reference_temperature = 26;
        let state = super::retained_calibration_state(snapshot).unwrap();
        assert_eq!(super::calibration_snapshot(&state), snapshot);
        // Registration restarts TX-power tracking from the power reference.
        assert_eq!(
            state
                .tx_power_tracking_parameters(true)
                .previous_tracking_temperature,
            26
        );
    }

    #[test]
    fn retained_calibration_state_rejects_incomplete_calibration() {
        let snapshot = super::calibration_snapshot(&crate::PhyState::default());
        assert_eq!(
            super::retained_calibration_state(snapshot).err(),
            Some(crate::state::PhyCalibrationCacheError::IncompleteCalibration)
        );
    }

    #[test]
    fn seeded_rx_table_last_indices_are_retained() {
        let mut state = crate::PhyState::default();
        super::seed_rx_table_last_indices(&mut state, 75, 71);
        let wifi = super::calibration_snapshot(&state).wifi;
        assert_eq!(
            (
                wifi.shared_rx_table_last_index,
                wifi.wifi_rx_table_last_index
            ),
            (75, 71)
        );
    }
}
