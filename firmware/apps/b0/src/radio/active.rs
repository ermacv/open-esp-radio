use super::*;
pub(super) type Running<'a, 'loan: 'a, 'p: 'loan, 'e: 'loan> =
    impl core::future::Future<Output = Result<(), Fault>> + 'a;
static RUNNING: static_cell::ConstStaticCell<
    oer_memory::StaticPlace<Running<'static, 'static, 'static, 'static>>,
> = static_cell::ConstStaticCell::new(oer_memory::StaticPlace::new());
#[define_opaque(Running)]
fn running<'a, 'loan: 'a, 'p: 'loan, 'e: 'loan>(
    actor: &'a mut LiveActor<'loan, 'p, 'e>,
    runner: &'a mut LiveRunner<'p, 'e>,
    timer_task: &'a mut ControllerModemTimerTask<'static, irq::TimerPort, 1, X>,
) -> Running<'a, 'loan, 'p, 'e> {
    actor.run_until(async {
        let operations = async {
            let started = runner
                .start(Start::Receive(Receive {
                    channel: TestChannel::new(19).expect("DTM test channel"),
                    phy: TestPhy::Le1M,
                }))
                .await?;
            if started != oer_esp32s31_bluetooth_driver::port::Started::Executed {
                return Err(Fault::Hardware);
            }
            runner.end().await?;
            Ok::<_, Fault>(())
        };
        match select(maintenance(timer_task), operations).await {
            Either::First(result) => result,
            Either::Second(result) => result,
        }
    })
}

pub(super) fn run<'a, 'loan: 'a, 'p: 'loan, 'e: 'loan>(
    actor: &'a mut LiveActor<'loan, 'p, 'e>,
    runner: &'a mut LiveRunner<'p, 'e>,
    timer_task: &'a mut ControllerModemTimerTask<'static, irq::TimerPort, 1, X>,
) -> oer_memory::PlacedFuture<'a, Running<'a, 'loan, 'p, 'e>> {
    RUNNING.take().future(|| running(actor, runner, timer_task))
}
pub(super) fn timer_step(
    task: &mut ControllerModemTimerTask<'_, irq::TimerPort, 1, X>,
) -> Result<(), Fault> {
    if irq::FAULT.load(core::sync::atomic::Ordering::Acquire) {
        return Err(Fault::Hardware);
    }
    match task.readiness().class() {
        ControllerModemTimerReadinessClass::Interrupt => match task.begin() {
            ControllerModemTimerBegin::Started
            | ControllerModemTimerBegin::NotReady
            | ControllerModemTimerBegin::AlreadyActive => Ok(()),
            _ => Err(Fault::Hardware),
        },
        ControllerModemTimerReadinessClass::Step => match task.step() {
            ControllerModemTimerStep::ExpirationPending(_)
            | ControllerModemTimerStep::Backpressured(_) => Err(Fault::Hardware),
            _ => Ok(()),
        },
        ControllerModemTimerReadinessClass::EventCapacity => Err(Fault::Hardware),
        ControllerModemTimerReadinessClass::Rearm => match task.rearm() {
            ControllerModemTimerRearm::Rearmed | ControllerModemTimerRearm::NotReady => Ok(()),
            _ => Err(Fault::Hardware),
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
                .map_err(|_| Fault::Hardware)?;
        }
        timer_step(task)?;
        let _ = EmbassyClock.wait_for(Duration::from_micros(10)).await;
    }
}
