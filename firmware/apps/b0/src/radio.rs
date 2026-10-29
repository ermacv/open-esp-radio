//! Permanent physical P2 worker. Only ordinary graph return admits another epoch.
use oer_esp32s31_compose::Esp32s31;
use super::*;
use core::future::{pending, poll_fn};
use embassy_futures::select::{Either, select};
use oer_bluetooth_le_port::LeBand;
use oer_bluetooth_le_port::TxPower;
use oer_esp32s31_bluetooth_driver::{
    modem_timer::{
        ControllerModemTimerBegin, ControllerModemTimerPlanSubmission,
        ControllerModemTimerReadinessClass, ControllerModemTimerStep, ControllerModemTimerTask,
    },
    port::{
        Fault, mailbox as rpc,
        runner::{ClockRequests, Limits, Memory, Runner},
    },
    radio::ControllerLifecycle,
    runtime_resources::ControllerRuntimeResources,
    shutdown,
};
use oer_esp32s31_board as board;
use oer_esp32s31_hal::{
    bluetooth::engine::{
        ArenaError, BluetoothArena, ChipDma, Engine, EpochScope, PoolScope,
        SchedulerAllocationConfig, with_epoch_async, with_pool_async,
    },
    bluetooth::{ClockedOwner, ColdOwner, InterruptSetupOwner},
    memory::dma_static::Placed,
    domain::{BluetoothClient, CoexAbsent},
    power::ChipClocks,
    root::Root,
};
use oer_esp32s31_phy::{
    NoopPhyTargetObserver, PhyCalibrationIdentity, PhyRegisterConfig, concurrent::ConcurrentPhy,
    phy_get_rf_cal_version,
};
use oer_esp32s31_radio_client::{
    ClientInbox, Leaving, LpClock, MembershipCell, RcSlow,
    bluetooth::{self, Bluetooth, BluetoothJoin, Operation, Permission},
    wifi::Wifi,
};
use oer_esp32s31_radio_system::{BluetoothActor, BluetoothAttachment, Esp32s31Radio, RadioSystem};
use oer_radio_regulatory::{Country, PowerLimits, RegulatoryDomain};
use oer_time::{Clock, Duration, Timer};

type Shared = Esp32s31Radio<
    ChipClocks,
    oer_espressif_executor::Cooperative<Esp32s31, EmbassyClock>,
    NoopPhyTargetObserver,
    crate::phy_pilot::Client,
>;
type Actor = RadioSystem<Wifi, RcSlow, Shared, EmbassyClock>;
type Arena = BluetoothArena<0, 0, 0, 0, 0, 0>;
mod lifecycle;
oer_esp32s31_hal::dma_static! {static ARENA:Arena=zeroed in ".dma.bss.bluetooth.dtm";}

