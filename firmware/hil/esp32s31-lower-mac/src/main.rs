#![no_main]
#![no_std]
#![forbid(unsafe_code)]
#![feature(type_alias_impl_trait)]
#[cfg(not(feature = "diagnostic-lower-mac"))]
compile_error!("build the diagnostic-lower-mac image class through cargo hil");
mod irq;
mod suite;
mod transport;
use core::future::poll_fn;
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_board as board;
use oer_esp32s31_hal::{
    platform::{FromCpu0Irq, FromCpu2Irq, FromCpu3Irq, SystimerAlarm0, SystimerAlarm1},
    power::ChipClocks,
    root::Root,
    usb_serial_jtag::ConsoleInterrupt,
    watchdog::{ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters},
};
use oer_esp32s31_ieee80211_driver::irq::WifiIrqPart;
use oer_esp32s31_phy::{
    NoopPhyTargetObserver, PhyCalibrationIdentity, PhyRegisterConfig, concurrent::ConcurrentPhy,
    phy_get_rf_cal_version,
};
use oer_esp32s31_radio_client::{LpClock, RcSlow, claim};
use oer_esp32s31_radio_system::{Esp32s31Radio, RadioSystem};
use oer_espressif_executor::{Doorbell, Executor, InterruptExecutor, LevelOrder};
use oer_hil_agent::base::Intake;
use oer_hil_protocol::{
    RequestIdentity,
    base::{ImageKeySet, RejectReason, Rejected},
    lower_mac::*,
};
use oer_memory::{Doorbell as RequestDoorbell, Poster, RequestSlot};
use static_cell::StaticCell;
oer_espressif_staged_runtime::interrupt_table! {
    /// The application executor's wake (thread mode).
    wake: WakeToken = FromCpu0Irq => oer_espressif_executor::wake_handler::<0, FromCpu0Irq>, line 16, level 1, hart 0, Hardware;
    /// The radio executor (P2), polled in its handler.
    radio: RadioToken = FromCpu2Irq => oer_espressif_executor::interrupt_handler::<RadioTasks, FromCpu2Irq>, line 17, level 2, hart 0, Executor;
    /// The radio interrupt (P3): both Wi-Fi MAC banks and the doorbell.
    wifi: WifiToken = WifiIrqPart<irq::Lower> => crate::irq::on_wifi, line 18, level 3, hart 0, Hardware;
    /// The radio executor's timer alarm; it only acknowledges and pends.
    radio_alarm: RadioAlarmToken = SystimerAlarm0 => oer_espressif_executor::alarm_interrupt::<0, SystimerAlarm0>, line 19, level 3, hart 0, Hardware;
    /// The console's USB Serial/JTAG interrupt; it raises the endpoint's signal.
    console_usb: ConsoleUsb = ConsoleInterrupt => crate::transport::usb_interrupt, line 20, level 1, hart 0, Hardware;
    /// The application executor's timer alarm.
    app_alarm: AppAlarmToken = SystimerAlarm1 => oer_espressif_executor::alarm_interrupt::<1, SystimerAlarm1>, line 21, level 1, hart 0, Hardware;
}

