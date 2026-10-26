use super::*;
pub(super) async fn run<'a, 'loan: 'a, 'p: 'loan, 'e: 'loan>(
    actor: &'a mut LiveActor<'loan, 'p, 'e>,
    runner: &'a mut LiveRunner<'p, 'e>,
    timer_task: &'a mut ControllerModemTimerTask<'static, irq::TimerPort, 1, X>,
    epoch: u64,
    reply: &'a mut Option<(rpc::Command, bridge::Response)>,
) -> Result<(), Fault> {
    let mut operation = core::pin::pin!(async {
        let operations = async {
            let clock = bridge::RuntimeClock::new(epoch);
            loop {
                match select(bridge::command(), poll_fn(|cx| clock.poll_request(cx))).await {
                    Either::Second(request) => {
                        let result = runner.now().await;
                        bridge::RuntimeClock::answer(request, result);
                        result?;
                    }
                    Either::First((command, response)) => {
                        *reply = Some((command, response));
                        if command.epoch != epoch {
                            return Err(Fault::Accounting);
                        }
                        let result = match command.operation {
                            rpc::Operation::Enable => Err(Fault::Accounting),
                            rpc::Operation::Start(config) => {
                                runner.start(config).await.map(rpc::Completion::Started)
                            }
                            rpc::Operation::End => runner.end().await.map(rpc::Completion::Ended),
                            rpc::Operation::Progress => {
                                runner.progress().await.map(|()| rpc::Completion::Done)
                            }
                            rpc::Operation::Shutdown => {
                                // The frontend loan ended before this request.
                                // Settle its last cancelled clock call first.
                                if let Some(request) = clock.try_request() {
                                    let result = runner.now().await;
                                    bridge::RuntimeClock::answer(request, result);
                                    result?;
                                }
                                runner.end().await?;
                                // Shutdown is answered only after the enclosing
                                // fresh pool/epoch scope returns the actual arena.
                                return Ok(());
                            }
                        };
                        bridge::complete(reply, result);
                        result?;
                    }
                }
            }
        };
        match select(maintenance(timer_task), operations).await {
            Either::First(result) => result,
            Either::Second(result) => result,
        }
    });
    actor.run_until(operation.as_mut()).await
}

pub(super) fn timer_step(
    task: &mut ControllerModemTimerTask<'_, irq::TimerPort, 1, X>,
) -> Result<(), Fault> {
    if irq::FAULT.load(core::sync::atomic::Ordering::Acquire) {
        return Err(Fault::TimerInterrupt);
    }
    match task.readiness().class() {
        ControllerModemTimerReadinessClass::Interrupt => match task.begin() {
            ControllerModemTimerBegin::Started
            | ControllerModemTimerBegin::NotReady
            | ControllerModemTimerBegin::AlreadyActive => Ok(()),
            _ => Err(Fault::TimerBegin),
        },
        ControllerModemTimerReadinessClass::Step => match task.step() {
            ControllerModemTimerStep::ExpirationPending(_)
            | ControllerModemTimerStep::Backpressured(_) => Err(Fault::TimerStep),
            _ => Ok(()),
        },
        ControllerModemTimerReadinessClass::EventCapacity => Err(Fault::TimerStep),
        ControllerModemTimerReadinessClass::Plan => match task.submit_plan() {
            ControllerModemTimerPlanSubmission::Submitted | ControllerModemTimerPlanSubmission::NotReady => Ok(()),
            _ => Err(Fault::TimerRearm),
        },
    }
}
async fn maintenance(
    task: &mut ControllerModemTimerTask<'_, irq::TimerPort, 1, X>,
) -> Result<(), Fault> {
    loop {
        if matches!(
            task.readiness().class(),
            ControllerModemTimerReadinessClass::Interrupt
        ) {
            poll_fn(|cx| irq::TIMER_SIGNAL.poll_pending(cx))
                .await
                .map_err(|_| Fault::TimerInterrupt)?;
        }
        timer_step(task)?;
        let _ = EmbassyClock.wait_for(Duration::from_micros(10)).await;
    }
}
