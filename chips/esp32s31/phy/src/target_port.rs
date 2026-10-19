//! Complete ESP32-S31 target port for the recovered PHY transitions.
//!
//! This module owns the target-side composition of the individual PHY state
//! machines. Runtime RX/TX calibration executes synchronously after physical
//! admission; asynchronous time remains available to longer registration,
//! tracking and lifecycle edges. Applications inject timing capabilities and
//! an optional observer; they must not reconstruct the recovered hardware
//! contract.

use crate::executor::wait::{Kind, PhyShortDelay, delay, delay_observed};
use oer_time::Timer;

use core::marker::PhantomData;

pub mod calibration;
pub mod rfpll;

use crate::{
    HARDWARE_EDGE_LIMIT, PhyParamTrackingRunError, PhyRegisterRunError,
    analog::{
        dcode::{PhyDcodeCompletion, PhyDcodeExternalBinding},
        i2c::{PhyRfInitPrefixAction, PhyRfInitPrefixCompletion},
        pbus::PhyForceTxRxExternalBinding,
        rfpll::{RfpllFrequencyAction, RfpllFrequencyCompletion, RfpllFrequencyExternalBinding},
        temperature::{
            PhyTemperatureAction, PhyTemperatureCompletion, PhyTemperatureExternalBinding,
            PhyTemperatureTransition,
        },
    },
    calibration::{
        baseband::{PhyBbExternalBinding, PhyBbInitCompletion},
        bluetooth::{
            PhyBluetoothTxGainInitCompletion, PhyBluetoothTxGainInitExternalBinding,
            PhyBluetoothTxPowerCompletion, PhyBluetoothTxPowerExternalBinding,
        },
        cold::{
            PhyColdExternalBinding, PhyColdI2cAction, PhyColdI2cError, PhyColdI2cObservation,
            PhyColdObservationRequest, PhyColdPbusObservation,
        },
        estimator::{PhyDcIqCompletion, PhyDcIqExternalBinding},
        registration::{
            PhyCalibrationIdentity, PhyRegisterCompletion, PhyRegisterExternalBinding,
            PhyRegisterOutcome, PhyRegisterTransition,
        },
    },
    channel::{
        PhyChipChannelAction, PhyChipChannelCompletion, PhyChipChannelExternalBinding,
        PhyChipChannelFailure, PhyChipChannelOutcome, PhyChipChannelRequest,
        PhyChipChannelTransition, PhyWifiTxGainImage, PhyWifiTxGainRequest,
    },
    executor::{
        PhyCalibrationTrackingPort, PhyParamTrackingPort, PhyRegisterPort,
        run_phy_calibration_tracking, run_phy_param_tracking, run_phy_register,
    },
    rx::{
        dc_offset::{
            PhyRxDcMinimumCompletion, PhyRxDcMinimumExternalBinding, PhyRxDcoCompletion,
            PhyRxDcoExternalBinding,
        },
        gain::{
            PhyRxGainInitCompletion, PhyRxGainInitExternalBinding, PhyRxGainPublishCompletion,
            PhyRxGainPublishExternalBinding,
        },
        gain_calibration::{
            PhyRxDcCalibrationCompletion, PhyRxDcCalibrationExternalBinding, PhyRxGainDcCompletion,
            PhyRxGainDcExternalBinding,
        },
        iq::{
            PhyRxIqCoverCompletion, PhyRxIqCoverExternalBinding, PhyRxIqDataCompletion,
            PhyRxIqDataExternalBinding, PhyRxIqEstimatorCompletion,
            PhyRxIqEstimatorExternalBinding, PhyRxIqGainCompletion, PhyRxIqGainExternalBinding,
            PhyRxIqInitCompletion, PhyRxIqInitExternalBinding, PhyRxIqRfCalibrationCompletion,
            PhyRxIqRfCalibrationExternalBinding,
        },
        saturation::{PhyRxSaturationCompletion, PhyRxSaturationExternalBinding},
    },
    state::{PhyCalibrationCache, PhyState, client::PhyPendingTracking},
    target_executor::{
        PhyTargetPortError, complete_bluetooth_i2c, complete_bluetooth_pbus, complete_channel_i2c,
        complete_dcode_i2c, complete_final_i2c, complete_i2c_configuration, complete_masked_i2c,
        complete_rfpll_i2c, complete_rx_dc_calibration_pbus, complete_rx_dco_pbus,
        complete_rx_gain_dc_pbus, complete_rx_gain_i2c, complete_rx_gain_publish_pbus,
        complete_rx_saturation_pbus, complete_rxiq_adjusted_tx_i2c, complete_rxiq_gain_i2c,
        complete_rxiq_gain_pbus, complete_rxiq_init_i2c, complete_rxiq_init_pbus,
        complete_temperature_i2c, complete_tx_calibration_environment_pbus, complete_tx_dc_pbus,
        complete_tx_dc_pwdet_pbus, complete_tx_dc_pwdet_search_pbus, complete_tx_power_i2c,
        complete_txiq_init_i2c, complete_txiq_pbus, write_i2c_direct,
    },
    tracking::{
        calibration::{
            PhyCalibrationForceTxRxTransition, PhyCalibrationTrackingAction,
            PhyCalibrationTrackingCompletion, PhyCalibrationTrackingExternalBinding,
            PhyCalibrationTxDcPwdetTransition,
        },
        i2c::PhyWifiI2cTrackingCompletion,
        parameters::{
            PhyParamTrackingAction, PhyParamTrackingCalibrationTransition,
            PhyParamTrackingCompletion, PhyParamTrackingOutcome, PhyParamTrackingRfpllTransition,
            PhyParamTrackingTemperatureTransition, PhyParamTrackingTxPowerTransition,
            PhyParamTrackingWifiI2cTransition,
        },
    },
    tx::{
        calibration::{
            PhyPowerAttenuationCompletion, PhyPowerAttenuationExternalBinding,
            PhyToneSarCompletion, PhyToneSarExternalBinding, PhyTxCalibrationEnvironmentCompletion,
            PhyTxCalibrationEnvironmentExternalBinding, PhyTxCapCompletion,
            PhyTxCapExternalBinding, PhyTxCapSearchCompletion, PhyTxCapSearchExternalBinding,
        },
        dc_offset::{PhyTxDcAction, PhyTxDcCompletion, PhyTxDcExternalBinding},
        dc_power_detector::{
            PhyTxDcPwdetCompletion, PhyTxDcPwdetExternalBinding, PhyTxDcPwdetSearchCompletion,
            PhyTxDcPwdetSearchExternalBinding,
        },
        iq::{
            PhyTxIqCalibrationCompletion, PhyTxIqCalibrationExternalBinding,
            PhyTxIqCoverCompletion, PhyTxIqCoverExternalBinding, PhyTxIqInitCompletion,
            PhyTxIqInitExternalBinding, PhyTxIqLinearPowerCompletion,
            PhyTxIqLinearPowerExternalBinding, PhyTxIqLoopbackCompletion,
            PhyTxIqLoopbackExternalBinding, PhyTxIqMisPowerCompletion,
            PhyTxIqMisPowerExternalBinding,
        },
        power::{
            PhyPowerControlPointCompletion, PhyPowerControlPointExternalBinding,
            PhyTxPowerCompletion, PhyTxPowerExternalBinding,
        },
        power_detector::{PhyPwdetCompletion, PhyPwdetExternalBinding, PhyPwdetPbusObservation},
    },
};

mod concurrent;
pub use concurrent::{
    ConcurrentPhyRegisterFailure, ConcurrentPhyRegistration, ConcurrentPhyTrackingError,
    ConcurrentRfError, ConcurrentTrackingTick, ConcurrentWifiChannelError, close_concurrent_rf,
    maintain_concurrent_phy, register_concurrent_phy, select_concurrent_wifi_channel,
    switch_concurrent_wifi_channel, track_concurrent_phy, wake_concurrent_rf,
};
mod domain;
pub use domain::{PhyDomainRegisterFailure, PhyDomainRegistered, PhyRegisterConfig};

mod radio_lifecycle;
pub(crate) use radio_lifecycle::PhyRfCloseTemperatureFailure;

use oer_esp32s31_hal::owner::{PhyInitializationAccess, SharedPhyAccess, SharedPhyContext};

/// The maintaining owner's PHY grant-protect request.
///
/// Periodic tracking brackets its RFPLL correction and each calibration
/// branch with this request, where the pinned archive calls
/// `phy_acquire_grant_protect` and `phy_release_grant_protect`. The request
/// raises the PHY's coexistence priority; it is never an ownership proof.
pub trait PhyGrantProtectPort {
    /// Program (`true`) or withdraw (`false`) the request.
    ///
    /// # Errors
    ///
    /// The request is not in the opposite state; nothing is written.
    fn set_grant_protect(&mut self, enabled: bool) -> Result<(), PhyTargetPortError>;
}

impl PhyGrantProtectPort for oer_esp32s31_hal::coex::PhyGrantProtect<'_> {
    fn set_grant_protect(&mut self, enabled: bool) -> Result<(), PhyTargetPortError> {
        if enabled {
            self.acquire()
        } else {
            self.release()
        }
        .map_err(|_| PhyTargetPortError::HardwareInvariant)
    }
}

impl<G: PhyGrantProtectPort + ?Sized> PhyGrantProtectPort for &mut G {
    fn set_grant_protect(&mut self, enabled: bool) -> Result<(), PhyTargetPortError> {
        (**self).set_grant_protect(enabled)
    }
}

/// The weak `phy_acquire_grant_protect` and `phy_release_grant_protect` of the
/// PHY archive linked without the coexistence archive: both return without
/// effect. Vendor comparison of PHY-only leaves uses this port.
#[cfg(any(test, oer_probe))]
pub struct WeakPhyGrantProtect;

#[cfg(any(test, oer_probe))]
impl PhyGrantProtectPort for WeakPhyGrantProtect {
    fn set_grant_protect(&mut self, _enabled: bool) -> Result<(), PhyTargetPortError> {
        Ok(())
    }
}

const CHANNEL_READY_SAMPLE_LIMIT: u32 = 10_000;
const RF_OPERATION_LIMIT: u32 = 100_000;
const MAC_CHANNEL_SETTLE_US: u64 = 20;
const MAC_CHANNEL_IDLE_SETTLE_US: u64 = 5;

/// A semantic checkpoint exposed to target diagnostics without exporting raw
/// MMIO or application logging into the driver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyRfBoundary {
    BeforeRfInit,
    AfterPbusClear,
    BeforeI2cMasterRegisterInit,
    BeforePowerDetectorRegisterInit,
    BeforeFrontEndRegisterInit,
    BeforeTemperatureSensorReadInit,
    BeforeTxPowerControlBackgroundInit,
    BeforeChannelFrequencyInit,
}

