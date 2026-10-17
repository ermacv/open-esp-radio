//! Non-consuming inspection of the registered tracking policy.
//!
//! A schedule deadline asks for a tracking evaluation, not every heavy branch.
//! Conditions use retained temperature with explicit acquisition provenance. They do
//! not certify freshness, provide RF access or select independent executable
//! jobs. Earlier children can change shared state; the executor reevaluates
//! conditions at their actual action boundary using the same predicates.

use super::{
    calibration::decision::Decision,
    i2c::PhyWifiI2cTrackingParameters,
    parameters::{PhyCalibrationTrackClass as Class, PhyParamTrackingPolicy},
    power::{PhyTxPowerTrackingDecision, PhyTxPowerTrackingRequest, decide_tx_power_tracking},
    schedule::Schedule,
};
use crate::{
    PhyState, RegisteredPhyState,
    state::client::{PhyClientSnapshot, PhyTrackTimeError, RadioClient},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClassInspection {
    /// Includes recomputation versus actual gain-publication demand.
    pub power: PhyTxPowerTrackingDecision,
    /// None when calibration tracking is disabled by the registered policy.
    pub calibration: Option<Decision>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Inspection {
    /// Unmodified observation of the source scheduler's evaluation deadline.
    pub schedule: Schedule,
    pub temperature: super::temperature::Observation,
    pub inhibited: bool,
    /// None when inactive, inhibited or disabled. Present does not mean due.
    pub rfpll: Option<super::rfpll::thermal::Request>,
    /// None when Wi-Fi is inactive or tracking is inhibited.
    pub wifi: Option<ClassInspection>,
    pub wifi_i2c: Option<PhyWifiI2cTrackingParameters>,
    /// BT and 154 share this calibration class, not protocol ownership.
    pub bluetooth_ieee802154: Option<ClassInspection>,
}

impl Inspection {
    pub(crate) fn registered(
        registered: &RegisteredPhyState,
        clients: PhyClientSnapshot,
        now_micros: u64,
    ) -> Result<Self, PhyTrackTimeError> {
        Self::inspect(
            registered.state(),
            registered.tracking_policy(),
            clients,
            now_micros,
        )
    }

    fn inspect(
        state: &PhyState,
        policy: PhyParamTrackingPolicy,
        clients: PhyClientSnapshot,
        now_micros: u64,
    ) -> Result<Self, PhyTrackTimeError> {
        let schedule = clients.tracking_schedule_at(now_micros)?;
        let active = !policy.tracking_inhibited && !clients.is_empty();
        let wifi = active && clients.contains(RadioClient::Wifi);
        let shared = active
            && (clients.contains(RadioClient::Bluetooth)
                || clients.contains(RadioClient::Ieee802154));
        let calibration =
            state.calibration_tracking_parameters(policy.calibration_tracking_threshold);
        let power = state.tx_power_tracking_parameters(policy.relaxed_power_tracking_threshold);
        let class = |class, enabled| ClassInspection {
            power: decide_tx_power_tracking(
                PhyTxPowerTrackingRequest {
                    class,
                    enabled,
                    wifi_channel: calibration.current_channel,
                },
                power,
            ),
            calibration: policy
                .calibration_tracking_enabled
                .then(|| calibration.decision()),
        };
        Ok(Self {
            schedule,
            temperature: state.temperature_observation(),
            inhibited: policy.tracking_inhibited,
            rfpll: (active && policy.rfpll_cap_tracking_enabled)
                .then(|| state.rfpll_tracking_request(policy.rfpll_cap_tracking_threshold)),
            wifi: wifi.then(|| class(Class::Wifi, true)),
            wifi_i2c: wifi.then(|| state.wifi_i2c_tracking_parameters()),
            bluetooth_ieee802154: shared.then(|| {
                class(
                    Class::BluetoothIeee802154,
                    policy.bluetooth_ieee802154_power_tracking_enabled,
                )
            }),
        })
    }
}

#[cfg(test)]
mod tests;
