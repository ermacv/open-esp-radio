use super::*;
pub(super) type Installed<'a, 'p: 'a, 'e: 'a> = impl core::future::Future<
        Output = oer_esp32s31_hal::bluetooth::BluetoothModemLpTimerPendingObservation,
    > + 'a;
#[define_opaque(Installed)]
pub(super) fn run<'a, 'p: 'a, 'e: 'a>(
    boot: &'a mut Boot,
    pool: PoolScope<'p>,
    epoch: EpochScope<'e>,
    timer: &'static irq::TimerPort,
) -> Installed<'a, 'p, 'e> {
    async move {
        let common = boot.actor.as_mut().expect("one actor handover");
        let inbox = boot.inbox.take().expect("one BLE inbox");
        let cold = boot.cold.take().expect("one cold client");
        let coex = boot.coex.take().expect("one coex client");
        let cell = boot.cell.take().expect("one membership");
        let bell = SchedulerAccess(boot.bell);
        let release = SchedulerAccess(boot.bell);
        let Some(gate) = boot.gate.take() else {
            fail(&mut boot.arena, &mut boot.reply).await
        };
        let profile = boot.profile;
        let address = boot.address;
        let arena = boot.arena.take().expect("returned static arena");
        let port_epoch = boot.port_epoch;
        let controller = lifecycle::new(arena, pool, epoch, gate, address);
        let mut attachment = BluetoothAttachment::new(cold, coex, controller);
        let mut actor = match common.attach_bluetooth(inbox, &mut attachment) {
            Ok(actor) => actor,
            Err(mut inbox) => fail(&mut inbox, &mut boot.reply).await,
        };
        let (permission, power) = permission(profile);
        let join = BluetoothJoin::new(permission, Operation::DtmTx, Some(power))
            .expect("build-validated DTM transmit permission");
        let (cell, joined) = core::pin::pin!(actor.run_until(cell.join(join))).await;
        let joined = match joined {
            Ok(j) => j,
            Err(_) => fail(&mut actor, &mut boot.reply).await,
        };
        let (cell, tracked) = core::pin::pin!(
            actor.run_until(cell.track(joined.member, joined.payload.installation))
        )
        .await;
        let tracked = match tracked {
            Ok(t) => t,
            Err(_) => fail(&mut actor, &mut boot.reply).await,
        };
        let runtime = match actor
            .attachment_mut()
            .controller_mut()
            .take_runtime(tracked.payload.0)
        {
            Ok(r) => r,
            Err(_) => fail(&mut actor, &mut boot.reply).await,
        };
        let oer_esp32s31_bluetooth_driver::ble_phy::BlePhyRuntime {
            endpoints:
                oer_esp32s31_bluetooth_driver::low_power::ControllerRuntimeEndpoints {
                    interrupt,
                    mut task,
                    modem_timer: modem,
                },
            interrupts,
            timer: initialized,
            timing,
        } = runtime;
        let scheduler = match task
            .task_owner()
            .engine()
            .install_scheduler_interrupts(&irq::CHANNEL)
        {
            Ok(s) => s,
            Err(_) => fail(&mut task, &mut boot.reply).await,
        };
        if let Err(mut refused) = actor
            .attachment_mut()
            .controller_mut()
            .gate_mut()
            .open(interrupts, scheduler, initialized)
            .await
        {
            fail(&mut refused, &mut boot.reply).await;
        }
        let mut timer_task = ControllerModemTimerTask::new(timer, modem);
        let mut runner = Runner::new(
            task,
            timing,
            bell,
            EmbassyClock,
            interrupt.scheduler_wake(),
            permission,
            bridge::RuntimeClock::new(port_epoch),
            Limits::new(
                Duration::from_micros(10),
                Duration::from_millis(1),
                Duration::from_millis(100),
            )
            .expect("finite bring-up containment"),
            &irq::FAULT,
        );
        bridge::complete(&mut boot.reply, Ok(rpc::Completion::Done));
        let result = {
            let mut operation = core::pin::pin!(active::run(
                &mut actor,
                &mut runner,
                &mut timer_task,
                port_epoch,
                &mut boot.reply
            ));
            operation.as_mut().await
        };
        if let Err(cause) = result {
            fault(&mut runner, &mut boot.reply, cause).await;
        }
        let deadline = EmbassyClock
            .now()
            .checked_add(Duration::from_millis(100))
            .expect("finite timer shutdown bound");
        while !timer_task.retirement_ready() {
            if EmbassyClock.now() >= deadline || active::timer_step(&mut timer_task).is_err() {
                fail(&mut timer_task, &mut boot.reply).await;
            }
            let _ = EmbassyClock.wait_for(Duration::from_micros(10)).await;
        }
        // The timer's owner stays with its handler; the task's queue ends
        // with the epoch.
        let retired_timer = match timer_task.try_retire() {
            Ok(t) => t,
            Err((_, mut t)) => fail(&mut t, &mut boot.reply).await,
        };
        let (_retired_timer_runtime, timer_port) = retired_timer.into_shutdown_parts();
        let task = match runner.into_stopped() {
            Ok(t) => t,
            Err(mut r) => fail(&mut r, &mut boot.reply).await,
        };
        let retired = match shutdown::retire(
            task,
            actor.attachment_mut().controller_mut().gate_mut(),
            &release,
            timer_port,
        )
        .await
        {
            Ok(t) => t,
            Err(mut failure) => {
                if let shutdown::ControllerShutdownError::Output(cause) = failure.error() {
                    crate::release_fault::reset(&mut failure, cause);
                }
                fail(&mut failure, &mut boot.reply).await
            }
        };
        if let Err(mut retired) = actor
            .attachment_mut()
            .controller_mut()
            .return_retired(retired)
        {
            fail(&mut retired, &mut boot.reply).await;
        }
        let (cell, left) = core::pin::pin!(
            actor.run_until(cell.leave(Leaving::Tracked(tracked.member), tracked.payload))
        )
        .await;
        if !left.payload.stopped {
            fail(&mut actor, &mut boot.reply).await;
        }
        let inbox = match actor.detach() {
            Ok(inbox) => inbox,
            Err(mut actor) => fail(&mut actor, &mut boot.reply).await,
        };
        let (cold, coex, controller) = match attachment.into_stopped() {
            Ok(owners) => owners,
            Err(mut attachment) => fail(&mut attachment, &mut boot.reply).await,
        };
        let (arena, gate, pending) = match controller.into_reset() {
            Ok(owners) => owners,
            Err(mut controller) => fail(&mut controller, &mut boot.reply).await,
        };
        boot.inbox = Some(inbox);
        boot.cell = Some(cell);
        boot.cold = Some(cold);
        boot.coex = Some(coex);
        boot.arena = Some(arena);
        boot.gate = Some(gate);
        pending
    }
}
