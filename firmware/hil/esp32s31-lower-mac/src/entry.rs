#[cfg(not(feature = "diagnostic-lower-mac"))]
compile_error!("build the diagnostic-lower-mac image class through cargo hil");
mod irq;
mod services;
mod suite;
mod transport;
use core::future::poll_fn;
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_board as board;
use oer_esp32s31_compose::Esp32s31;
use oer_esp32s31_hal::{
    peripherals::{software_interrupt::{FromCpu0Irq, FromCpu2Irq, FromCpu3Irq}, system_timer::{SystimerAlarm0, SystimerAlarm1}},
    power::ChipClocks,
    root::Root,
    peripherals::usb_serial_jtag::ConsoleInterrupt,
    peripherals::watchdog::{ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters},
};
use oer_esp32s31_ieee80211_driver::irq::WifiIrqPart;
use oer_esp32s31_phy::{
    NoopPhyTargetObserver, PhyCalibrationIdentity, PhyRegisterConfig, concurrent::ConcurrentPhy,
    phy_get_rf_cal_version,
};
use oer_esp32s31_radio_client::{LpClock, RcSlow, claim};
use oer_esp32s31_radio_system::{Esp32s31Radio, RadioSystem};
use oer_espressif_executor::{Doorbell, Executor, InterruptExecutor};
use oer_hil_agent::base::Intake;
use oer_hil_protocol::{RequestIdentity, base::RejectReason, base::Rejected};
use oer_ieee80211_conformance_control::*;
use oer_memory::{Doorbell as RequestDoorbell, Poster, RequestSlot};
use static_cell::StaticCell;
mod phy_pilot;

oer_espressif_staged_runtime::interrupt_table! {
    contexts { app_wake: 1; radio_executor: 2; radio: 3; }
    /// The application executor's wake (thread mode).
    wake: WakeToken = FromCpu0Irq => oer_espressif_executor::wake_handler::<Esp32s31, 0, FromCpu0Irq>, line 40, context app_wake, hart 0, Hardware;
    // SOURCE: family.toml [interrupts] highest-line; HW23_S31_P2_ARBITRATION_20261027
    // measures only this P2 pair on S31: FROM_CPU2 line17 before TIMG0 line16.
    phy: PhyToken = oer_esp32s31_hal::peripherals::timg0_alarm::Timg0AlarmInterrupt => phy_pilot::interrupt, line 16, context radio_executor, hart 0, Exempt(ExecutorLevel, "SAR/PWDET steps between radio executor rounds");
    /// The radio executor (P2), polled in its handler.
    radio: RadioToken = FromCpu2Irq => oer_espressif_executor::interrupt_handler::<RadioTasks, FromCpu2Irq>, line 17, context radio_executor, hart 0, Executor;
    /// The radio interrupt (P3): both Wi-Fi MAC banks and the doorbell.
    wifi: WifiToken = WifiIrqPart<irq::Lower> => crate::irq::on_wifi, line 18, context radio, hart 0, Hardware;
    /// The radio executor's timer alarm; it only acknowledges and pends.
    radio_alarm: RadioAlarmToken = SystimerAlarm0 => oer_espressif_executor::alarm_interrupt::<Esp32s31, 0, SystimerAlarm0>, line 19, context radio, hart 0, Hardware;
    /// The console's USB Serial/JTAG interrupt; it raises the endpoint's signal.
    console_usb: ConsoleUsb = ConsoleInterrupt => crate::transport::usb_interrupt, line 41, context app_wake, hart 0, Hardware;
    /// The application executor's timer alarm.
    app_alarm: AppAlarmToken = SystimerAlarm1 => oer_espressif_executor::alarm_interrupt::<Esp32s31, 1, SystimerAlarm1>, line 42, context app_wake, hart 0, Hardware;
}

