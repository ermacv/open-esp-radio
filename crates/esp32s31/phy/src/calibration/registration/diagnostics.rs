//! Value-only results retained after RF calibration releases its temporary state.

use crate::analog::{i2c::PhyRfInitPrefixOutcome, rfpll::RfpllFrequencyOutcome};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RfpllCalibrationPoint {
    pub lock_observed: bool,
    pub initial_cap: u16,
    pub final_cap: u16,
    pub accepted_cap_samples: u8,
}

impl From<RfpllFrequencyOutcome> for RfpllCalibrationPoint {
    fn from(outcome: RfpllFrequencyOutcome) -> Self {
        Self {
            lock_observed: outcome.lock_observed,
            initial_cap: outcome.initial_cap,
            final_cap: outcome.final_cap,
            accepted_cap_samples: outcome.accepted_cap_samples,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrequencyCalibrationDiagnostics {
    pub nominal: RfpllCalibrationPoint,
    pub low: RfpllCalibrationPoint,
    pub high: RfpllCalibrationPoint,
    pub table_entries: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RfCalibrationDiagnostics {
    pub charge_pump_locked: bool,
    pub frequency: Option<FrequencyCalibrationDiagnostics>,
}

impl RfCalibrationDiagnostics {
    pub(super) fn from_outcome(outcome: PhyRfInitPrefixOutcome) -> Option<Self> {
        let PhyRfInitPrefixOutcome::ChannelFrequencyInitialized {
            rfpll_lock_observed,
            channel_frequency,
            ..
        } = outcome
        else {
            return None;
        };
        Some(Self {
            charge_pump_locked: rfpll_lock_observed,
            frequency: channel_frequency.calibration.map(|calibration| {
                FrequencyCalibrationDiagnostics {
                    nominal: calibration.nominal.into(),
                    low: calibration.low.into(),
                    high: calibration.high.into(),
                    table_entries: calibration.table.entries_written,
                }
            }),
        })
    }
}