oer_espressif_executor::tasks! {
    /// The radio executor's tasks (P2).
    pub(crate) RadioTasks: InterruptExecutor<Esp32s31, 2> {
        driver(boot: &'static mut Boot) = task::driver;
        supervisor() = bridge::supervisor;
    }
}

oer_espressif_executor::tasks! {
    /// The application executor's tasks.
    pub(crate) AppTasks: Executor<Esp32s31, 0> {
        serve(
            console: host::Console,
            pads: oer_esp32s31_hal::peripherals::usb_serial_jtag::UsbPads,
            rng: host::Rng,
            channel: bridge::FrontendChannel,
            power: TxPower,
        ) = host::serve;
        rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_rtc_watchdog;
    }
}

pub(crate) struct Boot {
    actor: Option<Actor>,
    inbox: Option<ClientInbox<Bluetooth>>,
    cold: Option<ColdOwner>,
    coex: Option<CoexAbsent<BluetoothClient>>,
    cell: Option<MembershipCell<Bluetooth>>,
    bell: &'static irq::SchedulerBell,
    port: Option<&'static mut irq::TimerPort>,
    gate: Option<irq::Gate>,
    arena: Option<Placed<Arena>>,
    port_epoch: u64,
    reply: Option<(rpc::Command, bridge::Response)>,
    stop_reporter: bridge::StopReporter,
    profile: Profile,
    address: [u8; 6],
}
static BOOT: StaticCell<Boot> = StaticCell::new();
type LiveActor<'loan, 'p, 'e> = BluetoothActor<
    'loan,
    RcSlow,
    Shared,
    EmbassyClock,
    BluetoothAttachment<lifecycle::Life<'p, 'e>>,
>;
type LiveRunner<'p, 'e> = Runner<'p, 'e, X, SchedulerAccess, EmbassyClock, bridge::RuntimeClock>;
mod active;
mod installed;

static SCHEDULER: StaticCell<irq::SchedulerBell> = StaticCell::new();
struct SchedulerAccess(&'static irq::SchedulerBell);
impl oer_esp32s31_hal::bluetooth::engine::scheduler::interrupt::SchedulerDoorbell
    for SchedulerAccess
{
    fn ring(&self) {
        oer_esp32s31_hal::bluetooth::engine::scheduler::interrupt::SchedulerDoorbell::ring(self.0);
    }
    fn poll_reply(
        &self,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<
        Result<
            (),
            oer_esp32s31_hal::bluetooth::engine::scheduler::interrupt::SchedulerInterruptError,
        >,
    > {
        oer_esp32s31_hal::bluetooth::engine::scheduler::interrupt::SchedulerDoorbell::poll_reply(
            self.0, cx,
        )
    }
}
pub fn permission(profile: Profile) -> (Permission, TxPower) {
    let domain = RegulatoryDomain::of(Country::new(profile.country).expect("build country"))
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
    let power = TxPower::within(
        oer_bluetooth_le_port::LeTxCeiling::new(band, permission.ceiling()),
        profile.cap_dbm,
    )
    .expect("configured power within its ceiling");
    permission
        .tx_power(Operation::DtmTx, power)
        .expect("profile permits DTM TX");
    (permission, power)
}

pub fn boot(handoff: oer_espressif_staged_runtime::Handoff<oer_esp32s31_hal::platform::Port>) -> ! {
    let previous =
        oer_espressif_staged_runtime::panic::peek_previous::<oer_esp32s31_hal::platform::Port>();
    let Some(root) = Root::take() else {
        crate::fail("root-taken")
    };
    let (runtime, _psram, mut adopted) = match oer_espressif_staged_runtime::adoption::run::<oer_esp32s31_compose::Esp32s31, _, _>(
        handoff,
        root.into_runtime(),
        INTERRUPT_TABLE,
        board::BootstrapPsram,
        oer_esp32s31_compose::phy_fault::report_previous,
        core::convert::identity,
    ) {
        Ok(adopted) => adopted.into_parts(),
        Err(error) => crate::fail(error.name().as_str()),
    };
    crate::gate_fault::report_previous();
    crate::release_fault::report_previous();
    if let Some(previous) = previous
        && oer_esp32s31_hal::peripherals::usb_serial_jtag::early_text(b"OER-B0-BRINGUP: previous panic\r\n") {
        oer_espressif_staged_runtime::panic::clear_previous(&previous);
    }
    let profile = PROFILE;
    let Some(tokens) = Interrupts::take() else {
        crate::fail("interrupt-table")
    };
    let factory = oer_esp32s31_hal::peripherals::efuse::FactoryIdentity::read(runtime.efuse);
    let base = factory.base_mac();
    let identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        base_mac_address: base,
        mac_extension: factory.mac_extension(),
    };
    let (domain, parts) = runtime
        .radio
        .split::<ChipClocks, _>(
            ConcurrentPhy::new(),
            oer_esp32s31_compose::phy_fault::identity(),
        )
        .unwrap_or_else(|failure| {
            let fault = failure.fault();
            let _owners = failure;
            oer_esp32s31_compose::phy_fault::reset(fault)
        });
    let cold = ColdOwner::from_partition(parts.bluetooth);
    let (Some((end, _)), Some((inbox, cell)), Some(lp)) = (
        oer_esp32s31_radio_client::claim(),
        bluetooth::claim(),
        LpClock::<RcSlow>::claim(),
    ) else {
        crate::fail("claim")
    };
    let phy_pilot = crate::phy_pilot::init(runtime.timg0_alarm, adopted.router(), tokens.phy);
    let backend = Esp32s31Radio::new(
        domain,
        parts.wifi_coex,
        runtime.clocks,
        oer_espressif_executor::Cooperative::<Esp32s31, _>::new(EmbassyClock),
        NoopPhyTargetObserver,
        PhyRegisterConfig::new(identity),
        phy_pilot,
    );
    let actor = RadioSystem::new(end, backend, EmbassyClock, lp);
    let timer = runtime.system_timer;
    oer_espressif_executor::init::<Esp32s31, 2, _, _>(
        oer_espressif_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        adopted.router(),
        tokens.radio_alarm,
        timer.alarm0,
    )
    .unwrap_or_else(|failure| failure.reset());
    oer_espressif_executor::bind::<Esp32s31, 0, _, _>(adopted.router(), tokens.app_alarm, timer.alarm1)
        .unwrap_or_else(|failure| failure.reset());
    let software = runtime.software_interrupts;
    let Ok(bound) = irq::bind(
        adopted.router(),
        tokens.bt,
        software.line3,
        software.irq3,
        cold.timer_status(),
    ) else {
        crate::fail("bluetooth-interrupts")
    };
    let Some(bell) = SCHEDULER.try_init(bound.bell) else {
        crate::fail("scheduler-bell")
    };
    let urgent = bound.urgent;
    let _ = bridge::clock_state();
    let (hart, router) = adopted.parts();
    let radio = X::new(software.line2)
        .start(hart, router, tokens.radio, software.irq2, urgent)
        .unwrap_or_else(|failure| failure.reset());
    let Some(arena) = ARENA.take() else {
        crate::fail("arena")
    };
    let Some((stop_reporter, stop_reports)) = bridge::stop_reports() else {
        crate::fail("stop-report-storage")
    };
    let Some(boot) = BOOT.try_init(Boot {
        actor: Some(actor),
        inbox: Some(inbox),
        cold: Some(cold),
        coex: Some(parts.bluetooth_coex),
        cell: Some(cell),
        bell,
        port: Some(bound.port),
        gate: Some(bound.gate),
        arena: Some(arena),
        port_epoch: 0,
        reply: None,
        stop_reporter,
        profile,
        address: base,
    }) else {
        crate::fail("boot")
    };
    if RadioTasks::driver(&radio, boot).is_err() || RadioTasks::supervisor(&radio).is_err() {
        crate::fail("radio-tasks");
    }
    let (console, pads) = usb::open(adopted.router(), tokens.console_usb);
    let app = Executor::<Esp32s31, 0>::new(software.line0);
    let (mut router, hart) = match adopted.enable_interrupts() {
        Ok(router) => router,
        Err(error) => crate::fail(error.name().as_str()),
    };
    let rng = host::Rng::new(runtime.entropy, EmbassyClock);
    let watchdog = runtime.watchdogs.rtc;
    match app
        .run(
            &hart,
            &mut router,
            tokens.wake,
            software.irq0,
            urgent,
            move |spawner| {
                let (_, power) = permission(profile);
                if AppTasks::serve(
                    spawner,
                    console,
                    pads,
                    rng,
                    bridge::frontend(stop_reports),
                    power,
                )
                .is_err()
                    || AppTasks::rtc_watchdog(spawner, watchdog).is_err()
                {
                    crate::fail("app-tasks");
                }
            },
        )
        .unwrap_or_else(|failure| failure.reset()) {}
}
async fn hold<T>(owner: &mut T) -> ! {
    core::hint::black_box(owner);
    irq::FAULT.store(true, core::sync::atomic::Ordering::Release);
    bridge::fail_clock(Fault::Hardware);
    pending().await
}
async fn fail<T>(owner: &mut T, reply: &mut Option<(rpc::Command, bridge::Response)>) -> ! {
    bridge::complete(reply, Err(Fault::Hardware));
    hold(owner).await
}
async fn fault<T>(
    owner: &mut T,
    reply: &mut Option<(rpc::Command, bridge::Response)>,
    cause: Fault,
) -> ! {
    if let Fault::ControllerTimeRange(observation) = cause {
        core::hint::black_box(&mut *owner);
        crate::gate_fault::reset_controller_time(observation);
    }
    bridge::complete(reply, Err(cause));
    bridge::fail_clock(cause);
    hold(owner).await
}
mod task {
    pub(super) async fn driver(boot: &'static mut super::Boot) {
        super::WORKER.take().future(|| super::worker(boot)).await;
    }
}
type Worker = impl core::future::Future<Output = ()>;
static WORKER: static_cell::ConstStaticCell<oer_memory::StaticPlace<Worker>> =
    static_cell::ConstStaticCell::new(oer_memory::StaticPlace::new());
#[define_opaque(Worker)]
fn worker(boot: &'static mut Boot) -> Worker {
    async move {
        let Some(timer) = boot.port.take() else {
            hold(boot).await
        };
        let timer: &'static irq::TimerPort = timer;
        loop {
            let (command, reply) = bridge::command().await;
            if command.operation != rpc::Operation::Enable
                || boot.port_epoch.checked_add(1) != Some(command.epoch)
            {
                bridge::answer(command, reply, Err(Fault::Accounting));
                hold(boot).await;
            }
            boot.port_epoch = command.epoch;
            boot.reply = Some((command, reply));
            let pending = {
                let mut pool = core::pin::pin!(with_pool_async(async |pool| {
                    let mut epoch = core::pin::pin!(with_epoch_async(async |epoch| {
                        let mut installed =
                            core::pin::pin!(installed::run(boot, pool, epoch, timer));
                        installed.as_mut().await
                    }));
                    epoch.as_mut().await
                }));
                pool.as_mut().await
            };
            let bytes = [pending.state_0024(), pending.state_002c()];
            if let Err(mut report) = boot.stop_reporter.try_send(bytes) {
                fail(&mut report, &mut boot.reply).await;
            }
            // Every scoped future and its old generative authority ended;
            // the actual arena and cold owners are back in Boot.
            bridge::complete(&mut boot.reply, Ok(rpc::Completion::Done));
        }
    }
}