oer_espressif_executor::tasks! {
    /// The radio executor's tasks (P2).
    RadioTasks: InterruptExecutor<Esp32s31, 2> {
        radio_actor(actor: &'static mut Actor) = radio_actor;
        serve_driver(boot: suite::Boot, channel: suite::DriverChannel) = serve_driver;
    }
}

oer_espressif_executor::tasks! {
    /// The application executor's tasks.
    AppTasks: Executor<Esp32s31, 0> {
        console(usb: transport::Usb, boot: u64, chip: transport::Chip) = console;
        dispatch(poster: Poster<Request, Answer>) = dispatch;
        serve_port(channel: suite::FrontendChannel) = serve_port;
        rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_rtc_watchdog;
    }
}
static DOORBELL: RequestDoorbell = RequestDoorbell::new();
static SLOT: RequestSlot<Request, Answer> = RequestSlot::new(&DOORBELL, 1);
use services::Request;
enum Answer {
    Prepared(Prepared),
    Trace(CaseTrace),
    Rejected(RejectReason),
}
static REQUESTS: embassy_sync::channel::Channel<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    (RequestIdentity, Request),
    1,
> = embassy_sync::channel::Channel::new();
static ACTOR: StaticCell<Actor> = StaticCell::new();

type Actor = RadioSystem<
    oer_esp32s31_radio_client::wifi::Wifi,
    RcSlow,
    Esp32s31Radio<
        ChipClocks,
        oer_espressif_executor::Cooperative<Esp32s31, EmbassyClock>,
        NoopPhyTargetObserver,
        crate::phy_pilot::Client,
    >,
    EmbassyClock,
>;

/// The image's entropy owner.
pub(crate) type Rng = oer_esp32s31_hal::peripherals::entropy::Entropy<EmbassyClock>;

oer_esp32s31_hal::phy_fault_reporter!(oer_esp32s31_compose::phy_fault::reset);
oer_espressif_staged_runtime::entry!(main, oer_esp32s31_hal::platform::Port);
fn main(handoff: oer_espressif_staged_runtime::Handoff<oer_esp32s31_hal::platform::Port>) -> ! {
    oer_esp32s31_hal::peripherals::usb_serial_jtag::early_note(&oer_diagnostics::Note::diagnostic_start(
        oer_diagnostics::name!("lower-mac"),
    ));
    let Some(root) = Root::take() else {
        fail(oer_diagnostics::name!("root-taken"; 32));
    };
    let runtime = root
        .into_runtime()
        .unwrap_or_else(|error| fail(error.name()));
    oer_esp32s31_compose::wifi_fault::report_previous();
    let (_psram, mut adopted) = match handoff.adopt(
        INTERRUPT_TABLE,
        runtime.interrupts.controller,
        board::BootstrapPsram,
        oer_esp32s31_compose::phy_fault::report_previous,
    ) {
        Ok(adopted) => adopted,
        Err(error) => fail(error.name()),
    };
    let Some(interrupts) = Interrupts::take() else {
        fail(oer_diagnostics::name!("interrupt-table"; 32));
    };
    let factory = oer_esp32s31_hal::peripherals::efuse::FactoryIdentity::read(runtime.efuse);
    let identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        base_mac_address: factory.base_mac(),
        mac_extension: factory.mac_extension(),
    };
    let station = factory.address(oer_esp32s31_hal::peripherals::efuse::Interface::Station);
    let ap = factory.address(oer_esp32s31_hal::peripherals::efuse::Interface::AccessPoint);
    let (domain, partitions) = runtime
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
    let Some((end, cells)) = claim() else {
        fail(oer_diagnostics::name!("radio-actor"; 32));
    };
    let Some(lp) = LpClock::<RcSlow>::claim() else {
        fail(oer_diagnostics::name!("low-power-clock"; 32));
    };
    let phy_pilot = crate::phy_pilot::init(runtime.timg0_alarm, adopted.router(), interrupts.phy);
    let backend = Esp32s31Radio::new(
        domain,
        partitions.wifi_coex,
        runtime.clocks,
        oer_espressif_executor::Cooperative::<Esp32s31, _>::new(EmbassyClock),
        NoopPhyTargetObserver,
        PhyRegisterConfig::new(identity),
        phy_pilot,
    );
    let actor = ACTOR.init_with(|| RadioSystem::new(end, backend, EmbassyClock, lp));
    let timer = runtime.system_timer;
    oer_espressif_executor::init::<Esp32s31, 2, _, _>(
        oer_espressif_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        adopted.router(),
        interrupts.radio_alarm,
        timer.alarm0,
    )
    .unwrap_or_else(|failure| failure.reset());
    oer_espressif_executor::bind::<Esp32s31, 0, _, _>(
        adopted.router(),
        interrupts.app_alarm,
        timer.alarm1,
    )
    .unwrap_or_else(|failure| failure.reset());
    let software = runtime.software_interrupts;
    let (line, ack) = oer_espressif_executor::DoorbellLine::<Esp32s31, 3>::split(software.line3);
    let bound = adopted
        .router()
        .bind(
            interrupts.wifi,
            WifiIrqPart::new(runtime.wifi_interrupts, software.irq3, irq::Lower(ack)),
        )
        .unwrap_or_else(|error| {
            oer_espressif_staged_runtime::interrupts::reset_binding::<
                oer_esp32s31_hal::platform::Port,
                _,
            >(error)
        });
    let (bell, urgent) = Doorbell::new::<FromCpu3Irq>(line, &bound);
    let bell = irq::ImageDoorbell(bell);
    let (hart, router) = adopted.parts();
    let radio = InterruptExecutor::<Esp32s31, 2>::new(software.line2)
        .start(hart, router, interrupts.radio, software.irq2, urgent)
        .unwrap_or_else(|failure| failure.reset());
    let mut rng = Rng::new(runtime.entropy, EmbassyClock);
    let boot = (u64::from(rng.random()) << 32 | u64::from(rng.random())).max(1);
    let (driver_channel, frontend_channel) = suite::channels();
    if RadioTasks::radio_actor(&radio, actor).is_err() {
        fail(oer_diagnostics::name!("task-start"; 32));
    }
    if RadioTasks::serve_driver(
        &radio,
        suite::Boot {
            cell: cells.wifi,
            wifi: partitions.wifi,
            rng,
            station,
            ap,
            bound,
            bell,
        },
        driver_channel,
    )
    .is_err()
    {
        fail(oer_diagnostics::name!("task-start"; 32));
    }
    let Some(poster) = SLOT.claim() else {
        fail(oer_diagnostics::name!("requester"; 32));
    };
    let chip = transport::Chip(oer_esp32s31_hal::peripherals::reset_cause::ResetCauses::read(
        runtime.reset_cause,
    ));
    let Ok((usb, _pads)) = transport::open_usb(adopted.router(), interrupts.console_usb) else {
        fail(oer_diagnostics::name!("console"; 32));
    };
    transport::CONSOLE.start(boot);
    let (mut router, hart) = match adopted.enable_interrupts() {
        Ok(router) => router,
        Err(error) => fail(error.name()),
    };
    let watchdog = runtime.watchdogs.rtc;
    match Executor::<Esp32s31, 0>::new(software.line0)
        .run(
            &hart,
            &mut router,
            interrupts.wake,
            software.irq0,
            urgent,
            move |spawner| {
                if AppTasks::console(spawner, usb, boot, chip).is_err()
                    || AppTasks::dispatch(spawner, poster).is_err()
                    || AppTasks::serve_port(spawner, frontend_channel).is_err()
                    || AppTasks::rtc_watchdog(spawner, watchdog).is_err()
                {
                    fail(oer_diagnostics::name!("task-start"; 32));
                }
            },
        )
        .unwrap_or_else(|failure| failure.reset()) {};
}

