use core::{
    future::{Future, ready},
    task::{Context, Poll, Waker},
};

use super::{
    PhyCalibrationTrackingPort, PhyParamTrackingPort, PhyRegisterPort, PhyRegisterRunError,
    run_phy_calibration_tracking, run_phy_param_tracking, run_phy_register,
};
use crate::calibration::registration::{
    PhyRegisterAction, PhyRegisterCompletion, PhyRegisterExternalBinding,
    PhyRegisterMmioCompletion, PhyRegisterTransition,
};

fn run_ready<F: Future>(future: F) -> F::Output {
    let mut context = Context::from_waker(Waker::noop());
    let mut future = std::pin::pin!(future);
    match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("test port unexpectedly waited"),
    }
}

struct StopAfterFirstMmio {
    calls: u8,
}

impl PhyRegisterPort for StopAfterFirstMmio {
    type Error = u8;

    fn complete(
        &mut self,
        binding: PhyRegisterExternalBinding,
    ) -> impl Future<Output = Result<PhyRegisterCompletion, Self::Error>> + '_ {
        self.calls += 1;
        if self.calls != 1 {
            return ready(Err(self.calls));
        }
        let PhyRegisterExternalBinding::Mmio(binding) = binding else {
            return ready(Err(0xff));
        };
        ready(Ok(PhyRegisterCompletion::Mmio(PhyRegisterMmioCompletion {
            action: binding.action(),
        })))
    }
}

#[test]
fn executor_awaits_only_lowered_identity_bound_operations() {
    let mut transition = PhyRegisterTransition::with_production_config();
    let mut port = StopAfterFirstMmio { calls: 0 };

    assert_eq!(
        run_ready(run_phy_register(&mut transition, &mut port)),
        Err(PhyRegisterRunError::Port(2))
    );
    assert_eq!(port.calls, 2);
    assert!(transition.state().is_some());
    assert!(matches!(
        transition.step_local(),
        Ok(
            crate::calibration::registration::PhyRegisterLocalStep::External(
                PhyRegisterAction::Mmio(_)
            )
        )
    ));
}

struct RestoreOnlyCalibrationPort {
    calls: u8,
}

impl PhyCalibrationTrackingPort for RestoreOnlyCalibrationPort {
    type Error = ();

    fn complete<'port, 'state>(
        &'port mut self,
        transition: &'port mut crate::tracking::parameters::PhyParamTrackingCalibrationTransition<
            'state,
        >,
    ) -> impl Future<
        Output = Result<
            crate::tracking::calibration::PhyCalibrationTrackingCompletion,
            Self::Error,
        >,
    > + 'port {
        self.calls += 1;
        ready(match transition.action() {
                crate::tracking::calibration::PhyCalibrationTrackingAction::RestoreTxGainCompensation => {
                    Ok(crate::tracking::calibration::PhyCalibrationTrackingCompletion::TxGainCompensationRestored)
                }
                _ => Err(()),
            })
    }
}

#[test]
fn skipped_calibration_executor_never_calls_the_hardware_port() {
    let policy = crate::tracking::parameters::PhyParamTrackingPolicy {
        tracking_inhibited: false,
        rfpll_cap_tracking_enabled: false,
        rfpll_cap_tracking_threshold: None,
        calibration_tracking_threshold: None,
        diagnostics: crate::tracking::parameters::PhyTrackingDiagnostics::Enabled,
        bluetooth_ieee802154_power_tracking_enabled: true,
        calibration_tracking_enabled: true,
        relaxed_power_tracking_threshold: false,
    };
    let mut outer = crate::tracking::parameters::PhyParamTrackingTransition::new(
        crate::tracking::parameters::PhyParamTrackRequest::new(false, true),
        policy,
    );
    outer
        .advance(crate::tracking::parameters::PhyParamTrackingCompletion::EnteredCritical)
        .unwrap();
    outer
            .advance(
                crate::tracking::parameters::PhyParamTrackingCompletion::BluetoothIeee802154TxPowerTracked {
                    enabled: true,
                    gain_updated: false,
                },
            )
            .unwrap();

    let mut state = crate::PhyState::new(crate::PhyConfig::production());
    let mut child = outer.begin_calibration_tracking(&mut state).unwrap();
    let mut port = RestoreOnlyCalibrationPort { calls: 0 };
    assert_eq!(
        run_ready(run_phy_calibration_tracking(&mut child, &mut port)),
        Ok(())
    );
    assert_eq!(port.calls, 0);
    assert!(matches!(
        child.action(),
        crate::tracking::calibration::PhyCalibrationTrackingAction::Complete(_)
    ));
    let completion = child.commit().unwrap();
    outer.advance(completion).unwrap();
    assert_eq!(
        outer.action(),
        crate::tracking::parameters::PhyParamTrackingAction::TemperatureRead
    );
}

struct CriticalOnlyTrackingPort {
    calls: u8,
}

impl PhyParamTrackingPort for CriticalOnlyTrackingPort {
    type Error = ();

    fn complete<'port>(
        &'port mut self,
        pending: &'port mut crate::state::client::PhyPendingTracking,
        _state: &'port mut crate::PhyState,
    ) -> impl Future<
        Output = Result<crate::tracking::parameters::PhyParamTrackingCompletion, Self::Error>,
    > + 'port {
        self.calls += 1;
        ready(match pending.action() {
            crate::tracking::parameters::PhyParamTrackingAction::EnterCritical => {
                Ok(crate::tracking::parameters::PhyParamTrackingCompletion::EnteredCritical)
            }
            crate::tracking::parameters::PhyParamTrackingAction::ExitCritical => {
                Ok(crate::tracking::parameters::PhyParamTrackingCompletion::ExitedCritical)
            }
            _ => Err(()),
        })
    }
}

