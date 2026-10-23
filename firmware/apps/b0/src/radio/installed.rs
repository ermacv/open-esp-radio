use super::*;
pub(super) type Installed<'p, 'e> = impl core::future::Future<Output = ()>;
static INSTALLED: static_cell::ConstStaticCell<
    oer_memory::StaticPlace<Installed<'static, 'static>>,
> = static_cell::ConstStaticCell::new(oer_memory::StaticPlace::new());
#[define_opaque(Installed)]
fn installed<'p, 'e>(
    boot: &'static mut Boot,
    pool: PoolScope<'p>,
    epoch: EpochScope<'e>,
) -> Installed<'p, 'e> {
    async move {
        let common = boot.actor.as_mut().expect("one actor handover");
        let inbox = boot.inbox.take().expect("one BLE inbox");
        let cold = boot.cold.take().expect("one cold client");
        let coex = boot.coex.take().expect("one coex client");
        let cell = boot.cell.take().expect("one membership");
        let bell = boot.bell.take().expect("one scheduler endpoint");
        let timer: &'static irq::TimerPort = boot.timer.take().expect("one P2 timer port");
        let routes = boot.routes.take().expect("one route epoch");
        let profile = boot.profile;
        let address = boot.address;
        let arena = ARENA.take().expect("one static arena");
        let controller = lifecycle::new(arena, pool, epoch, address);
        let mut attachment = BluetoothAttachment::new(cold, coex, controller);
        let mut actor = match common.attach_bluetooth(inbox, &mut attachment) {
            Ok(actor) => actor,
            Err(mut inbox) => hold(&mut inbox).await,
        };
        let domain =
            RegulatoryDomain::of(Country::new(profile.country).expect("build-validated country"))
                .expect("profile country in database");
        let band = domain.permit(LeBand).expect("profile permits LE band");
        let permission = Permission::new(
            band,
            PowerLimits {
                antenna_gain_mbm: 0,
                chip_maximum_mbm: 2100,
                owner_cap_mbm: Some(i32::from(profile.cap_dbm) * 100),
            },
        );
        let join = BluetoothJoin::new(permission, Operation::DtmRx, None)
            .expect("listening has no TX power request");
        let (cell, joined) = actor.run_until(cell.join(join)).await;
        let joined = match joined {
            Ok(j) => j,
            Err(_) => hold(&mut actor).await,
        };
        let (cell, tracked) = actor
            .run_until(cell.track(joined.member, joined.payload.installation))
            .await;
        let tracked = match tracked {
            Ok(t) => t,
            Err(_) => hold(&mut actor).await,
        };
        let runtime = match actor
            .attachment_mut()
            .controller_mut()
            .take_runtime(tracked.payload.0)
        {
            Ok(r) => r,
            Err(_) => hold(&mut actor).await,
        };
        let oer_esp32s31_bluetooth_driver::low_power::ControllerRuntimeEndpoints {
            interrupt: _,
            mut task,
            modem_timer: modem,
        } = runtime.endpoints;
        let scheduler = match task
            .task_owner()
            .engine()
            .install_scheduler_interrupts(&irq::CHANNEL)
        {
            Ok(s) => s,
            Err(_) => hold(&mut task).await,
        };
        let half = oer_esp32s31_bluetooth_driver::interrupt::ControllerInterruptHalf::new(
            runtime.output.stage_for_cpu_routes(),
            scheduler,
            &irq::REPLY,
        );
        let ready = runtime.timer.start_runtime_timer().stage_for_interrupt();
        if let Err(mut retained) = routes.publish(half, ready) {
            hold(&mut retained).await;
        }
        let active = match routes.enable() {
            Ok(r) => r,
            Err(mut r) => hold(&mut r).await,
        };
        let mut timer_task = ControllerModemTimerTask::new(timer, modem);
        let mut runner = Runner::new(
            task,
            runtime.timing,
            bell,
            EmbassyClock,
            &irq::WAKE,
            permission,
            NoClockRequests,
            Limits::new(Duration::from_micros(10), Duration::from_millis(100))
                .expect("finite bring-up containment"),
            &irq::FAULT,
        );
        let result = active::run(&mut actor, &mut runner, &mut timer_task).await;
        if result.is_err() {
            hold(&mut runner).await;
        }
        let routes = active.disable();
        let deadline = EmbassyClock
            .now()
            .checked_add(Duration::from_millis(100))
            .expect("finite timer shutdown bound");
        while !timer_task.retirement_ready() {
            if EmbassyClock.now() >= deadline || active::timer_step(&mut timer_task).is_err() {
                hold(&mut timer_task).await;
            }
            let _ = EmbassyClock.wait_for(Duration::from_micros(10)).await;
        }
        let (half, ready) = match routes.take() {
            Ok(x) => x,
            Err(_) => hold(&mut runner).await,
        };
        timer.park_after_disable(ready);
        let retired_timer = match timer_task.try_retire() {
            Ok(t) => t,
            Err((_, mut t)) => hold(&mut t).await,
        };
        let (ready, _, _) = retired_timer.into_shutdown_parts();
        let (output, scheduler) = half.deactivate();
        let mut task = match runner.into_stopped() {
            Ok(t) => t,
            Err(mut r) => hold(&mut r).await,
        };
        if let Err(mut retained) = task
            .task_owner()
            .engine()
            .retire_scheduler_interrupts(scheduler)
        {
            hold(&mut retained).await;
        }
        let retired = match shutdown::retire(task, output, ready, &EmbassyClock) {
            Ok(t) => t,
            Err(mut f) => hold(&mut f).await,
        };
        if let Err(mut retired) = actor.attachment_mut().controller_mut().return_retired(retired) {
            hold(&mut retired).await;
        }
        let (cell, left) = actor
            .run_until(cell.leave(Leaving::Tracked(tracked.member), tracked.payload))
            .await;
        if !left.payload.stopped {
            hold(&mut actor).await;
        }
        let inbox = match actor.detach() {
            Ok(inbox) => inbox,
            Err(mut actor) => hold(&mut actor).await,
        };
        let (cold, coex, controller) = match attachment.into_stopped() {
            Ok(owners) => owners,
            Err(mut attachment) => hold(&mut attachment).await,
        };
        let arena = match controller.into_reset() {
            Ok(arena) => arena,
            Err(mut controller) => hold(&mut controller).await,
        };
        let mut returned = (inbox, cell, cold, coex, arena);
        let _ = RESULT.set(true);
        hold_success(common, &mut returned).await;

    }
}
pub(super) fn run<'a, 'p: 'a, 'e: 'a>(
    boot: &'static mut Boot,
    pool: PoolScope<'p>,
    epoch: EpochScope<'e>,
) -> oer_memory::PlacedFuture<'a, Installed<'p, 'e>> {
    INSTALLED.take().future(|| installed(boot, pool, epoch))
}