/// Halt the image on a failed boot step, its reason on the console while
/// no one holds the block.
fn fail<const B: usize>(reason: oer_diagnostics::Name<B>) -> ! {
    oer_espressif_staged_runtime::fatal_halt::<oer_esp32s31_hal::platform::Port>(|_path| {
        let _note = oer_esp32s31_hal::peripherals::usb_serial_jtag::fatal_note(
            &oer_diagnostics::Note::runtime_fail(reason),
        );
    })
}

/// Feed the RTC watchdog for good: the image resets when this executor stops
/// polling.
async fn feed_rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) {
    let period = oer_time::Duration::from_millis(RTC_WATCHDOG_FEED_PERIOD_MS);
    match watchdog.feed_every(&EmbassyClock, period).await {}
}

async fn radio_actor(actor: &'static mut Actor) {
    match actor.run().await {}
}
async fn console(usb: transport::Usb, boot: u64, chip: transport::Chip) {
    let intake = Intake::new(boot, services::KEYS, 0);
    let failure = transport::CONSOLE
        .run(usb, intake, &chip, |id, request: Request| {
            if REQUESTS.try_send((id, request)).is_err() {
                transport::CONSOLE.publish(
                    id.session_id,
                    id.request_id,
                    &Rejected(RejectReason::Busy),
                );
            }
        })
        .await;
    transport::console_fault(failure.cause);
}

async fn dispatch(mut poster: Poster<Request, Answer>) {
    loop {
        let (id, request) = REQUESTS.receive().await;
        let mut pending = poster.post(request);
        let (returned, answer) = poll_fn(|cx| pending.poll_answer(cx)).await;
        poster = returned;
        match answer {
            Answer::Prepared(value) => {
                transport::CONSOLE
                    .publish_reliably(id.session_id, id.request_id, &value)
                    .await;
            }
            Answer::Trace(value) => {
                transport::CONSOLE
                    .publish_reliably(id.session_id, id.request_id, &value)
                    .await;
            }
            Answer::Rejected(reason) => {
                transport::CONSOLE
                    .publish_reliably(id.session_id, id.request_id, &Rejected(reason))
                    .await;
            }
        }
    }
}
async fn serve_driver(boot: suite::Boot, channel: suite::DriverChannel) {
    let slot = suite::Slot::take_static().expect("one static conformance slot");
    suite::drive(boot, slot, channel).await
}
async fn serve_port(channel: suite::FrontendChannel) {
    suite::serve(channel).await
}
