#![no_main]
#![no_std]
#![forbid(unsafe_code)]
#![feature(type_alias_impl_trait)]

#[cfg(not(feature = "diagnostic-phy"))]
compile_error!("build the diagnostic-phy image class through cargo hil");

mod cache;
mod cca_checkpoint;
mod cca_rx;
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
use oer_espressif_executor::{Executor, InterruptExecutor, LevelOrder};
use oer_hil_agent::base::Intake;
use oer_hil_protocol::{
    RequestIdentity,
    base::{ImageKeySet, RejectReason, Rejected},
    phy::*,
};
use oer_memory::{Doorbell, Poster, RequestSlot};
use static_cell::StaticCell;

// The IEEE 802.15.4 MAC's source has no entry: the radio task polls the
// engine (`ieee802154::poll`), with the router's proof that no line
// receives it.
oer_espressif_staged_runtime::interrupt_table! {
    /// The application executor's wake (thread mode).
    wake: WakeToken = FromCpu0Irq => oer_espressif_executor::wake_handler::<0, FromCpu0Irq>, line 16, level 1, hart 0, Hardware;
    /// The radio executor (P2), polled in its handler.
    radio: RadioToken = FromCpu2Irq => oer_espressif_executor::interrupt_handler::<RadioTasks, FromCpu2Irq>, line 17, level 2, hart 0, Executor;
    /// The radio executor's timer alarm; it only acknowledges and pends.
    radio_alarm: RadioAlarmToken = SystimerAlarm0 => oer_espressif_executor::alarm_interrupt::<0, SystimerAlarm0>, line 18, level 3, hart 0, Hardware;
    /// The application executor's timer alarm.
    app_alarm: AppAlarmToken = SystimerAlarm1 => oer_espressif_executor::alarm_interrupt::<1, SystimerAlarm1>, line 19, level 1, hart 0, Hardware;
    /// The console's USB Serial/JTAG interrupt; it raises the endpoint's signal.
    console_usb: ConsoleUsb = ConsoleInterrupt => crate::transport::usb_interrupt, line 20, level 1, hart 0, Hardware;
}

const LEVELS: LevelOrder<RadioAlarmToken, RadioToken, WakeToken> = LevelOrder::new();
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

oer_espressif_staged_runtime::entry!(main);
fn main(handoff: oer_espressif_staged_runtime::Handoff) -> ! {
    oer_esp32s31_hal::usb_serial_jtag::early_note(&oer_diagnostics::Note::diagnostic_start("phy"));
    let Some(root) = Root::take() else {
        fail("root-taken");
    };
    let runtime = root.into_runtime();
    let (_psram, mut adopted) = match handoff.adopt(
        INTERRUPT_TABLE,
        runtime.interrupts.controller,
        oer_esp32s31_board::BootstrapPsram,
    ) {
        Ok(adopted) => adopted,
        Err(error) => fail(error.name()),
    };
    let Some(interrupts) = Interrupts::take() else {
        fail("interrupt-table");
    };
    let Ok(silent) = adopted.router().silent::<Ieee802154InterruptOwner>() else {
        fail("ieee802154-route");
    };
    let factory = oer_esp32s31_hal::platform::FactoryIdentity::read(runtime.efuse);
    let identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        base_mac_address: factory.base_mac(),
        mac_extension: factory.mac_extension(),
    };
    let station = factory.address(oer_esp32s31_hal::platform::Interface::Station);
    let access_point = factory.address(oer_esp32s31_hal::platform::Interface::AccessPoint);
    let (domain, partitions) = runtime.radio.split::<ChipClocks, _>(ConcurrentPhy::new());
    let mut rng = Rng::new(runtime.entropy, oer_embassy_time::EmbassyClock);
    let boot = (u64::from(rng.random()) << 32 | u64::from(rng.random())).max(1);
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
    );
    #[cfg(feature = "diagnostic-radio-health")]
    radio.set_health_boot(boot);
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
    let radio_spawner = InterruptExecutor::<2>::new(software.line2).start(
        adopted.router(),
        interrupts.radio,
        software.irq2,
        LEVELS,
    );
    if RadioTasks::serve_radio(&radio_spawner, radio).is_err() {
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
    let mut router = match adopted.enable_interrupts() {
        Ok(router) => router,
        Err(error) => fail(error.name()),
    };
    let watchdog = runtime.watchdogs.rtc;
    Executor::<0>::new(software.line0).run(
        &mut router,
        interrupts.wake,
        software.irq0,
        move |spawner| {
            if AppTasks::console(spawner, usb, boot, chip).is_err()
                || AppTasks::dispatch(spawner, poster).is_err()
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
    match watchdog
        .feed_every(&oer_embassy_time::EmbassyClock, period)
        .await {}
}

#[cfg(not(feature = "diagnostic-radio-health"))]
async fn console(usb: transport::Usb, boot: u64, chip: transport::Chip) {
    let keys = oer_hil_protocol::feature_keys::image_keys(&|feature| feature == "diagnostic-phy");
    let intake = Intake::new(boot, ImageKeySet::new(&keys), 0);
    transport::CONSOLE
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
            transport::CONSOLE.written(sequence).await;
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
            poll_fn(|cx| radio::poll_registration(registration.as_mut(), cx)).await;
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
                    let result = radio.request(request, &mut capture_rx).await;
                    radio.request_finished(&result);
                    response.answer(result.unwrap_or_else(Answer::Rejected));
                }
            }
        }
    }
}

#[cfg(feature = "diagnostic-radio-health")]
mod health;
#[macro_export]
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

#[cfg(not(feature = "diagnostic-radio-health"))]
type IntakeRequest = Request;
#[cfg(feature = "diagnostic-radio-health")]
type IntakeRequest = oer_hil_agent::base::Either<Request, health::Request>;
#[cfg(feature = "diagnostic-radio-health")]
static HEALTH_SLOT: RequestSlot<health::Request, health::Answer> = RequestSlot::new(&DOORBELL, 1);
