use super::*;
use crate::tracking::{
    observation::{Event, Operation},
    parameters::{
        PhyParamTrackRequest, PhyParamTrackingAction, PhyParamTrackingCompletion,
        PhyParamTrackingPolicy, PhyTrackingDiagnostics,
    },
};
use core::task::{Context, Poll, Waker};

struct Port {
    wait: bool,
    events: std::vec::Vec<(Operation, Event)>,
}

impl PhyParamTrackingPort for Port {
    type Error = ();

    fn observe(&mut self, operation: Operation, event: Event) {
        self.events.push((operation, event));
    }

    async fn complete<'port>(
        &'port mut self,
        pending: &'port mut crate::state::client::PhyPendingTracking,
        _: &'port mut crate::PhyState,
    ) -> Result<PhyParamTrackingCompletion, ()> {
        if pending.action() == PhyParamTrackingAction::EnterCritical {
            return Ok(PhyParamTrackingCompletion::EnteredCritical);
        }
        if self.wait {
            core::future::pending().await
        } else {
            // A port returning successfully with a foreign completion must
            // still be reported as failure by the executor.
            Ok(PhyParamTrackingCompletion::TemperatureRead)
        }
    }
}

#[test]
fn executor_observes_rejected_completions_and_preserves_cancelled_attempts() {
    for wait in [false, true] {
        let request = PhyParamTrackRequest::new(false, true);
        let policy = PhyParamTrackingPolicy {
            tracking_inhibited: false,
            rfpll_cap_tracking_enabled: false,
            rfpll_cap_tracking_threshold: None,
            calibration_tracking_threshold: None,
            diagnostics: PhyTrackingDiagnostics::Disabled,
            bluetooth_ieee802154_power_tracking_enabled: true,
            calibration_tracking_enabled: false,
            relaxed_power_tracking_threshold: false,
        };
        let mut pending = crate::state::client::PhyPendingTracking::for_test(request, policy);
        let mut state = crate::PhyState::new(crate::PhyConfig::production());
        let mut port = Port {
            wait,
            events: std::vec::Vec::new(),
        };
        {
            let mut future =
                std::pin::pin!(run_phy_param_tracking(&mut pending, &mut state, &mut port));
            let mut cx = Context::from_waker(Waker::noop());
            if wait {
                assert!(future.as_mut().poll(&mut cx).is_pending());
            } else {
                assert!(matches!(
                    future.as_mut().poll(&mut cx),
                    Poll::Ready(Err(PhyParamTrackingRunError::Transition(_)))
                ));
            }
        }
        let operation = Operation::BluetoothIeee802154Power;
        let expected = if wait {
            std::vec![
                (Operation::Tracking, Event::Started),
                (operation, Event::Started)
            ]
        } else {
            std::vec![
                (Operation::Tracking, Event::Started),
                (operation, Event::Started),
                (operation, Event::Failed),
                (Operation::Tracking, Event::Failed)
            ]
        };
        assert_eq!(port.events, expected);
        assert!(pending.into_owner().is_err());
    }
}
