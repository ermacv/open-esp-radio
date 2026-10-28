#[cfg(not(feature = "diagnostic-phy"))]
compile_error!("build the diagnostic-phy image class through cargo hil");

macro_rules! capture_observer {
    ($radio:expr) => {{
        #[cfg(feature = "diagnostic-radio-health")]
        {
            crate::health::Observer {
                store: &mut $radio.health,
            }
        }
        #[cfg(not(feature = "diagnostic-radio-health"))]
        {
            oer_esp32s31_phy::NoopPhyTargetObserver
        }
    }};
}

mod cache;
mod cca_checkpoint;
mod cca_rx;
mod completion;
mod hold;
mod ieee802154;
mod projection;
mod radio;

const PHY_CLOCK: oer_espressif_executor::Cooperative<oer_embassy_time::EmbassyClock> =
    oer_espressif_executor::Cooperative::new(oer_embassy_time::EmbassyClock);
mod sar2_checkpoint;
mod transport;
mod tx_rate_checkpoint;

use core::future::poll_fn;
use oer_esp32s31_hal::{
    ieee802154::mac::Ieee802154InterruptOwner,
    platform::{FromCpu0Irq, FromCpu2Irq, SystimerAlarm0, SystimerAlarm1},
    power::ChipClocks,
    root::Root,
    usb_serial_jtag::ConsoleInterrupt,
    watchdog::{ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters},
};
use oer_esp32s31_phy::{PhyCalibrationIdentity, concurrent::ConcurrentPhy, phy_get_rf_cal_version};
use oer_espressif_executor::{Executor, InterruptExecutor};
use oer_hil_agent::base::Intake;
use oer_hil_protocol::{
    RequestIdentity,
    base::{RejectReason, Rejected},
    phy::*,
};
use oer_memory::{Doorbell, Poster, RequestSlot};
use static_cell::StaticCell;

// The IEEE 802.15.4 MAC's source has no entry: the radio task polls the
// engine (`ieee802154::poll`), with the router's proof that no line
// receives it.
mod phy_pilot;
mod pilot_timing;

oer_espressif_staged_runtime::interrupt_table! {
    contexts { app_wake: 1; radio_executor: 2; radio: 3; }
    /// The application executor's wake (thread mode).
    wake: WakeToken = FromCpu0Irq => oer_espressif_executor::wake_handler::<0, FromCpu0Irq>, line 40, context app_wake, hart 0, Hardware;
    // SOURCE: family.toml [interrupts] highest-line; HW23_S31_P2_ARBITRATION_20261027
    // measures only this P2 pair on S31: FROM_CPU2 line17 before TIMG0 line16.
    phy: PhyToken = oer_esp32s31_hal::platform::timg0_alarm::Timg0AlarmInterrupt => phy_pilot::interrupt, line 16, context radio_executor, hart 0, Exempt(ExecutorLevel, "SAR/PWDET steps between radio executor rounds");
    /// The radio executor (P2), polled in its handler.
    radio: RadioToken = FromCpu2Irq => oer_espressif_executor::interrupt_handler::<RadioTasks, FromCpu2Irq>, line 17, context radio_executor, hart 0, Executor;
    /// The radio executor's timer alarm; it only acknowledges and pends.
    radio_alarm: RadioAlarmToken = SystimerAlarm0 => oer_espressif_executor::alarm_interrupt::<0, SystimerAlarm0>, line 18, context radio, hart 0, Hardware;
    /// The application executor's timer alarm.
    app_alarm: AppAlarmToken = SystimerAlarm1 => oer_espressif_executor::alarm_interrupt::<1, SystimerAlarm1>, line 41, context app_wake, hart 0, Hardware;
    /// The console's USB Serial/JTAG interrupt; it raises the endpoint's signal.
    console_usb: ConsoleUsb = ConsoleInterrupt => crate::transport::usb_interrupt, line 42, context app_wake, hart 0, Hardware;
}