/// Optional, synchronous target observations which cannot affect PHY state.
///
/// Production integrations normally use [`NoopPhyTargetObserver`]. Diagnostic
/// code can implement this trait to compare a completed Rust result with ROM or to
/// capture diagnostic MMIO without placing either dependency in this crate.
pub trait PhyTargetObserver {
    const OBSERVE_DELAYS: bool = false;
    /// Include sensor age at RFPLL entry; disabled observers add no clock read.
    const OBSERVE_RFPLL_AGE: bool = false;
    fn tx_wait(
        &mut self,
        _scope: crate::executor::wait::tx::Scope,
        _kind: crate::executor::wait::Kind,
        _event: crate::executor::wait::Event,
    ) {
    }
    fn tx_sar_ready(&mut self, _ready: bool) {}
    /// Emitted only after the thermal child restores control and commits state.
    fn rfpll_completed(&mut self, _observation: crate::tracking::rfpll::Observation) {}
    /// Emitted only after the sensor child commits its observed temperature.
    fn temperature_completed(
        &mut self,
        _outcome: crate::analog::temperature::PhyTemperatureOutcome,
    ) {
    }

    fn dcode_wait(
        &mut self,
        _scope: crate::executor::wait::Scope,
        _kind: crate::executor::wait::Kind,
        _event: crate::executor::wait::Event,
    ) {
    }

    fn dcode_pll_lock(&mut self, _locked: bool) {}

    /// Emitted once after a terminally successful RX-gain child. The child
    /// performs no diagnostic clock reads while accumulating these counts.
    fn rx_gain_execution(&mut self, _execution: crate::tracking::observation::RxGainExecution) {}

    fn tracking_operation(
        &mut self,
        _operation: crate::tracking::observation::Operation,
        _event: crate::tracking::observation::Event,
    ) {
    }
    fn operation_started(&mut self) {}
    fn operation_completed(&mut self) {}
    fn channel_frequency_ready_timed_out(&mut self, _samples: u32) {}
    fn channel_tx_gain(&mut self, _request: PhyWifiTxGainRequest, _image: PhyWifiTxGainImage) {}
    fn channel_completed(&mut self, _outcome: PhyChipChannelOutcome, _operations: u32) {}
    fn channel_failed(&mut self, _failure: PhyChipChannelFailure, _operations: u32) {}
    fn mac_channel_restarted(&mut self, _channel_or_frequency: u16, _cbw: u8, _link: u8) {}
    fn tx_dc_entry(&mut self) {}
    fn tx_dc_comparator(&mut self, _gain_index: u8, _iteration: u8, _comparator_high: [bool; 2]) {}
    fn power_detector_sample(
        &mut self,
        _measurement_index: u8,
        _sample_index: u8,
        _sample_value: u16,
    ) {
    }
    fn rf_boundary(&mut self, _boundary: PhyRfBoundary) {}
}

/// Observer used by production integrations which need no diagnostic side
/// channel.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopPhyTargetObserver;

impl PhyTargetObserver for NoopPhyTargetObserver {}

/// Operation counts produced by one PHY registration run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PhyTargetPortCounters {
    pub mmio: u16,
    pub delays: u16,
    pub reset_samples: u16,
    pub rf_operations: u32,
    pub baseband_operations: u32,
}

/// Failure from the concrete target registration runner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetPhyRegisterError {
    Run(PhyRegisterRunError<PhyTargetPortError>),
    MissingCompletedModelOwner,
}

/// Private witness accepted by the sole [`crate::RegisteredPhyState`] constructor.
///
/// The field is private to this module, so no sibling module can mint it even
/// though the proof constructor can name the type through crate visibility.
pub(crate) struct TargetRegistrationWitness {
    grant: oer_esp32s31_hal::owner::PhyRegistrationGrant,
}

impl TargetRegistrationWitness {
    const fn new(grant: oer_esp32s31_hal::owner::PhyRegistrationGrant) -> Self {
        Self { grant }
    }

    pub(crate) const fn epoch(&self) -> oer_esp32s31_hal::owner::PhyRegistrationEpoch {
        self.grant.epoch()
    }

    /// The registration's grant, which the registered state keeps.
    pub(crate) fn into_grant(self) -> oer_esp32s31_hal::owner::PhyRegistrationGrant {
        self.grant
    }
}

/// Complete target-side implementation of the registration graph port.
pub struct TargetPhyRegisterPort<'a, P, R, D, T, O = NoopPhyTargetObserver> {
    platform: &'a mut P,
    registers: &'a mut R,
    observer: O,
    counters: PhyTargetPortCounters,
    timer: &'a T,
    short: PhantomData<D>,
}

/// Complete target port for the bounded periodic-calibration parent.
///
/// It reuses the same leaf completer as cold registration, so periodic DCODE,
/// RX gain, channel, TXDC and gain publication cannot drift into a second
/// hardware implementation.
pub struct TargetPhyCalibrationTrackingPort<'a, P, R, G, D, T, O = NoopPhyTargetObserver> {
    platform: &'a mut P,
    registers: &'a mut R,
    grant: &'a mut G,
    observer: &'a mut O,
    timer: &'a T,
    short: PhantomData<D>,
}

/// Complete target port for one affine outer periodic-tracking request.
pub struct TargetPhyParamTrackingPort<'a, P, R, G, D, T, O = NoopPhyTargetObserver> {
    platform: &'a mut P,
    registers: &'a mut R,
    grant: &'a mut G,
    observer: O,
    timer: &'a T,
    short: PhantomData<D>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TargetPhyParamTrackingError {
    /// The registration no longer describes the borrowed PHY partition; the
    /// request was poisoned before any hardware access.
    EpochMismatch,
    Run(PhyParamTrackingRunError<PhyTargetPortError>),
    MissingCompletedOwner,
}

/// The leaf completer of the target ports, waiting on their timer.
struct TargetCompleter<'t, D, T> {
    timer: &'t T,
    short: PhantomData<D>,
}

impl<'t, D, T> TargetCompleter<'t, D, T> {
    const fn new(timer: &'t T) -> Self {
        Self {
            timer,
            short: PhantomData,
        }
    }
}

