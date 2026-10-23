//! One T3 family future owns the semantic commit and its P1 timing.
use super::{Failure, Job, Registers, drive};
use crate::tracking::{
    i2c::{PhyWifiI2cTrackingAction, PhyWifiI2cTrackingCompletion},
    observation::EngineTiming,
    parameters::{PhyParamTrackingCompletion, PhyParamTrackingWifiI2cTransition},
};
use oer_cooperate::CooperativeTimer as Timer;

pub(crate) async fn run_param_wifi_i2c<
    D: crate::executor::wait::PhyShortDelay,
    R: Registers,
    T: Timer,
>(
    timer: &T,
    mut child: PhyParamTrackingWifiI2cTransition<'_>,
    registers: &mut R,
    measured: bool,
) -> (Result<PhyParamTrackingCompletion, Failure>, EngineTiming) {
    let mut timing = EngineTiming::default();
    let result = loop {
        if let PhyWifiI2cTrackingAction::Complete(_) = child.action() {
            break child.commit().map_err(|_| Failure::Binding);
        }
        let mut binding = match child.lower_external() {
            Ok(binding) => binding,
            Err(_) => break Err(Failure::Binding),
        };
        {
            let mut job = match Job::counted_i2c(binding.transaction_mut(), timer.now()) {
                Ok(job) => job,
                Err(error) => break Err(error),
            };
            if let Err(error) = drive::<D, _, _>(
                &mut job,
                registers,
                timer,
                measured.then_some(&mut timing),
                None,
                None,
            )
            .await
            {
                break Err(error);
            }
        }
        let completion = match binding.into_completion() {
            Ok(completion) => completion,
            Err(_) => break Err(Failure::Binding),
        };
        if child
            .advance(PhyWifiI2cTrackingCompletion::MaskedWrite(completion))
            .is_err()
        {
            break Err(Failure::Binding);
        }
    };
    (result, timing)
}