oer_espressif_executor::tasks! {
    /// The radio executor's tasks (P2).
    RadioTasks: InterruptExecutor<2> {
        radio_actor(actor: &'static mut Actor) = radio_actor;
        serve_driver(boot: suite::Boot, channel: suite::DriverChannel) = serve_driver;
    }
}

oer_espressif_executor::tasks! {
    /// The application executor's tasks.
    AppTasks: Executor<0> {
        console(usb: transport::Usb, boot: u64, chip: transport::Chip) = console;
        dispatch(poster: Poster<Request, Answer>) = dispatch;
        serve_port(channel: suite::FrontendChannel) = serve_port;
        rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_rtc_watchdog;
    }
}
const LEVELS: LevelOrder<WifiToken, RadioToken, WakeToken> = LevelOrder::new();
static DOORBELL: RequestDoorbell = RequestDoorbell::new();
static SLOT: RequestSlot<Request, Answer> = RequestSlot::new(&DOORBELL, 1);
oer_hil_agent::requests! { enum Request(sessions = false) { Prepare(Prepare), Run(RunCase), } }
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
        oer_espressif_executor::Cooperative<EmbassyClock>,
        NoopPhyTargetObserver,
    >,
    EmbassyClock,
>;

/// The image's entropy owner.
pub(crate) type Rng = oer_esp32s31_hal::platform::Entropy<EmbassyClock>;

oer_espressif_staged_runtime::entry!(main);
fn main(handoff: oer_espressif_staged_runtime::Handoff) -> ! {
    oer_esp32s31_hal::usb_serial_jtag::early_note(&oer_diagnostics::Note::diagnostic_start(
        "lower-mac",
    ));
    let Some(root) = Root::take() else {
        fail("root-taken");
    };
    let runtime = root.into_runtime();
    let (_psram, mut adopted) = handoff.adopt(
        INTERRUPT_TABLE,
        runtime.interrupts.controller,
        board::BootstrapPsram,
    );
    let Some(interrupts) = Interrupts::take() else {
        fail("interrupt-table");
    };
    let factory = oer_esp32s31_hal::platform::FactoryIdentity::read(runtime.efuse);
    let identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        base_mac_address: factory.base_mac(),
        mac_extension: factory.mac_extension(),
    };
    let station = factory.address(oer_esp32s31_hal::platform::Interface::Station);
    let ap = factory.address(oer_esp32s31_hal::platform::Interface::AccessPoint);
    let (domain, partitions) = runtime.radio.split::<ChipClocks, _>(ConcurrentPhy::new());
    let Some((end, cells)) = claim() else {
        fail("radio-actor");
    };
    let Some(lp) = LpClock::<RcSlow>::claim() else {
        fail("low-power-clock");
    };
    let backend = Esp32s31Radio::new(
        domain,
        partitions.wifi_coex,
        runtime.clocks,
        oer_espressif_executor::Cooperative::new(EmbassyClock),
        NoopPhyTargetObserver,
        PhyRegisterConfig::new(identity),
    );
    let actor = ACTOR.init_with(|| RadioSystem::new(end, backend, EmbassyClock, lp));
    let timer = runtime.system_timer;
    oer_espressif_executor::init::<2, _, _>(
        oer_espressif_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        adopted.router(),
        interrupts.radio_alarm,
        timer.alarm0,
    );
    oer_espressif_executor::bind::<0, _, _>(adopted.router(), interrupts.app_alarm, timer.alarm1);
    let software = runtime.software_interrupts;
    let (line, ack) = Doorbell::<3>::split(software.line3);
    let Ok(bound) = adopted.router().bind(
        interrupts.wifi,
        WifiIrqPart::new(runtime.wifi_interrupts, software.irq3, irq::Lower(ack)),
    ) else {
        fail("wifi-route");
    };
    let bell = irq::ImageDoorbell(Doorbell::new::<FromCpu3Irq, _>(line, &bound));
    let radio = InterruptExecutor::<2>::new(software.line2).start(
        adopted.router(),
        interrupts.radio,
        software.irq2,
        LEVELS,
    );
    let mut rng = Rng::new(runtime.entropy, EmbassyClock);
    let boot = (u64::from(rng.random()) << 32 | u64::from(rng.random())).max(1);
    let (driver_channel, frontend_channel) = suite::channels();
    if RadioTasks::radio_actor(&radio, actor).is_err() {
        fail("task-start");
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
        fail("task-start");
    }
    let Some(poster) = SLOT.claim() else {
        fail("requester");
    };
    let chip = transport::Chip(oer_esp32s31_hal::platform::ResetCauses::read(
        runtime.reset_cause,
    ));
    let Ok((usb, _pads)) = transport::open_usb(adopted.router(), interrupts.console_usb) else {
        fail("console");
    };
    transport::CONSOLE.start(boot);
    let mut router = adopted.enable_interrupts();
    let watchdog = runtime.watchdogs.rtc;
    Executor::<0>::new(software.line0).run(
        &mut router,
        interrupts.wake,
        software.irq0,
        move |spawner| {
            if AppTasks::console(spawner, usb, boot, chip).is_err()
                || AppTasks::dispatch(spawner, poster).is_err()
                || AppTasks::serve_port(spawner, frontend_channel).is_err()
                || AppTasks::rtc_watchdog(spawner, watchdog).is_err()
            {
                fail("task-start");
            }
        },
    );
}

/// Halt the image on a failed boot step, its reason on the console while
/// no one holds the block.
fn fail(reason: &'static str) -> ! {
    oer_espressif_staged_runtime::fatal_halt(|_path| {
        let _note = oer_esp32s31_hal::usb_serial_jtag::fatal_note(
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
    let keys =
        oer_hil_protocol::feature_keys::image_keys(&|feature| feature == "diagnostic-lower-mac");
    let intake = Intake::new(boot, ImageKeySet::new(&keys), 0);
    transport::CONSOLE
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