oer_espressif_executor::tasks! {
    /// The radio executor's tasks (P2).
    RadioTasks: InterruptExecutor<2> {
        #[cfg(not(feature = "diagnostic-radio-health"))]
        serve_radio(radio: &'static mut radio::Radio) = serve_radio;
        #[cfg(feature = "diagnostic-radio-health")]
        serve_radio(radio: &'static mut radio::Radio) = health::startup::serve_radio;
    }
}

oer_espressif_executor::tasks! {
    /// The application executor's tasks.
    AppTasks: Executor<0> {
        #[cfg(not(feature = "diagnostic-radio-health"))]
        console(usb: transport::Usb, boot: u64, chip: transport::Chip) = console;
        #[cfg(feature = "diagnostic-radio-health")]
        console(usb: transport::Usb, boot: u64, chip: transport::Chip) = health::startup::console;
        #[cfg(not(feature = "diagnostic-radio-health"))]
        dispatch(poster: Poster<Request, Answer>) = dispatch;
        #[cfg(feature = "diagnostic-radio-health")]
        dispatch(poster: Poster<Request, Answer>) = health::startup::dispatch;
        rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_rtc_watchdog;
    }
}
static RADIO: StaticCell<radio::Radio> = StaticCell::new();
static DOORBELL: Doorbell = Doorbell::new();
static SLOT: RequestSlot<Request, Answer> = RequestSlot::new(&DOORBELL, 1);

oer_hil_agent::requests! {
    enum WireRequest(sessions = false) {
        CacheIdentity(GetCalibrationCacheIdentity), Start(StartCalibration),
        CacheWrite(WriteCalibrationCachePage), CacheComplete(CompleteCalibrationCache),
        CacheStatus(GetCalibrationStartStatus), CacheRead(ReadCalibrationSnapshot),
        Restart(RestartCaptureRadio), Stop(StopCapture),
        HoldStart(StartTrackingHold), HoldGet(GetTrackingHold), HoldStop(StopTrackingHold),
        Calibration(ReadCalibrationProjection), Registers(ReadRegisterImage), Analog(ReadAnalogImage),
    }
}

// P1 publishes the wire response before releasing P2's sealed registration.
enum Request {
    Wire(WireRequest),
    Proceed,
}

enum Answer {
    CacheIdentity(CalibrationCacheIdentity),
    Started(CalibrationStarted),
    CacheWritten(CalibrationCachePageWritten),
    CacheCompleted(CalibrationCacheCompleted),
    CacheStatus(CalibrationStartStatus),
    CacheSnapshot(CalibrationSnapshot),
    Proceeded,
    HoldStarted(HoldStarted),
    HoldPage(TrackingHold),
    HoldStopped(HoldStopped),
    Restarted(CaptureRestarted),
    Stopped(CaptureStopped),
    Calibration(CalibrationProjection),
    Registers(RegisterImage),
    Analog(AnalogImage),
    Rejected(RejectReason),
}

// Both ends of this queue run on P1. P2 is reached only through RequestSlot.
static REQUESTS: embassy_sync::channel::Channel<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    (RequestIdentity, IntakeRequest),
    1,
> = embassy_sync::channel::Channel::new();

/// The image's entropy owner.
pub(crate) type Rng = oer_esp32s31_hal::platform::Entropy<oer_embassy_time::EmbassyClock>;

oer_esp32s31_hal::phy_fault_reporter!(oer_espressif_staged_runtime::phy_fault::reset);
oer_espressif_staged_runtime::entry!(main);
fn main(handoff: oer_espressif_staged_runtime::Handoff) -> ! {
    oer_esp32s31_hal::usb_serial_jtag::early_note(&oer_diagnostics::Note::diagnostic_start(
        oer_diagnostics::name!("phy"),
    ));
    let Some(root) = Root::take() else {
        fail(oer_diagnostics::name!("root-taken"; 32));
    };
    let runtime = root
        .into_runtime()
        .unwrap_or_else(|error| fail(error.name()));
    ieee802154::report_previous_fault();
    let (_psram, mut adopted) = match handoff.adopt(
        INTERRUPT_TABLE,
        runtime.interrupts.controller,
        oer_esp32s31_board::BootstrapPsram,
    ) {
        Ok(adopted) => adopted,
        Err(error) => fail(error.name()),
    };
    let Some(interrupts) = Interrupts::take() else {
        fail(oer_diagnostics::name!("interrupt-table"; 32));
    };
    let Ok(silent) = adopted.router().silent::<Ieee802154InterruptOwner>() else {
        fail(oer_diagnostics::name!("ieee802154-route"; 32));
    };
    let factory = oer_esp32s31_hal::platform::FactoryIdentity::read(runtime.efuse);
    let identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        base_mac_address: factory.base_mac(),
        mac_extension: factory.mac_extension(),
    };
    let station = factory.address(oer_esp32s31_hal::platform::Interface::Station);
    let access_point = factory.address(oer_esp32s31_hal::platform::Interface::AccessPoint);
    let (domain, partitions) = runtime
        .radio
        .split::<ChipClocks, _>(
            ConcurrentPhy::new(),
            oer_espressif_staged_runtime::phy_fault::identity(),
        )
        .unwrap_or_else(|failure| {
            let fault = failure.fault();
            let _owners = failure;
            oer_espressif_staged_runtime::phy_fault::reset(fault)
        });
    // RNG's inter-read settle needs an advancing clock before executor startup.
    let timer = runtime.system_timer;
    oer_espressif_executor::init::<2, _, _>(
        oer_espressif_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        adopted.router(),
        interrupts.radio_alarm,
        timer.alarm0,
    )
    .unwrap_or_else(|failure| failure.reset());
    let mut rng = Rng::new(runtime.entropy, oer_embassy_time::EmbassyClock);
    let boot = (u64::from(rng.random()) << 32 | u64::from(rng.random())).max(1);
    let (phy_pilot, pilot_timing) =
        crate::phy_pilot::init(runtime.timg0_alarm, adopted.router(), interrupts.phy);
    let radio = radio::Radio::place(
        RADIO.uninit(),
        domain,
        partitions.wifi,
        partitions.wifi_coex,
        ieee802154::Rx::new(partitions.ieee802154, partitions.ieee802154_coex, silent),
        runtime.clocks,
        identity,
        station,
        access_point,
        rng,
        phy_pilot,
        pilot_timing,
    );
    #[cfg(feature = "diagnostic-radio-health")]
    radio.set_health_boot(boot);
    oer_espressif_executor::bind::<0, _, _>(adopted.router(), interrupts.app_alarm, timer.alarm1)
        .unwrap_or_else(|failure| failure.reset());
    let software = runtime.software_interrupts;
    let (hart, router) = adopted.parts();
    let radio_spawner = InterruptExecutor::<2>::new(software.line2)
        .start(hart, router, interrupts.radio, software.irq2, ())
        .unwrap_or_else(|failure| failure.reset());
    if RadioTasks::serve_radio(&radio_spawner, radio).is_err() {
        fail(oer_diagnostics::name!("task-start"; 32));
    }
    let Some(poster) = SLOT.claim() else {
        fail(oer_diagnostics::name!("requester"; 32));
    };
    let chip = transport::Chip(oer_esp32s31_hal::platform::ResetCauses::read(
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
    match Executor::<0>::new(software.line0)
        .run(
            &hart,
            &mut router,
            interrupts.wake,
            software.irq0,
            (),
            move |spawner| {
                if AppTasks::console(spawner, usb, boot, chip).is_err()
                    || AppTasks::dispatch(spawner, poster).is_err()
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
    match watchdog
        .feed_every(&oer_embassy_time::EmbassyClock, period)
        .await {}
}

#[cfg(not(feature = "diagnostic-radio-health"))]
async fn console(usb: transport::Usb, boot: u64, chip: transport::Chip) {
    let keys = oer_hil_protocol::feature_keys::image_keys(&|feature| feature == "diagnostic-phy");
    let intake = Intake::new(boot, keys.as_set(), 0);
    let failure = transport::CONSOLE
        .run(usb, intake, &chip, |id, request: WireRequest| {
            if REQUESTS.try_send((id, Request::Wire(request))).is_err() {
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
#[cfg(not(feature = "diagnostic-radio-health"))]
async fn dispatch(mut poster: Poster<Request, Answer>) {
    loop {
        let (id, request) = REQUESTS.receive().await;
        let mut pending = poster.post(request);
        let (returned, answer) = poll_fn(|cx| pending.poll_answer(cx)).await;
        poster = returned;
        macro_rules! send {
            ($value:expr) => {
                transport::CONSOLE
                    .publish_reliably(id.session_id, id.request_id, &$value)
                    .await
            };
        }
        let release = matches!(&answer, Answer::Started(_) | Answer::CacheCompleted(_));
        let sequence = match answer {
            Answer::CacheIdentity(value) => {
                send!(value)
            }
            Answer::Started(value) => {
                send!(value)
            }
            Answer::CacheWritten(value) => {
                send!(value)
            }
            Answer::CacheCompleted(value) => {
                send!(value)
            }
            Answer::CacheStatus(value) => {
                send!(value)
            }
            Answer::CacheSnapshot(value) => {
                send!(value)
            }
            Answer::Proceeded => continue,

            Answer::HoldStarted(value) => {
                send!(value)
            }
            Answer::HoldPage(value) => {
                send!(value)
            }
            Answer::HoldStopped(value) => {
                send!(value)
            }
            Answer::Restarted(value) => {
                send!(value)
            }
            Answer::Stopped(value) => {
                send!(value)
            }
            Answer::Calibration(value) => {
                send!(value)
            }
            Answer::Registers(value) => {
                send!(value)
            }
            Answer::Analog(value) => {
                send!(value)
            }
            Answer::Rejected(reason) => {
                send!(Rejected(reason))
            }
        };
        if release {
            if let Err(fault) = transport::CONSOLE.written(sequence).await {
                transport::receiver_limit(fault);
            }
            let mut pending = poster.post(Request::Proceed);
            let (returned, _answer) = poll_fn(|cx| pending.poll_answer(cx)).await;
            poster = returned;
        }
    }
}
#[cfg(not(feature = "diagnostic-radio-health"))]
async fn serve_radio(radio: &'static mut radio::Radio) {
    // RX ring authority is core-local and is created only after the P2 spawn.
    let mut capture_rx = None;
    loop {
        radio.expire_cache();
        if radio.registration_pending() {
            let mut registration = core::pin::pin!(radio.service_registration());
            poll_fn(|cx| radio::poll_operation(registration.as_mut(), cx)).await;
            continue;
        }
        if let Some(deadline) = radio.deadline() {
            use core::future::Future;
            use oer_time::Timer;
            let mut timer = core::pin::pin!(oer_embassy_time::EmbassyClock.wait_until(deadline));
            poll_fn(|cx| {
                if DOORBELL.poll_rung(cx).is_ready() {
                    core::task::Poll::Ready(())
                } else {
                    timer.as_mut().poll(cx)
                }
            })
            .await;
        } else {
            poll_fn(|cx| DOORBELL.poll_rung(cx)).await;
        }
        ieee802154::poll();
        // A started PHY future always reaches its terminal result before a command.
        {
            let mut hold = core::pin::pin!(radio.service_hold());
            hold.as_mut().await;
        }
        if let Some((request, response)) = SLOT.take_request() {
            radio.expire_cache();
            match request {
                Request::Proceed => {
                    radio.release_cache();
                    response.answer(Answer::Proceeded);
                }
                Request::Wire(request) => {
                    // Request/response scratch must not remain on the stack
                    // while an unrelated registration or hold is polled.
                    let mut operation = core::pin::pin!(async {
                        let result = {
                            let mut request =
                                core::pin::pin!(radio.request(request, &mut capture_rx));
                            request.as_mut().await
                        };
                        radio.request_finished(&result);
                        response.answer(result.unwrap_or_else(Answer::Rejected));
                    });
                    poll_fn(|cx| radio::poll_operation(operation.as_mut(), cx)).await;
                }
            }
        }
    }
}

#[cfg(feature = "diagnostic-radio-health")]
mod health;

#[cfg(not(feature = "diagnostic-radio-health"))]
type IntakeRequest = Request;
#[cfg(feature = "diagnostic-radio-health")]
type IntakeRequest = oer_hil_agent::base::Either<Request, health::Request>;
#[cfg(feature = "diagnostic-radio-health")]
static HEALTH_SLOT: RequestSlot<health::Request, health::Answer> = RequestSlot::new(&DOORBELL, 1);
