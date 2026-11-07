//! Value-only results retained after RF calibration releases its temporary state.

use crate::{
    analog::{i2c::PhyRfInitPrefixOutcome, rfpll::RfpllFrequencyOutcome},
    calibration::{baseband::PhyBbInitCompletion, bluetooth::PhyBluetoothTxGainInitCompletion},
    tx::{dc_offset::PhyTxDcCompletion, power_detector::PhyPwdetCompletion},
};

use super::PhyRegisterCompletion;

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

/// The longest runs of not-ready reads the registration's bounded readiness
/// waits observed before their flag became ready. Each wait gives up after
/// [`crate::HARDWARE_EDGE_LIMIT`] such reads.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReadyWaitDiagnostics {
    /// TX DC comparator readiness, in Wi-Fi and Bluetooth TX DC search.
    pub tx_dc_max_not_ready_samples: u16,
    /// Power-detector SAR readiness.
    pub pwdet_sar_max_not_ready_samples: u16,
}

impl ReadyWaitDiagnostics {
    pub(super) fn observe(&mut self, completion: &PhyRegisterCompletion) {
        let PhyRegisterCompletion::Baseband(completion) = completion else {
            return;
        };
        match completion {
            PhyBbInitCompletion::TxDc(PhyTxDcCompletion::ReadyObserved {
                not_ready_samples,
                ..
            })
            | PhyBbInitCompletion::BluetoothTxGain(PhyBluetoothTxGainInitCompletion::TxDc(
                PhyTxDcCompletion::ReadyObserved {
                    not_ready_samples, ..
                },
            )) => {
                self.tx_dc_max_not_ready_samples =
                    self.tx_dc_max_not_ready_samples.max(*not_ready_samples);
            }
            PhyBbInitCompletion::Pwdet(PhyPwdetCompletion::SarReadyObserved {
                not_ready_samples,
                ..
            }) => {
                self.pwdet_sar_max_not_ready_samples =
                    self.pwdet_sar_max_not_ready_samples.max(*not_ready_samples);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tx_dc(not_ready_samples: u16) -> PhyTxDcCompletion {
        PhyTxDcCompletion::ReadyObserved {
            gain_index: 0,
            iteration: 0,
            not_ready_samples,
        }
    }

    #[test]
    fn ready_waits_keep_the_longest_not_ready_run_of_each_wait() {
        let mut waits = ReadyWaitDiagnostics::default();
        for completion in [
            PhyBbInitCompletion::TxDc(tx_dc(7)),
            PhyBbInitCompletion::BluetoothTxGain(PhyBluetoothTxGainInitCompletion::TxDc(tx_dc(12))),
            PhyBbInitCompletion::TxDc(tx_dc(3)),
            PhyBbInitCompletion::Pwdet(PhyPwdetCompletion::SarReadyObserved {
                measurement_index: 0,
                sample_index: 0,
                not_ready_samples: 5,
            }),
            PhyBbInitCompletion::TxDc(PhyTxDcCompletion::ReadyDeadlineElapsed {
                gain_index: 0,
                iteration: 0,
            }),
        ] {
            waits.observe(&PhyRegisterCompletion::Baseband(completion));
        }
        assert_eq!(
            waits,
            ReadyWaitDiagnostics {
                tx_dc_max_not_ready_samples: 12,
                pwdet_sar_max_not_ready_samples: 5,
            }
        );
    }
}