impl<D: PhyShortDelay, T: Timer> TargetCompleter<'_, D, T> {
    async fn complete_rfpll<P>(
        &self,
        binding: RfpllFrequencyExternalBinding,
        platform: &mut P,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<RfpllFrequencyCompletion, PhyTargetPortError> {
        self.complete_rfpll_with(binding, platform, registers, |kind, micros| {
            delay::<D, _>(self.timer, kind, micros)
        })
        .await
    }

    async fn complete_rfpll_with<P, F: core::future::Future<Output = ()>>(
        &self,
        binding: RfpllFrequencyExternalBinding,
        _platform: &mut P,
        registers: &mut impl SharedPhyAccess,
        mut delay: impl FnMut(crate::executor::wait::Kind, u64) -> F,
    ) -> Result<RfpllFrequencyCompletion, PhyTargetPortError> {
        match binding {
            RfpllFrequencyExternalBinding::Mmio(binding) => match binding.action() {
                RfpllFrequencyAction::ReadChannelReady { samples }
                    if samples >= CHANNEL_READY_SAMPLE_LIMIT =>
                {
                    Ok(RfpllFrequencyCompletion::ChannelReadyTimedOut)
                }
                _ => Ok(binding.execute_target(registers)),
            },
            RfpllFrequencyExternalBinding::I2c(binding) => {
                complete_rfpll_i2c(binding, registers, &mut delay).await
            }
            RfpllFrequencyExternalBinding::Timer(binding) => {
                delay(
                    crate::executor::wait::Kind::Settle,
                    u64::from(binding.micros()),
                )
                .await;
                Ok(binding.into_completion())
            }
        }
    }

    async fn complete_tx_calibration_environment(
        &self,
        binding: PhyTxCalibrationEnvironmentExternalBinding,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyTxCalibrationEnvironmentCompletion, PhyTargetPortError> {
        match binding {
            PhyTxCalibrationEnvironmentExternalBinding::Mmio(binding) => {
                Ok(binding.execute_target(registers))
            }
            PhyTxCalibrationEnvironmentExternalBinding::Pbus(binding) => {
                complete_tx_calibration_environment_pbus(binding, registers, |kind, micros| {
                    delay::<D, _>(self.timer, kind, micros)
                })
                .await
            }
            PhyTxCalibrationEnvironmentExternalBinding::Timer(binding) => {
                delay::<D, _>(self.timer, Kind::Settle, u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
        }
    }

    async fn complete_tone_sar(
        &self,
        binding: PhyToneSarExternalBinding,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyToneSarCompletion, PhyTargetPortError> {
        self.complete_tone_sar_with(binding, registers, |_, micros| {
            delay::<D, _>(self.timer, Kind::Settle, micros)
        })
        .await
    }

    async fn complete_tone_sar_with<F: core::future::Future<Output = ()>>(
        &self,
        binding: PhyToneSarExternalBinding,
        registers: &mut impl SharedPhyAccess,
        mut delay: impl FnMut(crate::tx::calibration::PhyToneSarDelayPhase, u64) -> F,
    ) -> Result<PhyToneSarCompletion, PhyTargetPortError> {
        match binding {
            PhyToneSarExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyToneSarExternalBinding::Timer(binding) => {
                let micros = binding.micros();
                let completion = binding.into_completion();
                let PhyToneSarCompletion::DelayElapsed { phase, .. } = completion else {
                    unreachable!()
                };
                delay(phase, u64::from(micros)).await;
                Ok(completion)
            }
        }
    }

    async fn complete_temperature<P>(
        &self,
        binding: PhyTemperatureExternalBinding,
        _platform: &mut P,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyTemperatureCompletion, PhyTargetPortError> {
        match binding {
            PhyTemperatureExternalBinding::I2c(binding) => {
                complete_temperature_i2c(binding, registers, |kind, micros| {
                    delay::<D, _>(self.timer, kind, micros)
                })
                .await
            }
            PhyTemperatureExternalBinding::Sample(binding) => Ok(binding.execute_target(registers)),
        }
    }

    async fn complete_power_control_point(
        &self,
        binding: PhyPowerControlPointExternalBinding,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyPowerControlPointCompletion, PhyTargetPortError> {
        match binding {
            PhyPowerControlPointExternalBinding::Mmio(binding) => {
                Ok(binding.execute_target(registers))
            }
            PhyPowerControlPointExternalBinding::ToneSar(binding) => {
                let completion = self.complete_tone_sar(binding, registers).await?;
                Ok(PhyPowerControlPointCompletion::ToneSar(completion))
            }
        }
    }

    async fn complete_tx_power<P>(
        &self,
        binding: PhyTxPowerExternalBinding,
        platform: &mut P,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyTxPowerCompletion, PhyTargetPortError> {
        match binding {
            PhyTxPowerExternalBinding::Environment(binding) => {
                Ok(PhyTxPowerCompletion::Environment(
                    self.complete_tx_calibration_environment(binding, registers)
                        .await?,
                ))
            }
            PhyTxPowerExternalBinding::Rfpll(binding) => Ok(PhyTxPowerCompletion::Rfpll(
                self.complete_rfpll(binding, platform, registers).await?,
            )),
            PhyTxPowerExternalBinding::I2c(binding) => {
                complete_tx_power_i2c(binding, registers, |kind, micros| {
                    delay::<D, _>(self.timer, kind, micros)
                })
                .await
            }
            PhyTxPowerExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyTxPowerExternalBinding::ToneSar(binding) => {
                let completion = self.complete_tone_sar(binding, registers).await?;
                Ok(PhyTxPowerCompletion::ToneSar(completion))
            }
            PhyTxPowerExternalBinding::Point(binding) => Ok(PhyTxPowerCompletion::Point(
                self.complete_power_control_point(binding, registers)
                    .await?,
            )),
        }
    }

    async fn complete_bluetooth_tx_power<P>(
        &self,
        binding: PhyBluetoothTxPowerExternalBinding,
        platform: &mut P,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyBluetoothTxPowerCompletion, PhyTargetPortError> {
        match binding {
            PhyBluetoothTxPowerExternalBinding::I2c(binding) => {
                complete_bluetooth_i2c::<D>(self.timer, binding, registers).await
            }
            PhyBluetoothTxPowerExternalBinding::Prepare(binding) => {
                Ok(PhyBluetoothTxPowerCompletion::Prepare(
                    self.complete_tx_calibration_environment(binding, registers)
                        .await?,
                ))
            }
            PhyBluetoothTxPowerExternalBinding::Cleanup(binding) => {
                Ok(PhyBluetoothTxPowerCompletion::Cleanup(
                    self.complete_tx_calibration_environment(binding, registers)
                        .await?,
                ))
            }
            PhyBluetoothTxPowerExternalBinding::Pbus(binding) => {
                complete_bluetooth_pbus(binding, registers, |kind, micros| {
                    delay::<D, _>(self.timer, kind, micros)
                })
                .await
            }
            PhyBluetoothTxPowerExternalBinding::ReadPbus(binding) => {
                Ok(binding.execute_target(registers))
            }
            PhyBluetoothTxPowerExternalBinding::Calibration(binding) => {
                Ok(PhyBluetoothTxPowerCompletion::Calibration(
                    self.complete_tx_power(binding, platform, registers).await?,
                ))
            }
        }
    }

    async fn complete_bluetooth_tx_gain<P, O: PhyTargetObserver>(
        &self,
        binding: PhyBluetoothTxGainInitExternalBinding,
        platform: &mut P,
        registers: &mut impl SharedPhyContext,
        observer: &mut O,
    ) -> Result<PhyBluetoothTxGainInitCompletion, PhyTargetPortError> {
        match binding {
            PhyBluetoothTxGainInitExternalBinding::Rfpll(binding) => {
                Ok(PhyBluetoothTxGainInitCompletion::Rfpll(
                    self.complete_rfpll(binding, platform, registers).await?,
                ))
            }
            PhyBluetoothTxGainInitExternalBinding::TxCap(binding) => {
                Ok(PhyBluetoothTxGainInitCompletion::TxCap(
                    self.complete_tx_power(binding, platform, registers).await?,
                ))
            }
            PhyBluetoothTxGainInitExternalBinding::TxDc(binding) => {
                Ok(PhyBluetoothTxGainInitCompletion::TxDc(
                    self.complete_tx_dc(binding, registers, observer).await?,
                ))
            }
            PhyBluetoothTxGainInitExternalBinding::TxPower(binding) => {
                Ok(PhyBluetoothTxGainInitCompletion::TxPower(
                    self.complete_bluetooth_tx_power(binding, platform, registers)
                        .await?,
                ))
            }
            PhyBluetoothTxGainInitExternalBinding::TxDcPwdet(binding) => {
                Ok(PhyBluetoothTxGainInitCompletion::TxDcPwdet(
                    self.complete_tx_dc_pwdet(binding, registers).await?,
                ))
            }
            PhyBluetoothTxGainInitExternalBinding::ForceTxRx(binding) => {
                Ok(PhyBluetoothTxGainInitCompletion::ForceTxRx(match binding {
                    PhyForceTxRxExternalBinding::Mmio(binding) => binding.execute_target(registers),
                    PhyForceTxRxExternalBinding::Timer(binding) => {
                        delay::<D, _>(self.timer, Kind::Settle, u64::from(binding.micros())).await;
                        binding.into_completion()
                    }
                }))
            }
            PhyBluetoothTxGainInitExternalBinding::Publish(binding) => {
                Ok(binding.execute_target(registers))
            }
        }
    }

    async fn complete_power_attenuation(
        &self,
        binding: PhyPowerAttenuationExternalBinding,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyPowerAttenuationCompletion, PhyTargetPortError> {
        match binding {
            PhyPowerAttenuationExternalBinding::Mmio(binding) => {
                Ok(binding.execute_target(registers))
            }
            PhyPowerAttenuationExternalBinding::ToneSar(binding) => {
                let completion = self.complete_tone_sar(binding, registers).await?;
                Ok(PhyPowerAttenuationCompletion::ToneSar(completion))
            }
        }
    }

    async fn complete_tx_cap_search<P>(
        &self,
        binding: PhyTxCapSearchExternalBinding,
        _platform: &mut P,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyTxCapSearchCompletion, PhyTargetPortError> {
        match binding {
            PhyTxCapSearchExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyTxCapSearchExternalBinding::I2c(binding) => Ok(PhyTxCapSearchCompletion::I2c(
                complete_masked_i2c(binding, registers, |kind, micros| {
                    delay::<D, _>(self.timer, kind, micros)
                })
                .await?,
            )),
            PhyTxCapSearchExternalBinding::ToneSar(binding) => {
                let completion = self.complete_tone_sar(binding, registers).await?;
                Ok(PhyTxCapSearchCompletion::ToneSar(completion))
            }
        }
    }

    async fn complete_tx_cap<P>(
        &self,
        binding: PhyTxCapExternalBinding,
        platform: &mut P,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyTxCapCompletion, PhyTargetPortError> {
        match binding {
            PhyTxCapExternalBinding::Environment(binding) => Ok(PhyTxCapCompletion::Environment(
                self.complete_tx_calibration_environment(binding, registers)
                    .await?,
            )),
            PhyTxCapExternalBinding::Rfpll(binding) => Ok(PhyTxCapCompletion::Rfpll(
                self.complete_rfpll(binding, platform, registers).await?,
            )),
            PhyTxCapExternalBinding::I2c(binding) => Ok(PhyTxCapCompletion::I2c(
                complete_masked_i2c(binding, registers, |kind, micros| {
                    delay::<D, _>(self.timer, kind, micros)
                })
                .await?,
            )),
            PhyTxCapExternalBinding::Attenuation(binding) => Ok(PhyTxCapCompletion::Attenuation(
                self.complete_power_attenuation(binding, registers).await?,
            )),
            PhyTxCapExternalBinding::Search(binding) => Ok(PhyTxCapCompletion::Search(
                self.complete_tx_cap_search(binding, platform, registers)
                    .await?,
            )),
        }
    }

    async fn complete_tx_dc_pwdet_search<O: PhyTargetObserver>(
        &self,
        binding: PhyTxDcPwdetSearchExternalBinding,
        registers: &mut impl SharedPhyAccess,
        observer: &core::cell::RefCell<&mut O>,
    ) -> Result<PhyTxDcPwdetSearchCompletion, PhyTargetPortError> {
        use crate::executor::wait::{Kind, tx::Scope};
        let delay = |scope, kind, micros| {
            delay_observed::<D, _>(self.timer, kind, micros, O::OBSERVE_DELAYS, move |event| {
                observer.borrow_mut().tx_wait(scope, kind, event)
            })
        };
        match binding {
            PhyTxDcPwdetSearchExternalBinding::Pbus(binding) => {
                complete_tx_dc_pwdet_search_pbus(binding, registers, |kind, micros| {
                    delay(Scope::Pbus, kind, micros)
                })
                .await
            }
            PhyTxDcPwdetSearchExternalBinding::Timer(binding) => {
                delay(Scope::Search, Kind::Settle, u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyTxDcPwdetSearchExternalBinding::ToneSar(binding) => {
                let completion = self
                    .complete_tone_sar_with(binding, registers, |phase, micros| {
                        let scope = match phase {
                            crate::tx::calibration::PhyToneSarDelayPhase::ToneArmed => Scope::Tone,
                            crate::tx::calibration::PhyToneSarDelayPhase::SarTriggered => {
                                Scope::Sar
                            }
                        };
                        delay(scope, Kind::Settle, micros)
                    })
                    .await?;
                if let PhyToneSarCompletion::ReadySampled { ready, .. } = completion {
                    observer.borrow_mut().tx_sar_ready(ready);
                }
                Ok(PhyTxDcPwdetSearchCompletion::ToneSar(completion))
            }
        }
    }

    async fn complete_tx_dc_pwdet(
        &self,
        binding: PhyTxDcPwdetExternalBinding,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyTxDcPwdetCompletion, PhyTargetPortError> {
        let mut observer = NoopPhyTargetObserver;
        self.complete_tx_dc_pwdet_with(binding, registers, &core::cell::RefCell::new(&mut observer))
            .await
    }

    async fn complete_tx_dc_pwdet_with<O: PhyTargetObserver>(
        &self,
        binding: PhyTxDcPwdetExternalBinding,
        registers: &mut impl SharedPhyContext,
        observer: &core::cell::RefCell<&mut O>,
    ) -> Result<PhyTxDcPwdetCompletion, PhyTargetPortError> {
        use crate::executor::wait::{Kind, tx::Scope};
        let delay = |scope, kind, micros| {
            delay_observed::<D, _>(self.timer, kind, micros, O::OBSERVE_DELAYS, move |event| {
                observer.borrow_mut().tx_wait(scope, kind, event)
            })
        };
        match binding {
            PhyTxDcPwdetExternalBinding::Mmio(binding) => binding
                .execute_target(registers)
                .map_err(|_| PhyTargetPortError::HardwareInvariant),
            PhyTxDcPwdetExternalBinding::Pbus(binding) => {
                complete_tx_dc_pwdet_pbus(binding, registers, |kind, micros| {
                    delay(Scope::Pbus, kind, micros)
                })
                .await
            }
            PhyTxDcPwdetExternalBinding::Timer(binding) => {
                delay(Scope::Root, Kind::Settle, u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyTxDcPwdetExternalBinding::Search(binding) => Ok(PhyTxDcPwdetCompletion::Search(
                self.complete_tx_dc_pwdet_search(binding, registers, observer)
                    .await?,
            )),
        }
    }

    async fn complete_dcode<P, F: core::future::Future<Output = ()>>(
        &self,
        binding: PhyDcodeExternalBinding,
        platform: &mut P,
        registers: &mut impl SharedPhyAccess,
        mut delay: impl FnMut(crate::executor::wait::Scope, crate::executor::wait::Kind, u64) -> F,
        mut observe: impl FnMut(bool),
    ) -> Result<PhyDcodeCompletion, PhyTargetPortError> {
        match binding {
            PhyDcodeExternalBinding::Rfpll(binding) => {
                use crate::executor::wait::Scope;
                let completion = self
                    .complete_rfpll_with(binding, platform, registers, |kind, micros| {
                        delay(Scope::Rfpll, kind, micros)
                    })
                    .await?;
                if let RfpllFrequencyCompletion::MaskedRead {
                    field: crate::analog::i2c::analog_registers::RFPLL_LOCK_STATUS,
                    value,
                } = completion
                {
                    observe(value != 0);
                }
                Ok(PhyDcodeCompletion::Rfpll(completion))
            }
            PhyDcodeExternalBinding::I2c(binding) => {
                complete_dcode_i2c(binding, registers, |kind, micros| {
                    delay(crate::executor::wait::Scope::Dcode, kind, micros)
                })
                .await
            }
        }
    }

    async fn complete_txiq_linear_power(
        &self,
        binding: PhyTxIqLinearPowerExternalBinding,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyTxIqLinearPowerCompletion, PhyTargetPortError> {
        match binding {
            PhyTxIqLinearPowerExternalBinding::ToneSar(binding) => {
                Ok(PhyTxIqLinearPowerCompletion::ToneSar(
                    self.complete_tone_sar(binding, registers).await?,
                ))
            }
        }
    }

    async fn complete_txiq_mis_power(
        &self,
        binding: PhyTxIqMisPowerExternalBinding,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyTxIqMisPowerCompletion, PhyTargetPortError> {
        match binding {
            PhyTxIqMisPowerExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyTxIqMisPowerExternalBinding::Timer(binding) => {
                delay::<D, _>(self.timer, Kind::Settle, u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyTxIqMisPowerExternalBinding::LinearPower(binding) => {
                Ok(PhyTxIqMisPowerCompletion::LinearPower(
                    self.complete_txiq_linear_power(binding, registers).await?,
                ))
            }
        }
    }

    async fn complete_txiq_cover(
        &self,
        binding: PhyTxIqCoverExternalBinding,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyTxIqCoverCompletion, PhyTargetPortError> {
        match binding {
            PhyTxIqCoverExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyTxIqCoverExternalBinding::MisPower(binding) => Ok(PhyTxIqCoverCompletion::MisPower(
                self.complete_txiq_mis_power(binding, registers).await?,
            )),
        }
    }

    async fn complete_txiq_loopback<P>(
        &self,
        binding: PhyTxIqLoopbackExternalBinding,
        _platform: &mut P,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyTxIqLoopbackCompletion, PhyTargetPortError> {
        match binding {
            PhyTxIqLoopbackExternalBinding::I2c(binding) => Ok(PhyTxIqLoopbackCompletion::I2c(
                complete_masked_i2c(binding, registers, |kind, micros| {
                    delay::<D, _>(self.timer, kind, micros)
                })
                .await?,
            )),
            PhyTxIqLoopbackExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
        }
    }

    async fn complete_txiq_calibration<P>(
        &self,
        binding: PhyTxIqCalibrationExternalBinding,
        platform: &mut P,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyTxIqCalibrationCompletion, PhyTargetPortError> {
        match binding {
            PhyTxIqCalibrationExternalBinding::Mmio(binding) => binding
                .execute_target(registers)
                .map_err(|_| PhyTargetPortError::HardwareInvariant),
            PhyTxIqCalibrationExternalBinding::Loopback(binding) => {
                Ok(PhyTxIqCalibrationCompletion::Loopback(
                    self.complete_txiq_loopback(binding, platform, registers)
                        .await?,
                ))
            }
            PhyTxIqCalibrationExternalBinding::Pbus(binding) => {
                complete_txiq_pbus(binding, registers, |kind, micros| {
                    delay::<D, _>(self.timer, kind, micros)
                })
                .await
            }
            PhyTxIqCalibrationExternalBinding::Environment(binding) => {
                Ok(PhyTxIqCalibrationCompletion::Environment(
                    self.complete_tx_calibration_environment(binding, registers)
                        .await?,
                ))
            }
            PhyTxIqCalibrationExternalBinding::PowerAttenuation(binding) => {
                Ok(PhyTxIqCalibrationCompletion::PowerAttenuation(
                    self.complete_power_attenuation(binding, registers).await?,
                ))
            }
            PhyTxIqCalibrationExternalBinding::Cover(binding) => {
                Ok(PhyTxIqCalibrationCompletion::Cover(
                    self.complete_txiq_cover(binding, registers).await?,
                ))
            }
        }
    }

    async fn complete_txiq<P>(
        &self,
        binding: PhyTxIqInitExternalBinding,
        platform: &mut P,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyTxIqInitCompletion, PhyTargetPortError> {
        match binding {
            PhyTxIqInitExternalBinding::Rfpll(binding) => Ok(PhyTxIqInitCompletion::Rfpll(
                self.complete_rfpll(binding, platform, registers).await?,
            )),
            PhyTxIqInitExternalBinding::I2c(binding) => {
                complete_txiq_init_i2c(binding, registers, |kind, micros| {
                    delay::<D, _>(self.timer, kind, micros)
                })
                .await
            }
            PhyTxIqInitExternalBinding::Calibration(binding) => {
                Ok(PhyTxIqInitCompletion::Calibration(
                    self.complete_txiq_calibration(binding, platform, registers)
                        .await?,
                ))
            }
            PhyTxIqInitExternalBinding::Temperature(binding) => {
                Ok(PhyTxIqInitCompletion::Temperature(
                    self.complete_temperature(binding, platform, registers)
                        .await?,
                ))
            }
        }
    }

    async fn complete_dc_iq(
        &self,
        binding: PhyDcIqExternalBinding,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyDcIqCompletion, PhyTargetPortError> {
        match binding {
            PhyDcIqExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyDcIqExternalBinding::Timer(binding) => {
                delay::<D, _>(self.timer, Kind::Settle, u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyDcIqExternalBinding::Readiness(binding) => Ok(binding
                .observe_with(HARDWARE_EDGE_LIMIT, || {
                    crate::calibration::estimator::sample_readiness_target(registers)
                })),
        }
    }

    async fn complete_rx_dco(
        &self,
        binding: PhyRxDcoExternalBinding,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyRxDcoCompletion, PhyTargetPortError> {
        match binding {
            PhyRxDcoExternalBinding::Mmio(binding) => binding
                .execute_target(registers)
                .map_err(|_| PhyTargetPortError::HardwareInvariant),
            PhyRxDcoExternalBinding::Pbus(binding) => {
                complete_rx_dco_pbus(binding, registers, |kind, micros| {
                    delay::<D, _>(self.timer, kind, micros)
                })
                .await
            }
            PhyRxDcoExternalBinding::Timer(binding) => {
                delay::<D, _>(self.timer, Kind::Settle, u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyRxDcoExternalBinding::DcIq(binding) => Ok(PhyRxDcoCompletion::DcIq(
                self.complete_dc_iq(binding, registers).await?,
            )),
        }
    }

    async fn complete_rxiq_estimator(
        &self,
        binding: PhyRxIqEstimatorExternalBinding,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyRxIqEstimatorCompletion, PhyTargetPortError> {
        match binding {
            PhyRxIqEstimatorExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyRxIqEstimatorExternalBinding::Timer(binding) => {
                delay::<D, _>(self.timer, Kind::Settle, u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyRxIqEstimatorExternalBinding::Readiness(binding) => {
                if binding.samples() >= HARDWARE_EDGE_LIMIT {
                    Ok(binding.into_timeout_completion())
                } else {
                    delay::<D, _>(self.timer, Kind::Completion, 1).await;
                    Ok(binding.execute_target(registers))
                }
            }
        }
    }

    async fn complete_rxiq_cover(
        &self,
        binding: PhyRxIqCoverExternalBinding,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyRxIqCoverCompletion, PhyTargetPortError> {
        match binding {
            PhyRxIqCoverExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyRxIqCoverExternalBinding::Estimator(binding) => {
                Ok(PhyRxIqCoverCompletion::Estimator(
                    self.complete_rxiq_estimator(binding, registers).await?,
                ))
            }
        }
    }

    async fn complete_rxiq_rf_calibration(
        &self,
        binding: PhyRxIqRfCalibrationExternalBinding,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyRxIqRfCalibrationCompletion, PhyTargetPortError> {
        match binding {
            PhyRxIqRfCalibrationExternalBinding::Mmio(binding) => {
                Ok(binding.execute_target(registers))
            }
            PhyRxIqRfCalibrationExternalBinding::Cover(binding) => {
                Ok(PhyRxIqRfCalibrationCompletion::Cover(
                    self.complete_rxiq_cover(binding, registers).await?,
                ))
            }
        }
    }

    async fn complete_rxiq_data(
        &self,
        binding: PhyRxIqDataExternalBinding,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyRxIqDataCompletion, PhyTargetPortError> {
        match binding {
            PhyRxIqDataExternalBinding::Calibration(binding) => {
                Ok(PhyRxIqDataCompletion::Calibration(
                    self.complete_rxiq_rf_calibration(binding, registers)
                        .await?,
                ))
            }
        }
    }

    async fn complete_rxiq_gain<P>(
        &self,
        binding: PhyRxIqGainExternalBinding,
        _platform: &mut P,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyRxIqGainCompletion, PhyTargetPortError> {
        match binding {
            PhyRxIqGainExternalBinding::Pbus(binding) => {
                complete_rxiq_gain_pbus(binding, registers, |kind, micros| {
                    delay::<D, _>(self.timer, kind, micros)
                })
                .await
            }
            PhyRxIqGainExternalBinding::I2c(binding) => {
                complete_rxiq_gain_i2c(binding, registers, |kind, micros| {
                    delay::<D, _>(self.timer, kind, micros)
                })
                .await
            }
            PhyRxIqGainExternalBinding::AdjustTx(binding) => Ok(PhyRxIqGainCompletion::AdjustTx(
                complete_rxiq_adjusted_tx_i2c(binding, registers, |kind, micros| {
                    delay::<D, _>(self.timer, kind, micros)
                })
                .await?,
            )),
            PhyRxIqGainExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyRxIqGainExternalBinding::Dco(binding) => Ok(PhyRxIqGainCompletion::Dco(
                self.complete_rx_dco(binding, registers).await?,
            )),
            PhyRxIqGainExternalBinding::Estimator(binding) => Ok(PhyRxIqGainCompletion::Estimator(
                self.complete_rxiq_estimator(binding, registers).await?,
            )),
            PhyRxIqGainExternalBinding::Data(binding) => Ok(PhyRxIqGainCompletion::Data(
                self.complete_rxiq_data(binding, registers).await?,
            )),
        }
    }

    async fn complete_rxiq<P>(
        &self,
        binding: PhyRxIqInitExternalBinding,
        platform: &mut P,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyRxIqInitCompletion, PhyTargetPortError> {
        match binding {
            PhyRxIqInitExternalBinding::Rfpll(binding) => Ok(PhyRxIqInitCompletion::Rfpll(
                self.complete_rfpll(binding, platform, registers).await?,
            )),
            PhyRxIqInitExternalBinding::I2c(binding) => {
                complete_rxiq_init_i2c(binding, registers, |kind, micros| {
                    delay::<D, _>(self.timer, kind, micros)
                })
                .await
            }
            PhyRxIqInitExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyRxIqInitExternalBinding::Pbus(binding) => {
                complete_rxiq_init_pbus(binding, registers, |kind, micros| {
                    delay::<D, _>(self.timer, kind, micros)
                })
                .await
            }
            PhyRxIqInitExternalBinding::Loopback(binding) => Ok(PhyRxIqInitCompletion::Loopback(
                self.complete_txiq_loopback(binding, platform, registers)
                    .await?,
            )),
            PhyRxIqInitExternalBinding::Gain(binding) => Ok(PhyRxIqInitCompletion::Gain(
                self.complete_rxiq_gain(binding, platform, registers)
                    .await?,
            )),
            PhyRxIqInitExternalBinding::Timer(binding) => {
                delay::<D, _>(self.timer, Kind::Settle, u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
        }
    }

    async fn complete_rx_saturation(
        &self,
        binding: PhyRxSaturationExternalBinding,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyRxSaturationCompletion, PhyTargetPortError> {
        match binding {
            PhyRxSaturationExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyRxSaturationExternalBinding::Pbus(binding) => {
                complete_rx_saturation_pbus(binding, registers, |kind, micros| {
                    delay::<D, _>(self.timer, kind, micros)
                })
                .await
            }
            PhyRxSaturationExternalBinding::Timer(binding) => {
                delay::<D, _>(self.timer, Kind::Settle, u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyRxSaturationExternalBinding::Sample(binding) => {
                Ok(binding.execute_target(registers))
            }
        }
    }

    async fn complete_rx_dc_minimum(
        &self,
        binding: PhyRxDcMinimumExternalBinding,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyRxDcMinimumCompletion, PhyTargetPortError> {
        match binding {
            PhyRxDcMinimumExternalBinding::DcIq(binding) => Ok(PhyRxDcMinimumCompletion::DcIq(
                self.complete_dc_iq(binding, registers).await?,
            )),
        }
    }

    async fn complete_rx_dc_calibration(
        &self,
        binding: PhyRxDcCalibrationExternalBinding,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyRxDcCalibrationCompletion, PhyTargetPortError> {
        match binding {
            PhyRxDcCalibrationExternalBinding::Mmio(binding) => binding
                .execute_target(registers)
                .map_err(|_| PhyTargetPortError::HardwareInvariant),
            PhyRxDcCalibrationExternalBinding::Pbus(binding) => {
                complete_rx_dc_calibration_pbus(binding, registers, self.timer)
            }
            PhyRxDcCalibrationExternalBinding::Timer(binding) => {
                delay::<D, _>(self.timer, Kind::Settle, u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyRxDcCalibrationExternalBinding::Minimum(binding) => {
                Ok(PhyRxDcCalibrationCompletion::Minimum(
                    self.complete_rx_dc_minimum(binding, registers).await?,
                ))
            }
        }
    }

    async fn complete_rx_gain_dc<P>(
        &self,
        binding: PhyRxGainDcExternalBinding,
        platform: &mut P,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyRxGainDcCompletion, PhyTargetPortError> {
        match binding {
            PhyRxGainDcExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyRxGainDcExternalBinding::Rfpll(binding) => Ok(PhyRxGainDcCompletion::Rfpll(
                self.complete_rfpll(binding, platform, registers).await?,
            )),
            PhyRxGainDcExternalBinding::Pbus(binding) => {
                complete_rx_gain_dc_pbus(binding, registers, self.timer)
            }
            PhyRxGainDcExternalBinding::I2c(binding) => Ok(PhyRxGainDcCompletion::I2c(
                complete_rx_gain_i2c(binding, registers, self.timer)?,
            )),
            PhyRxGainDcExternalBinding::Calibration(binding) => {
                Ok(PhyRxGainDcCompletion::Calibration(
                    self.complete_rx_dc_calibration(binding, registers).await?,
                ))
            }
            PhyRxGainDcExternalBinding::Timer(binding) => {
                delay::<D, _>(self.timer, Kind::Settle, u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
        }
    }

    async fn complete_rx_gain_publish(
        &self,
        binding: PhyRxGainPublishExternalBinding,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyRxGainPublishCompletion, PhyTargetPortError> {
        match binding {
            PhyRxGainPublishExternalBinding::Mmio(binding) => Ok(binding.execute_target(registers)),
            PhyRxGainPublishExternalBinding::Pbus(binding) => {
                complete_rx_gain_publish_pbus(binding, registers, self.timer)
            }
            PhyRxGainPublishExternalBinding::Timer(binding) => {
                delay::<D, _>(self.timer, Kind::Settle, u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
        }
    }

    async fn complete_rx_gain<P>(
        &self,
        binding: PhyRxGainInitExternalBinding,
        platform: &mut P,
        registers: &mut impl SharedPhyContext,
    ) -> Result<PhyRxGainInitCompletion, PhyTargetPortError> {
        match binding {
            PhyRxGainInitExternalBinding::Mmio(binding) => binding
                .execute_target(registers)
                .map_err(|_| PhyTargetPortError::HardwareInvariant),
            PhyRxGainInitExternalBinding::Dc(binding) => Ok(PhyRxGainInitCompletion::Dc(
                self.complete_rx_gain_dc(binding, platform, registers)
                    .await?,
            )),
            PhyRxGainInitExternalBinding::Publish(binding) => Ok(PhyRxGainInitCompletion::Publish(
                self.complete_rx_gain_publish(binding, registers).await?,
            )),
        }
    }

    async fn complete_channel<P, O: PhyTargetObserver>(
        &self,
        binding: PhyChipChannelExternalBinding,
        platform: &mut P,
        registers: &mut impl SharedPhyAccess,
        observer: &mut O,
    ) -> Result<PhyChipChannelCompletion, PhyTargetPortError> {
        match binding {
            PhyChipChannelExternalBinding::Mmio(binding) => match binding.action() {
                PhyChipChannelAction::AwaitFrequencyReadyEdge { samples, .. }
                    if samples >= CHANNEL_READY_SAMPLE_LIMIT =>
                {
                    observer.channel_frequency_ready_timed_out(samples);
                    Ok(PhyChipChannelCompletion::FrequencyReadyTimedOut)
                }
                _ => Ok(binding.execute_target(platform, registers)),
            },
            PhyChipChannelExternalBinding::Temperature(binding) => {
                Ok(PhyChipChannelCompletion::Temperature(
                    self.complete_temperature(binding, platform, registers)
                        .await?,
                ))
            }
            PhyChipChannelExternalBinding::Timer(binding) => {
                delay::<D, _>(self.timer, Kind::Settle, u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyChipChannelExternalBinding::I2c(binding) => {
                complete_channel_i2c(binding, registers, |kind, micros| {
                    delay::<D, _>(self.timer, kind, micros)
                })
                .await
            }
            PhyChipChannelExternalBinding::TxGain(binding) => {
                let request = binding.request();
                let completion = binding.execute();
                if let PhyChipChannelCompletion::TxGainCalculated { image, .. } = completion {
                    observer.channel_tx_gain(request, image);
                }
                Ok(completion)
            }
        }
    }

    async fn complete_channel_hal<P, O: PhyTargetObserver>(
        &self,
        binding: PhyChipChannelExternalBinding,
        channel: &mut oer_esp32s31_hal::ieee80211::channel::RadioChannelHal<'_, P>,
        observer: &mut O,
    ) -> Result<PhyChipChannelCompletion, PhyTargetPortError> {
        match binding {
            PhyChipChannelExternalBinding::Mmio(binding) => match binding.action() {
                PhyChipChannelAction::AwaitFrequencyReadyEdge { samples, .. }
                    if samples >= CHANNEL_READY_SAMPLE_LIMIT =>
                {
                    observer.channel_frequency_ready_timed_out(samples);
                    Ok(PhyChipChannelCompletion::FrequencyReadyTimedOut)
                }
                _ => Ok(binding.execute_channel_hal(channel)),
            },
            PhyChipChannelExternalBinding::Temperature(binding) => {
                Ok(PhyChipChannelCompletion::Temperature(match binding {
                    PhyTemperatureExternalBinding::I2c(binding) => {
                        complete_temperature_i2c(binding, channel, |kind, micros| {
                            delay::<D, _>(self.timer, kind, micros)
                        })
                        .await?
                    }
                    PhyTemperatureExternalBinding::Sample(binding) => {
                        binding.execute_target(channel)
                    }
                }))
            }
            PhyChipChannelExternalBinding::Timer(binding) => {
                delay::<D, _>(self.timer, Kind::Settle, u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyChipChannelExternalBinding::I2c(binding) => {
                complete_channel_i2c(binding, channel, |kind, micros| {
                    delay::<D, _>(self.timer, kind, micros)
                })
                .await
            }
            PhyChipChannelExternalBinding::TxGain(binding) => {
                let request = binding.request();
                let completion = binding.execute();
                if let PhyChipChannelCompletion::TxGainCalculated { image, .. } = completion {
                    observer.channel_tx_gain(request, image);
                }
                Ok(completion)
            }
        }
    }

    async fn select_channel_hal<P, O: PhyTargetObserver>(
        &self,
        state: &mut PhyState,
        channel_or_frequency: u16,
        cbw: u8,
        channel: &mut oer_esp32s31_hal::ieee80211::channel::RadioChannelHal<'_, P>,
        observer: &mut O,
    ) -> Result<(), PhyTargetPortError> {
        let mut transition = PhyChipChannelTransition::new(PhyChipChannelRequest {
            channel_or_frequency,
            cbw,
            parameters: state.channel_parameters(),
        });

        for operation in 0..RF_OPERATION_LIMIT {
            match transition.action() {
                PhyChipChannelAction::Complete(outcome) => {
                    state.apply_channel_outcome(outcome);
                    observer.channel_completed(outcome, operation);
                    return Ok(());
                }
                PhyChipChannelAction::Failed(failure) => {
                    observer.channel_failed(failure, operation);
                    return Err(PhyTargetPortError::UnexpectedBinding);
                }
                action => {
                    let binding = PhyChipChannelExternalBinding::lower(action)
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                    let completion = self
                        .complete_channel_hal(binding, channel, observer)
                        .await?;
                    transition
                        .advance(completion)
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                }
            }
        }

        Err(PhyTargetPortError::RfOperationLimit)
    }

    async fn switch_channel_hal_with_mac_restart<P, O: PhyTargetObserver>(
        &self,
        state: &mut PhyState,
        channel_or_frequency: u16,
        cbw: u8,
        channel: &mut oer_esp32s31_hal::ieee80211::channel::RadioChannelHal<'_, P>,
        observer: &mut O,
    ) -> Result<(), PhyTargetPortError> {
        channel.request_mac_stop();
        delay::<D, _>(self.timer, Kind::Settle, MAC_CHANNEL_SETTLE_US).await;
        for _ in 0..RF_OPERATION_LIMIT {
            if channel.mac_active_state() == 0 {
                delay::<D, _>(self.timer, Kind::Settle, MAC_CHANNEL_IDLE_SETTLE_US).await;
                self.select_channel_hal(state, channel_or_frequency, cbw, channel, observer)
                    .await?;
                let regdma_link = channel.restart_mac();
                observer.mac_channel_restarted(channel_or_frequency, cbw, regdma_link);
                return Ok(());
            }
            delay::<D, _>(self.timer, Kind::Completion, 1).await;
        }
        Err(PhyTargetPortError::HardwareEdgeTimedOut)
    }

    async fn complete_tx_dc<O: PhyTargetObserver>(
        &self,
        binding: PhyTxDcExternalBinding,
        registers: &mut impl SharedPhyContext,
        observer: &mut O,
    ) -> Result<PhyTxDcCompletion, PhyTargetPortError> {
        match binding {
            PhyTxDcExternalBinding::Mmio(binding) => {
                if binding.action() == PhyTxDcAction::ConfigurePbusDebugMode {
                    observer.tx_dc_entry();
                }
                let completion = binding.execute_target(registers);
                if let PhyTxDcCompletion::ComparatorsRead {
                    gain_index,
                    iteration,
                    comparator_high,
                } = completion
                {
                    observer.tx_dc_comparator(gain_index, iteration, comparator_high);
                }
                Ok(completion)
            }
            PhyTxDcExternalBinding::Ready(binding) => {
                Ok(binding.execute_target(registers, self.timer))
            }
            PhyTxDcExternalBinding::Timer(binding) => {
                delay::<D, _>(self.timer, Kind::Settle, u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyTxDcExternalBinding::Pbus(binding) => {
                complete_tx_dc_pbus(binding, registers, |kind, micros| {
                    delay::<D, _>(self.timer, kind, micros)
                })
                .await
            }
        }
    }

    async fn complete_pwdet<O: PhyTargetObserver>(
        &self,
        binding: PhyPwdetExternalBinding,
        registers: &mut impl SharedPhyContext,
        observer: &mut O,
    ) -> Result<PhyPwdetCompletion, PhyTargetPortError> {
        match binding {
            PhyPwdetExternalBinding::Mmio(binding) => {
                let completion = binding.execute_target(registers);
                if let PhyPwdetCompletion::SarSampled {
                    measurement_index,
                    sample_index,
                    value,
                    ..
                } = completion
                {
                    observer.power_detector_sample(measurement_index, sample_index, value);
                }
                Ok(completion)
            }
            PhyPwdetExternalBinding::Ready(binding) => {
                Ok(binding.execute_target(registers, self.timer))
            }
            PhyPwdetExternalBinding::Timer(binding) => {
                delay::<D, _>(self.timer, Kind::Settle, u64::from(binding.micros())).await;
                Ok(binding.into_completion())
            }
            PhyPwdetExternalBinding::Pbus(mut binding) => {
                let mut started = false;
                for _ in 0..HARDWARE_EDGE_LIMIT {
                    if binding.start_target(registers).is_ok() {
                        started = true;
                        break;
                    }
                    delay::<D, _>(self.timer, Kind::BusBusy, 1).await;
                }
                if !started {
                    return Ok(binding.into_timeout_completion());
                }
                for _ in 0..HARDWARE_EDGE_LIMIT {
                    delay::<D, _>(self.timer, Kind::Completion, 1).await;
                    match binding
                        .sample_target_once(registers)
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                    {
                        PhyPwdetPbusObservation::Completed => {
                            return binding
                                .into_completion()
                                .map_err(|_| PhyTargetPortError::UnexpectedBinding);
                        }
                        PhyPwdetPbusObservation::StillPending => {}
                    }
                }
                Ok(binding.into_timeout_completion())
            }
        }
    }

    async fn complete_baseband<P, O: PhyTargetObserver>(
        &self,
        binding: PhyBbExternalBinding,
        platform: &mut P,
        registers: &mut impl PhyInitializationAccess,
        observer: &mut O,
    ) -> Result<PhyBbInitCompletion, PhyTargetPortError> {
        match binding {
            PhyBbExternalBinding::Mmio(binding) => {
                Ok(PhyBbInitCompletion::Mmio(binding.execute_target(registers)))
            }
            PhyBbExternalBinding::TxDc(binding) => Ok(PhyBbInitCompletion::TxDc(
                self.complete_tx_dc(binding, registers, observer).await?,
            )),
            PhyBbExternalBinding::Pwdet(binding) => Ok(PhyBbInitCompletion::Pwdet(
                self.complete_pwdet(binding, registers, observer).await?,
            )),
            PhyBbExternalBinding::TxCap(binding) => Ok(PhyBbInitCompletion::TxCap(
                self.complete_tx_cap(binding, platform, registers).await?,
            )),
            PhyBbExternalBinding::Temperature(binding) => Ok(PhyBbInitCompletion::Temperature(
                self.complete_temperature(binding, platform, registers)
                    .await?,
            )),
            PhyBbExternalBinding::TxPower(binding) => Ok(PhyBbInitCompletion::TxPower(
                self.complete_tx_power(binding, platform, registers).await?,
            )),
            PhyBbExternalBinding::TxDcPwdet(binding) => Ok(PhyBbInitCompletion::TxDcPwdet(
                self.complete_tx_dc_pwdet(binding, registers).await?,
            )),
            PhyBbExternalBinding::Dcode(binding) => Ok(PhyBbInitCompletion::Dcode(
                self.complete_dcode(
                    binding,
                    platform,
                    registers,
                    |_, kind, micros| delay::<D, _>(self.timer, kind, micros),
                    |_| {},
                )
                .await?,
            )),
            PhyBbExternalBinding::TxIq(binding) => Ok(PhyBbInitCompletion::TxIq(
                self.complete_txiq(binding, platform, registers).await?,
            )),
            PhyBbExternalBinding::TxCfr(binding) => Ok(PhyBbInitCompletion::TxCfr(
                binding.execute_target(registers),
            )),
            PhyBbExternalBinding::BluetoothTxGain(binding) => {
                Ok(PhyBbInitCompletion::BluetoothTxGain(
                    self.complete_bluetooth_tx_gain(binding, platform, registers, observer)
                        .await?,
                ))
            }
            PhyBbExternalBinding::PbusMemory(binding) => Ok(PhyBbInitCompletion::PbusMemory(
                binding
                    .execute_target(registers)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
            )),
            PhyBbExternalBinding::RxIq(binding) => Ok(PhyBbInitCompletion::RxIq(
                self.complete_rxiq(binding, platform, registers).await?,
            )),
            PhyBbExternalBinding::RxSaturation(binding) => Ok(PhyBbInitCompletion::RxSaturation(
                self.complete_rx_saturation(binding, registers).await?,
            )),
            PhyBbExternalBinding::RxGain(binding) => Ok(PhyBbInitCompletion::RxGain(
                self.complete_rx_gain(binding, platform, registers).await?,
            )),
            PhyBbExternalBinding::Channel(binding) => Ok(PhyBbInitCompletion::Channel(
                self.complete_channel(binding, platform, registers, observer)
                    .await?,
            )),
        }
    }

    async fn complete_rf<O: PhyTargetObserver>(
        &self,
        binding: PhyColdExternalBinding,
        registers: &mut impl SharedPhyContext,
        observer: &mut O,
    ) -> Result<PhyRfInitPrefixCompletion, PhyTargetPortError> {
        match binding {
            PhyColdExternalBinding::I2cConfiguration(binding) => {
                complete_i2c_configuration::<D>(self.timer, binding, registers).await
            }
            PhyColdExternalBinding::I2c(mut binding) => {
                for _ in 0..HARDWARE_EDGE_LIMIT {
                    match binding.action() {
                        PhyColdI2cAction::StartRead { .. }
                        | PhyColdI2cAction::StartWrite { .. } => {
                            match binding.start_target(registers) {
                                Ok(()) => {}
                                Err(PhyColdI2cError::BusyAtStart) => {
                                    delay::<D, _>(self.timer, Kind::BusBusy, 1).await
                                }
                                Err(_) => return Err(PhyTargetPortError::UnexpectedBinding),
                            }
                        }
                        PhyColdI2cAction::AwaitReadCompletionEdge { .. }
                        | PhyColdI2cAction::AwaitWriteCompletionEdge { .. } => {
                            delay::<D, _>(self.timer, Kind::Completion, 1).await;
                            match binding
                                .observe_target_edge(registers)
                                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                            {
                                PhyColdI2cObservation::EdgeConsumed
                                | PhyColdI2cObservation::StillPending => {}
                            }
                        }
                        PhyColdI2cAction::Complete(_) => {
                            return binding
                                .into_completion()
                                .map_err(|_| PhyTargetPortError::UnexpectedBinding);
                        }
                    }
                }
                Err(PhyTargetPortError::HardwareEdgeTimedOut)
            }
            PhyColdExternalBinding::Mmio(binding) => {
                let boundary = match binding.outer_action() {
                    PhyRfInitPrefixAction::ConfigureFeBbClock => Some(PhyRfBoundary::BeforeRfInit),
                    PhyRfInitPrefixAction::ConfigureI2cClockSelection => {
                        Some(PhyRfBoundary::AfterPbusClear)
                    }
                    PhyRfInitPrefixAction::ConfigureI2cMasterRegisters => {
                        Some(PhyRfBoundary::BeforeI2cMasterRegisterInit)
                    }
                    PhyRfInitPrefixAction::ConfigurePowerDetectorRegisters => {
                        Some(PhyRfBoundary::BeforePowerDetectorRegisterInit)
                    }
                    PhyRfInitPrefixAction::ConfigureFrontEndRegisters => {
                        Some(PhyRfBoundary::BeforeFrontEndRegisterInit)
                    }
                    PhyRfInitPrefixAction::ConfigureTemperatureSensorRead => {
                        Some(PhyRfBoundary::BeforeTemperatureSensorReadInit)
                    }
                    PhyRfInitPrefixAction::ConfigureTxPowerControlBackground => {
                        Some(PhyRfBoundary::BeforeTxPowerControlBackgroundInit)
                    }
                    PhyRfInitPrefixAction::ChannelFrequency(
                        crate::analog::frequency::PhyChannelFrequencyInitAction::ConfigureFrequencyRegisters {
                            ..
                        },
                    ) => Some(PhyRfBoundary::BeforeChannelFrequencyInit),
                    _ => None,
                };
                if let Some(boundary) = boundary {
                    observer.rf_boundary(boundary);
                }
                binding
                    .execute_target(registers)
                    .map_err(|error| match error {
                        crate::calibration::cold::PhyColdLoweringError::HardwareRestoreInvariant => {
                            PhyTargetPortError::HardwareInvariant
                        }
                        _ => PhyTargetPortError::UnexpectedBinding,
                    })
            }
            PhyColdExternalBinding::Observation(binding) => {
                if binding.outer_action() == PhyRfInitPrefixAction::CaptureChannelFrequencyControl {
                    observer.rf_boundary(PhyRfBoundary::BeforeChannelFrequencyInit);
                }
                match binding.request() {
                    PhyColdObservationRequest::ObserveDcIqReadiness {
                        readiness_samples, ..
                    }
                    | PhyColdObservationRequest::ObserveSignalPowerReadiness {
                        readiness_samples,
                        ..
                    } if readiness_samples >= HARDWARE_EDGE_LIMIT => binding
                        .into_timeout_completion()
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding),
                    PhyColdObservationRequest::ObserveDcIqReadiness { .. }
                    | PhyColdObservationRequest::ObserveSignalPowerReadiness { .. } => {
                        delay::<D, _>(self.timer, Kind::Completion, 1).await;
                        binding
                            .execute_target(registers)
                            .map_err(|_| PhyTargetPortError::UnexpectedBinding)
                    }
                    _ => binding
                        .execute_target(registers)
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding),
                }
            }
            PhyColdExternalBinding::Pbus(mut binding) => {
                binding
                    .start_target(registers)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                for _ in 0..HARDWARE_EDGE_LIMIT {
                    delay::<D, _>(self.timer, Kind::Completion, 1).await;
                    match binding
                        .observe_target_edge(registers)
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                    {
                        PhyColdPbusObservation::EdgeConsumed => {
                            return binding
                                .into_completion()
                                .map_err(|_| PhyTargetPortError::UnexpectedBinding);
                        }
                        PhyColdPbusObservation::StillPending => {}
                    }
                }
                binding
                    .into_timeout_completion()
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)
            }
            PhyColdExternalBinding::Timer(binding) => {
                delay::<D, _>(self.timer, Kind::Settle, u64::from(binding.micros())).await;
                binding
                    .into_elapsed_completion()
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)
            }
        }
    }

    fn run_calibration_force_txrx(
        &self,
        mut child: PhyCalibrationForceTxRxTransition,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyCalibrationTrackingCompletion, PhyTargetPortError> {
        for _ in 0..RF_OPERATION_LIMIT {
            child = match child.commit() {
                Ok(completion) => return Ok(completion),
                Err(child) => child,
            };
            let binding = child
                .lower_external()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
            let completion = match binding {
                PhyForceTxRxExternalBinding::Mmio(binding) => binding.execute_target(registers),
                PhyForceTxRxExternalBinding::Timer(binding) => {
                    if !D::settle_micros(binding.micros()) {
                        return Err(PhyTargetPortError::HardwareCapabilityUnavailable);
                    }
                    binding.into_completion()
                }
            };
            child
                .advance(completion)
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        }
        Err(PhyTargetPortError::RfOperationLimit)
    }

    fn run_calibration_tx_dc_pwdet<O: PhyTargetObserver>(
        &self,
        mut child: PhyCalibrationTxDcPwdetTransition,
        registers: &mut impl SharedPhyContext,
        observer: &core::cell::RefCell<&mut O>,
    ) -> Result<PhyCalibrationTrackingCompletion, PhyTargetPortError> {
        calibration::tx_dc_pwdet_init::<D, _>(
            child.transition_mut(),
            registers,
            observer,
            self.timer,
        )?;
        child
            .commit()
            .map_err(|_| PhyTargetPortError::UnexpectedBinding)
    }

    async fn run_param_rfpll<O: PhyTargetObserver>(
        &self,
        mut child: PhyParamTrackingRfpllTransition<'_>,
        registers: &mut impl SharedPhyAccess,
        grant: &mut impl PhyGrantProtectPort,
        observer: &mut O,
    ) -> Result<PhyParamTrackingCompletion, PhyTargetPortError> {
        let request = child.request();
        let sample_age_micros = O::OBSERVE_RFPLL_AGE
            .then(|| self.timer.now().as_micros())
            .and_then(|now| {
                match child
                    .state()
                    .temperature_observation()
                    .freshness(now, u64::MAX)
                {
                    crate::tracking::temperature::Freshness::Fresh { age_micros } => {
                        Some(age_micros)
                    }
                    _ => None,
                }
            });
        for _ in 0..RF_OPERATION_LIMIT {
            let action = child.action();
            if let crate::tracking::rfpll::thermal::Action::Complete(outcome) = action {
                let completion = child
                    .commit()
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                observer.rfpll_completed(crate::tracking::rfpll::Observation {
                    request,
                    outcome,
                    sample_age_micros,
                });
                return Ok(completion);
            }
            let completion =
                rfpll::complete_thermal::<D>(self.timer, registers, grant, action).await?;
            if let Some((operation, event)) = crate::tracking::observation::rfpll_hold_edge(action)
            {
                observer.tracking_operation(operation, event);
            }
            child
                .advance(completion)
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        }
        Err(PhyTargetPortError::RfOperationLimit)
    }

    async fn run_param_tx_power<P>(
        &self,
        mut child: PhyParamTrackingTxPowerTransition<'_>,
        platform: &mut P,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyParamTrackingCompletion, PhyTargetPortError> {
        for _ in 0..RF_OPERATION_LIMIT {
            child = match child.commit() {
                Ok(completion) => return Ok(completion),
                Err(child) => child,
            };
            let binding = child
                .lower_external()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
            if let Some(micros) = binding.delay_micros()
                && !D::settle_micros(micros)
            {
                return Err(PhyTargetPortError::HardwareCapabilityUnavailable);
            }
            let completion = binding.execute_target(platform, registers);
            child
                .advance(completion)
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        }
        Err(PhyTargetPortError::RfOperationLimit)
    }

    async fn run_param_wifi_i2c(
        &self,
        mut child: PhyParamTrackingWifiI2cTransition<'_>,
        registers: &mut impl SharedPhyAccess,
    ) -> Result<PhyParamTrackingCompletion, PhyTargetPortError> {
        for _ in 0..RF_OPERATION_LIMIT {
            child = match child.commit() {
                Ok(completion) => return Ok(completion),
                Err(child) => child,
            };
            let binding = child
                .lower_external()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
            let completion = complete_masked_i2c(binding, registers, |kind, micros| {
                delay::<D, _>(self.timer, kind, micros)
            })
            .await?;
            child
                .advance(PhyWifiI2cTrackingCompletion::MaskedWrite(completion))
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        }
        Err(PhyTargetPortError::RfOperationLimit)
    }

    async fn run_param_temperature<P, O: PhyTargetObserver>(
        &self,
        mut child: PhyParamTrackingTemperatureTransition<'_>,
        platform: &mut P,
        registers: &mut impl SharedPhyAccess,
        observer: &mut O,
    ) -> Result<PhyParamTrackingCompletion, PhyTargetPortError> {
        let started = Some(self.timer.now().as_micros());
        for _ in 0..RF_OPERATION_LIMIT {
            if let crate::analog::temperature::PhyTemperatureAction::Complete(outcome) =
                child.action()
            {
                let completion = child
                    .commit_observed(started, Some(self.timer.now().as_micros()))
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                observer.temperature_completed(outcome);
                return Ok(completion);
            }
            let binding = child
                .lower_external()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
            let completion = self
                .complete_temperature(binding, platform, registers)
                .await?;
            child
                .advance(completion)
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
        }
        Err(PhyTargetPortError::RfOperationLimit)
    }
}

impl<'a, P, R, D, T, O> TargetPhyRegisterPort<'a, P, R, D, T, O> {
    /// Bind the complete target port to disjoint platform and PHY borrows.
    pub fn new(platform: &'a mut P, registers: &'a mut R, timer: &'a T, observer: O) -> Self {
        Self {
            platform,
            registers,
            observer,
            counters: PhyTargetPortCounters::default(),
            timer,
            short: PhantomData,
        }
    }

    /// Snapshot operation counts without releasing the radio borrow.
    pub const fn counters(&self) -> PhyTargetPortCounters {
        self.counters
    }
}

impl<'a, P, R, G, D, T, O> TargetPhyCalibrationTrackingPort<'a, P, R, G, D, T, O> {
    pub fn new(
        platform: &'a mut P,
        registers: &'a mut R,
        grant: &'a mut G,
        timer: &'a T,
        observer: &'a mut O,
    ) -> Self {
        Self {
            platform,
            registers,
            grant,
            observer,
            timer,
            short: PhantomData,
        }
    }
}

impl<'a, P, R, G, D, T, O> TargetPhyParamTrackingPort<'a, P, R, G, D, T, O> {
    pub fn new(
        platform: &'a mut P,
        registers: &'a mut R,
        grant: &'a mut G,
        timer: &'a T,
        observer: O,
    ) -> Self {
        Self {
            platform,
            registers,
            grant,
            observer,
            timer,
            short: PhantomData,
        }
    }
}

impl<
    P,
    R: PhyInitializationAccess,
    G: PhyGrantProtectPort,
    D: PhyShortDelay,
    T: Timer,
    O: PhyTargetObserver,
> PhyCalibrationTrackingPort for TargetPhyCalibrationTrackingPort<'_, P, R, G, D, T, O>
{
    type Error = PhyTargetPortError;

    fn observe(
        &mut self,
        operation: crate::tracking::observation::Operation,
        event: crate::tracking::observation::Event,
    ) {
        self.observer.tracking_operation(operation, event);
    }

    async fn complete<'port, 'state>(
        &'port mut self,
        transition: &'port mut PhyParamTrackingCalibrationTransition<'state>,
    ) -> Result<PhyCalibrationTrackingCompletion, Self::Error> {
        self.observer.operation_started();
        let held_edge = crate::tracking::observation::calibration_hold_edge(
            transition.action(),
            transition.branch(),
        );
        let completion = match transition.action() {
            PhyCalibrationTrackingAction::SetGrantProtect { enabled } => {
                self.grant.set_grant_protect(enabled)?;
                PhyCalibrationTrackingCompletion::GrantProtectSet { enabled }
            }
            PhyCalibrationTrackingAction::ClearPbus => {
                let completed = calibration::clear_pbus::<D, _>(
                    self.timer,
                    transition
                        .begin_pbus_clear()
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
                    self.registers,
                    &mut *self.observer,
                )?;
                completed
            }
            PhyCalibrationTrackingAction::CalibrateDcode => {
                use crate::tracking::observation::{Event, Operation};
                self.observer
                    .tracking_operation(Operation::Dcode, Event::PollStarted);
                let result = calibration::dcode::<D, _>(
                    self.timer,
                    transition
                        .begin_dcode()
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
                    self.platform,
                    self.registers,
                    |locked| self.observer.dcode_pll_lock(locked),
                );
                self.observer
                    .tracking_operation(Operation::Dcode, Event::PollReady);
                result?
            }
            PhyCalibrationTrackingAction::RecalibrateRxGain => {
                use crate::tracking::observation::{Event, Operation};
                let observer = core::cell::RefCell::new(&mut *self.observer);
                observer
                    .borrow_mut()
                    .tracking_operation(Operation::RxGain, Event::PollStarted);
                let result = calibration::rx_gain::<D, _>(
                    self.timer,
                    transition
                        .begin_rx_gain_recalibration()
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
                    self.platform,
                    self.registers,
                    |operation, event| {
                        if operation.is_rx_gain_region() {
                            observer.borrow_mut().tracking_operation(operation, event);
                        }
                    },
                    |execution| observer.borrow_mut().rx_gain_execution(execution),
                );
                observer
                    .borrow_mut()
                    .tracking_operation(Operation::RxGain, Event::PollReady);
                result?
            }
            PhyCalibrationTrackingAction::RestoreChipChannel { .. } => {
                calibration::channel::<D, _, _>(
                    self.timer,
                    transition
                        .begin_channel_restore()
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
                    self.platform,
                    self.registers,
                    &mut *self.observer,
                )?
            }
            PhyCalibrationTrackingAction::ForceTxRxOff { .. } => {
                TargetCompleter::<D, T>::new(self.timer).run_calibration_force_txrx(
                    transition
                        .begin_force_txrx()
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
                    self.registers,
                )?
            }
            PhyCalibrationTrackingAction::CalibrateTxDcPwdet { .. } => {
                use crate::tracking::observation::{Event, Operation};
                let observer = core::cell::RefCell::new(&mut *self.observer);
                observer
                    .borrow_mut()
                    .tracking_operation(Operation::TxDcPwdet, Event::PollStarted);
                let result = TargetCompleter::<D, T>::new(self.timer).run_calibration_tx_dc_pwdet(
                    transition
                        .begin_tx_dc_pwdet()
                        .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
                    self.registers,
                    &observer,
                );
                observer
                    .borrow_mut()
                    .tracking_operation(Operation::TxDcPwdet, Event::PollReady);
                result?
            }
            PhyCalibrationTrackingAction::PublishWifiTxGain { .. }
            | PhyCalibrationTrackingAction::PublishBluetoothIeee802154TxGain => transition
                .begin_tx_gain_publication()
                .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                .execute_target(self.registers),
            PhyCalibrationTrackingAction::AwaitSoftwareFrequencySettle => {
                if !D::settle_micros(2) {
                    return Err(PhyTargetPortError::HardwareCapabilityUnavailable);
                }
                oer_esp32s31_hal::phy::frequency::observe_software_frequency_boundary(
                    self.registers,
                );
                crate::tracking::calibration::PhyCalibrationTrackingCompletion::SoftwareFrequencySettled
            }
            PhyCalibrationTrackingAction::SetForcedDigitalGain { .. }
            | PhyCalibrationTrackingAction::SetHardwareFrequencyControl { .. }
            | PhyCalibrationTrackingAction::ConfigureBasebandChannel { .. }
            | PhyCalibrationTrackingAction::DisableWifiBaseband
            | PhyCalibrationTrackingAction::EnableMacBaseband
            | PhyCalibrationTrackingAction::RestoreTxGainCompensation => {
                match transition
                    .lower_external()
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
                {
                    PhyCalibrationTrackingExternalBinding::Register(binding) => {
                        binding.execute_target(self.registers)
                    }
                    PhyCalibrationTrackingExternalBinding::MacBaseband(binding) => {
                        binding.execute_target(self.registers)
                    }
                }
            }
            PhyCalibrationTrackingAction::Complete(_) | PhyCalibrationTrackingAction::Failed(_) => {
                return Err(PhyTargetPortError::UnexpectedBinding);
            }
        };
        if let Some((operation, event)) = held_edge {
            self.observer.tracking_operation(operation, event);
        }
        self.observer.operation_completed();
        Ok(completion)
    }
}

impl<
    P,
    R: PhyInitializationAccess,
    G: PhyGrantProtectPort,
    D: PhyShortDelay,
    T: Timer,
    O: PhyTargetObserver,
> PhyParamTrackingPort for TargetPhyParamTrackingPort<'_, P, R, G, D, T, O>
{
    type Error = PhyTargetPortError;

    fn observe(
        &mut self,
        operation: crate::tracking::observation::Operation,
        event: crate::tracking::observation::Event,
    ) {
        self.observer.tracking_operation(operation, event);
    }

    async fn complete<'port>(
        &'port mut self,
        pending: &'port mut PhyPendingTracking,
        state: &'port mut PhyState,
    ) -> Result<PhyParamTrackingCompletion, Self::Error> {
        self.observer.operation_started();
        let completion = match pending.action() {
            PhyParamTrackingAction::EnterCritical => PhyParamTrackingCompletion::EnteredCritical,
            PhyParamTrackingAction::ExitCritical => PhyParamTrackingCompletion::ExitedCritical,
            PhyParamTrackingAction::RfpllCapTrack { .. } => {
                TargetCompleter::<D, T>::new(self.timer)
                    .run_param_rfpll(
                        pending
                            .begin_rfpll_cap_tracking(state)
                            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
                        self.registers,
                        &mut *self.grant,
                        &mut self.observer,
                    )
                    .await?
            }
            PhyParamTrackingAction::BluetoothIeee802154TxPowerTrack { .. }
            | PhyParamTrackingAction::WifiTxPowerTrack { .. } => {
                TargetCompleter::<D, T>::new(self.timer)
                    .run_param_tx_power(
                        pending
                            .begin_tx_power_tracking(state)
                            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
                        self.platform,
                        self.registers,
                    )
                    .await?
            }
            PhyParamTrackingAction::CalibrationTrack { .. } => {
                let mut child = pending
                    .begin_calibration_tracking(state)
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                let mut port = TargetPhyCalibrationTrackingPort::<_, _, _, D, _, _>::new(
                    self.platform,
                    self.registers,
                    &mut *self.grant,
                    self.timer,
                    &mut self.observer,
                );
                run_phy_calibration_tracking(&mut child, &mut port)
                    .await
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?;
                child
                    .commit()
                    .map_err(|_| PhyTargetPortError::UnexpectedBinding)?
            }
            PhyParamTrackingAction::WifiI2cTrack => {
                TargetCompleter::<D, T>::new(self.timer)
                    .run_param_wifi_i2c(
                        pending
                            .begin_wifi_i2c_tracking(state)
                            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
                        self.registers,
                    )
                    .await?
            }
            PhyParamTrackingAction::TemperatureRead => {
                TargetCompleter::<D, T>::new(self.timer)
                    .run_param_temperature(
                        pending
                            .begin_temperature_read(state)
                            .map_err(|_| PhyTargetPortError::UnexpectedBinding)?,
                        self.platform,
                        self.registers,
                        &mut self.observer,
                    )
                    .await?
            }
            PhyParamTrackingAction::Complete(_) => {
                return Err(PhyTargetPortError::UnexpectedBinding);
            }
        };
        self.observer.operation_completed();
        Ok(completion)
    }
}

impl<P, R: PhyInitializationAccess, D: PhyShortDelay, T: Timer, O: PhyTargetObserver>
    PhyRegisterPort for TargetPhyRegisterPort<'_, P, R, D, T, O>
{
    type Error = PhyTargetPortError;

    async fn complete(
        &mut self,
        binding: PhyRegisterExternalBinding,
    ) -> Result<PhyRegisterCompletion, Self::Error> {
        self.observer.operation_started();
        let result = match binding {
            PhyRegisterExternalBinding::Mmio(binding) => {
                self.counters.mmio += 1;
                Ok(PhyRegisterCompletion::Mmio(
                    binding.execute_target(self.platform, self.registers),
                ))
            }
            PhyRegisterExternalBinding::Timer(binding) => {
                delay::<D, _>(self.timer, Kind::Settle, u64::from(binding.micros())).await;
                self.counters.delays += 1;
                Ok(binding.into_completion())
            }
            PhyRegisterExternalBinding::ResetSample(binding) => {
                self.counters.reset_samples += 1;
                Ok(binding.execute_target(self.registers))
            }
            PhyRegisterExternalBinding::Rf(binding) => {
                if self.counters.rf_operations >= RF_OPERATION_LIMIT {
                    Err(PhyTargetPortError::RfOperationLimit)
                } else {
                    let completion = TargetCompleter::<D, T>::new(self.timer)
                        .complete_rf(binding, self.registers, &mut self.observer)
                        .await?;
                    self.counters.rf_operations += 1;
                    Ok(PhyRegisterCompletion::Rf(completion))
                }
            }
            PhyRegisterExternalBinding::Baseband(binding) => {
                let completion = TargetCompleter::<D, T>::new(self.timer)
                    .complete_baseband(binding, self.platform, self.registers, &mut self.observer)
                    .await?;
                self.counters.baseband_operations += 1;
                Ok(PhyRegisterCompletion::Baseband(completion))
            }
            PhyRegisterExternalBinding::Temperature(binding) => {
                Ok(PhyRegisterCompletion::Temperature(
                    TargetCompleter::<D, T>::new(self.timer)
                        .complete_temperature(binding, self.platform, self.registers)
                        .await?,
                ))
            }
            PhyRegisterExternalBinding::FinalI2c(binding) => {
                complete_final_i2c::<D>(self.timer, binding, self.registers).await
            }
        };
        if result.is_ok() {
            self.observer.operation_completed();
        }
        result
    }
}

/// Select a PHY channel with the same finite target contract used by cold
/// registration, through a temporary borrow of the complete HAL owner.
///
/// The raw state is not a registration proof, so this entry stays inside the
/// crate; callers select channels through the concurrent domain.
pub(crate) async fn select_phy_channel_with_hal<D: PhyShortDelay, P, O: PhyTargetObserver>(
    timer: &impl Timer,
    state: &mut PhyState,
    channel_or_frequency: u16,
    cbw: u8,
    channel: &mut oer_esp32s31_hal::ieee80211::channel::RadioChannelHal<'_, P>,
    observer: &mut O,
) -> Result<(), PhyTargetPortError> {
    TargetCompleter::<D, _>::new(timer)
        .select_channel_hal(state, channel_or_frequency, cbw, channel, observer)
        .await
}

/// Stop, retune and restart through a temporary borrow of the HAL owner.
pub(crate) async fn switch_phy_channel_with_hal_and_mac_restart<
    D: PhyShortDelay,
    P,
    O: PhyTargetObserver,
>(
    timer: &impl Timer,
    state: &mut PhyState,
    channel_or_frequency: u16,
    cbw: u8,
    channel: &mut oer_esp32s31_hal::ieee80211::channel::RadioChannelHal<'_, P>,
    observer: &mut O,
) -> Result<(), PhyTargetPortError> {
    TargetCompleter::<D, _>::new(timer)
        .switch_channel_hal_with_mac_restart(state, channel_or_frequency, cbw, channel, observer)
        .await
}

#[cfg(oer_probe)]
pub mod temperature;

#[cfg(oer_probe)]
pub(crate) use radio_lifecycle::reset_wake_i2c_master;
