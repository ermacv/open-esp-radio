#![no_main]
#![no_std]
#![forbid(unsafe_code)]

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
use esp_hal::{efuse, interrupt::software::SoftwareInterrupt, rng::RngOwner};
use oer_esp32s31_phy::{PhyCalibrationIdentity, concurrent::ConcurrentPhy, phy_get_rf_cal_version};
use oer_espressif_esp_hal_radio::{RadioSingletons, take_radio};
use oer_espressif_executor::{Executor, InterruptExecutor, LevelOrder};
use oer_hil_agent::base::Intake;
use oer_hil_protocol::{
    RequestIdentity,
    base::{ImageKeySet, RejectReason, Rejected},
    phy::*,
};
use oer_memory::{Doorbell, Poster, RequestSlot};
use static_cell::StaticCell;

oer_espressif_staged_runtime::interrupt_table! {
    wake: WakeToken = FROM_CPU_INTR0 => oer_espressif_executor::wake_handler::<0>, Priority1, ProCpu;
    radio: RadioToken = FROM_CPU_INTR2 => oer_espressif_executor::interrupt_handler::<2>, Priority2, ProCpu;
    mac: MacToken = MODEM_ZB_MAC => ieee802154::interrupt, Priority2, ProCpu;
    radio_alarm: RadioAlarmToken = SYSTIMER_TARGET0 => oer_espressif_executor::alarm_interrupt::<0>, Priority3, ProCpu;
    app_alarm: AppAlarmToken = SYSTIMER_TARGET1 => oer_espressif_executor::alarm_interrupt::<1>, Priority1, ProCpu;
    console_usb: ConsoleUsb = USB_DEVICE => esp_hal::usb::usb_serial_jtag::handle_async_interrupt, Priority1, ProCpu;
}

const LEVELS: LevelOrder<RadioAlarmToken, RadioToken, WakeToken> = LevelOrder::new();
static EXECUTOR: StaticCell<Executor<0>> = StaticCell::new();
static RADIO_EXECUTOR: StaticCell<InterruptExecutor<2>> = StaticCell::new();
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
    (RequestIdentity, Request),
    1,
> = embassy_sync::channel::Channel::new();

oer_espressif_staged_runtime::entry!(main);

fn main(handoff: oer_espressif_staged_runtime::Handoff) -> ! {
    oer_esp32s31_board::rom::install_usb_printf();
    oer_esp32s31_board::rom::line(c"OPEN_RADIO_HIL runtime=START diagnostic=phy");
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let (psram, adopted) = handoff.adopt(
        INTERRUPT_TABLE,
        oer_esp32s31_board::BootstrapPsram(peripherals.PSRAM),
    );
    let interrupts = Interrupts::take().expect("diagnostic interrupt table claimed once");
    let identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        base_mac_address: efuse::base_mac_address()
            .as_bytes()
            .try_into()
            .expect("base MAC has six octets"),
        mac_extension: efuse::read_field_le::<u16>(efuse::MAC_EXT),
    };
    let station = efuse::interface_mac_address(efuse::InterfaceMacAddress::Station)
        .as_bytes()
        .try_into()
        .expect("station MAC");
    let access_point = efuse::interface_mac_address(efuse::InterfaceMacAddress::AccessPoint)
        .as_bytes()
        .try_into()
        .expect("AP MAC");
    let (hardware, clocks) = take_radio(
        RadioSingletons {
            i2c_ana_mst: peripherals.I2C_ANA_MST,
            modem_syscon: peripherals.MODEM_SYSCON,
            modem_lpcon: peripherals.MODEM_LPCON,
            wifi: peripherals.WIFI,
            bt: peripherals.BT,
            ieee802154: peripherals.IEEE802154,
            lp_tsens: peripherals.LP_TSENS,
        },
        &psram,
    )
    .expect("diagnostic radio ownership");
    let (domain, partitions) =
        hardware.split::<oer_espressif_esp_hal_radio::EspHal, _>(ConcurrentPhy::new());
    let mut rng = RngOwner::new(peripherals.RNG);
    let boot = (u64::from(rng.random()) << 32 | u64::from(rng.random())).max(1);
    let radio = radio::Radio::place(
        RADIO.uninit(),
        domain,
        partitions.wifi,
        partitions.wifi_coex,
        ieee802154::Rx::new(
            partitions.ieee802154,
            partitions.ieee802154_coex,
            interrupts.mac,
        ),
        clocks,
        identity,
        station,
        access_point,
        rng,
    );
    oer_espressif_executor::init::<2, _>(peripherals.SYSTIMER, interrupts.radio_alarm);
    oer_espressif_executor::bind::<0, _>(interrupts.app_alarm);
    let radio_spawner = RADIO_EXECUTOR
        .init(InterruptExecutor::<2>::new(SoftwareInterrupt::new(
            peripherals.FROM_CPU_INTR2,
        )))
        .start(interrupts.radio, LEVELS);
    radio_spawner.spawn(serve_radio(radio).expect("radio diagnostic task"));
    let poster = SLOT.claim().expect("one diagnostic requester");
    let usb = transport::Usb::new(peripherals.USB_DEVICE, interrupts.console_usb);
    transport::CONSOLE.start(boot);
    adopted.enable_interrupts();
    EXECUTOR
        .init(Executor::<0>::new(SoftwareInterrupt::new(
            peripherals.FROM_CPU_INTR0,
        )))
        .run(interrupts.wake, move |spawner| {
            spawner.spawn(console(usb, boot).expect("diagnostic console task"));
            spawner.spawn(dispatch(poster).expect("diagnostic dispatch task"));
        });
}

#[embassy_executor::task]
async fn console(usb: transport::Usb, boot: u64) {
    let keys = oer_hil_protocol::feature_keys::image_keys(&|feature| feature == "diagnostic-phy");
    let intake = Intake::new(boot, ImageKeySet::new(&keys), 0);
    transport::CONSOLE
        .run(
            usb.rx,
            usb.tx,
            intake,
            &transport::Chip,
            |id, request: WireRequest| {
                if REQUESTS.try_send((id, Request::Wire(request))).is_err() {
                    transport::CONSOLE.publish(
                        id.session_id,
                        id.request_id,
                        &Rejected(RejectReason::Busy),
                    );
                }
            },
        )
        .await;
}

#[embassy_executor::task]
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

#[embassy_executor::task]
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