#[test]
fn outer_executor_holds_affine_client_owner_across_software_critical_section() {
    let request = crate::tracking::parameters::PhyParamTrackRequest::new(true, false);
    let policy = crate::tracking::parameters::PhyParamTrackingPolicy {
        tracking_inhibited: true,
        rfpll_cap_tracking_enabled: true,
        rfpll_cap_tracking_threshold: None,
        calibration_tracking_threshold: None,
        diagnostics: crate::tracking::parameters::PhyTrackingDiagnostics::Enabled,
        bluetooth_ieee802154_power_tracking_enabled: true,
        calibration_tracking_enabled: true,
        relaxed_power_tracking_threshold: false,
    };
    let mut pending = crate::state::client::PhyPendingTracking::for_test(request, policy);
    let mut state = crate::PhyState::new(crate::PhyConfig::production());
    let mut port = CriticalOnlyTrackingPort { calls: 0 };
    let outcome = run_ready(run_phy_param_tracking(&mut pending, &mut state, &mut port)).unwrap();
    assert_eq!(outcome.clients, request);
    assert!(outcome.tracking_inhibited);
    assert_eq!(port.calls, 2);
    assert!(pending.into_owner().is_ok());
}

struct FailingTrackingPort;

struct WaitingTrackingPort;

impl PhyParamTrackingPort for WaitingTrackingPort {
    type Error = ();

    async fn complete<'port>(
        &'port mut self,
        pending: &'port mut crate::state::client::PhyPendingTracking,
        _state: &'port mut crate::PhyState,
    ) -> Result<crate::tracking::parameters::PhyParamTrackingCompletion, Self::Error> {
        use crate::tracking::parameters::{PhyParamTrackingAction, PhyParamTrackingCompletion};
        if pending.action() == PhyParamTrackingAction::EnterCritical {
            Ok(PhyParamTrackingCompletion::EnteredCritical)
        } else {
            core::future::pending().await
        }
    }
}

#[test]
fn cancelling_tracking_while_waiting_cannot_release_an_ordinary_client_owner() {
    let request = crate::tracking::parameters::PhyParamTrackRequest::new(false, true);
    let policy = crate::tracking::parameters::PhyParamTrackingPolicy {
        tracking_inhibited: false,
        rfpll_cap_tracking_enabled: false,
        rfpll_cap_tracking_threshold: None,
        calibration_tracking_threshold: None,
        diagnostics: crate::tracking::parameters::PhyTrackingDiagnostics::Enabled,
        bluetooth_ieee802154_power_tracking_enabled: true,
        calibration_tracking_enabled: true,
        relaxed_power_tracking_threshold: false,
    };
    let mut pending = crate::state::client::PhyPendingTracking::for_test(request, policy);
    let mut state = crate::PhyState::new(crate::PhyConfig::production());
    let mut port = WaitingTrackingPort;
    {
        let mut future =
            std::pin::pin!(run_phy_param_tracking(&mut pending, &mut state, &mut port,));
        let mut context = Context::from_waker(Waker::noop());
        assert!(future.as_mut().poll(&mut context).is_pending());
        // Dropping the suspended executor must not manufacture completion.
    }
    assert!(matches!(
        pending.action(),
        crate::tracking::parameters::PhyParamTrackingAction::BluetoothIeee802154TxPowerTrack { .. }
    ));
    let pending = match pending.into_owner() {
        Ok(_) => panic!("cancelled tracking released the client owner"),
        Err(pending) => pending,
    };
    assert_eq!(pending.fail().request(), &request);
}

impl PhyParamTrackingPort for FailingTrackingPort {
    type Error = u8;

    fn complete<'port>(
        &'port mut self,
        _pending: &'port mut crate::state::client::PhyPendingTracking,
        _state: &'port mut crate::PhyState,
    ) -> impl Future<
        Output = Result<crate::tracking::parameters::PhyParamTrackingCompletion, Self::Error>,
    > + 'port {
        ready(Err(9))
    }
}

#[test]
fn outer_executor_error_preserves_pending_owner_for_explicit_poisoning() {
    let request = crate::tracking::parameters::PhyParamTrackRequest::new(false, true);
    let policy = crate::tracking::parameters::PhyParamTrackingPolicy {
        tracking_inhibited: false,
        rfpll_cap_tracking_enabled: false,
        rfpll_cap_tracking_threshold: None,
        calibration_tracking_threshold: None,
        diagnostics: crate::tracking::parameters::PhyTrackingDiagnostics::Enabled,
        bluetooth_ieee802154_power_tracking_enabled: true,
        calibration_tracking_enabled: true,
        relaxed_power_tracking_threshold: false,
    };
    let mut pending = crate::state::client::PhyPendingTracking::for_test(request, policy);
    let mut state = crate::PhyState::new(crate::PhyConfig::production());
    assert_eq!(
        run_ready(run_phy_param_tracking(
            &mut pending,
            &mut state,
            &mut FailingTrackingPort,
        )),
        Err(super::PhyParamTrackingRunError::Port(9))
    );
    assert_eq!(
        pending.action(),
        crate::tracking::parameters::PhyParamTrackingAction::EnterCritical
    );
    assert_eq!(pending.fail().request(), &request);
}
