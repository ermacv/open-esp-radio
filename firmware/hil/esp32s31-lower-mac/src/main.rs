#![no_main]
#![no_std]
#![forbid(unsafe_code)]
#![feature(type_alias_impl_trait, impl_trait_in_assoc_type)]
#[cfg(not(feature = "diagnostic-lower-mac"))]
compile_error!("build the diagnostic-lower-mac image class through cargo hil");
mod irq;
mod suite;
mod transport;
use core::future::poll_fn;
use esp_hal::{efuse, interrupt::software::SoftwareInterrupt, rng::RngOwner};
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_board as board;
use oer_esp32s31_phy::{
    NoopPhyTargetObserver, PhyCalibrationIdentity, PhyRegisterConfig, concurrent::ConcurrentPhy,
    phy_get_rf_cal_version,
};
use oer_esp32s31_radio_client::{LpClock, RcSlow, claim};
use oer_esp32s31_radio_system::{Esp32s31Radio, RadioSystem};
use oer_espressif_esp_hal_radio::{EspHalClocks, RadioSingletons, take_radio};
use oer_espressif_executor::{Executor, InterruptExecutor, LevelOrder};
use oer_hil_agent::base::Intake;
use oer_hil_protocol::{
    RequestIdentity,
    base::{ImageKeySet, RejectReason, Rejected},
    lower_mac::*,
};
use oer_memory::{Doorbell, Poster, RequestSlot};
use static_cell::StaticCell;
oer_espressif_staged_runtime::interrupt_table! {
    wake: WakeToken = FROM_CPU_INTR0 => oer_espressif_executor::wake_handler::<0>, Priority1, ProCpu;
    radio: RadioToken = FROM_CPU_INTR2 => oer_espressif_executor::interrupt_handler::<2>, Priority2, ProCpu;
    doorbell: DoorbellToken = FROM_CPU_INTR3 => crate::irq::doorbell, Priority3, ProCpu;
    wifi_mac: WifiMacToken = MODEM_WIFI_MAC => crate::irq::wifi_mac, Priority3, ProCpu;
    wifi_pwr: WifiPwrToken = MODEM_WIFI_PWR => crate::irq::wifi_pwr, Priority3, ProCpu;
    radio_alarm: RadioAlarmToken = SYSTIMER_TARGET0 => oer_espressif_executor::alarm_interrupt::<0>, Priority3, ProCpu;
    console_usb: ConsoleUsb = USB_DEVICE => esp_hal::usb::usb_serial_jtag::handle_async_interrupt, Priority1, ProCpu;
    app_alarm: AppAlarmToken = SYSTIMER_TARGET1 => oer_espressif_executor::alarm_interrupt::<1>, Priority1, ProCpu;
}

static EXECUTOR: StaticCell<Executor<0>> = StaticCell::new();
static RADIO: StaticCell<InterruptExecutor<2>> = StaticCell::new();
const LEVELS: LevelOrder<WifiMacToken, RadioToken, WakeToken> = LevelOrder::new();
static DOORBELL: Doorbell = Doorbell::new();
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
type Actor = RadioSystem<
    oer_esp32s31_radio_client::wifi::Wifi,
    RcSlow,
    Esp32s31Radio<EspHalClocks, EmbassyClock, NoopPhyTargetObserver>,
    EmbassyClock,
>;

oer_espressif_staged_runtime::entry!(main);
fn main(handoff: oer_espressif_staged_runtime::Handoff) -> ! {
    board::rom::install_usb_printf();
    board::rom::line(c"OPEN_RADIO_HIL runtime=START diagnostic=lower-mac");
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let (psram, adopted) = handoff.adopt(INTERRUPT_TABLE, board::BootstrapPsram(peripherals.PSRAM));
    let interrupts = Interrupts::take().expect("conformance interrupt table claimed once");
    let identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        base_mac_address: efuse::base_mac_address()
            .as_bytes()
            .try_into()
            .expect("base MAC"),
        mac_extension: efuse::read_field_le::<u16>(efuse::MAC_EXT),
    };
    let station = efuse::interface_mac_address(efuse::InterfaceMacAddress::Station)
        .as_bytes()
        .try_into()
        .expect("station MAC");
    let ap = efuse::interface_mac_address(efuse::InterfaceMacAddress::AccessPoint)
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
    let (domain, partitions) = hardware.split(ConcurrentPhy::new());
    let (end, cells) = claim().expect("diagnostic radio actor");
    let lp = LpClock::<RcSlow>::claim().expect("diagnostic low power clock");
    let backend = Esp32s31Radio::new(
        domain,
        partitions.wifi_coex,
        clocks,
        EmbassyClock,
        NoopPhyTargetObserver,
        PhyRegisterConfig::new(identity),
    );
    let actor = RadioSystem::new(end, backend, EmbassyClock, lp);
    oer_espressif_executor::init::<2, _>(peripherals.SYSTIMER, interrupts.radio_alarm);
    oer_espressif_executor::bind::<0, _>(interrupts.app_alarm);
    let bell = irq::ImageDoorbell::new(
        SoftwareInterrupt::new(peripherals.FROM_CPU_INTR3),
        interrupts.doorbell,
    );
    let radio = RADIO
        .init(InterruptExecutor::<2>::new(SoftwareInterrupt::new(
            peripherals.FROM_CPU_INTR2,
        )))
        .start(interrupts.radio, LEVELS);
    let mut rng = RngOwner::new(peripherals.RNG);
    let boot = (u64::from(rng.random()) << 32 | u64::from(rng.random())).max(1);
    radio.spawn(radio_actor(actor).expect("radio actor task"));
    radio.spawn(
        serve_radio(suite::Boot {
            cell: cells.wifi,
            wifi: partitions.wifi,
            rng,
            station,
            ap,
            routes: irq::ImageRoutes {
                mac: interrupts.wifi_mac,
                power: interrupts.wifi_pwr,
            },
            bell,
        })
        .expect("conformance task"),
    );
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
async fn radio_actor(actor: Actor) {
    match actor.run().await {}
}
#[embassy_executor::task]
async fn console(usb: transport::Usb, boot: u64) {
    let keys =
        oer_hil_protocol::feature_keys::image_keys(&|feature| feature == "diagnostic-lower-mac");
    let intake = Intake::new(boot, ImageKeySet::new(&keys), 0);
    transport::CONSOLE
        .run(
            usb.rx,
            usb.tx,
            intake,
            &transport::Chip,
            |id, request: Request| {
                if REQUESTS.try_send((id, request)).is_err() {
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
#[embassy_executor::task]
async fn serve_radio(boot: suite::Boot) {
    let mut server = suite::Server::new(boot);
    loop {
        poll_fn(|cx| DOORBELL.poll_rung(cx)).await;
        if let Some((request, response)) = SLOT.take_request() {
            let answer = server
                .request(request)
                .await
                .unwrap_or_else(Answer::Rejected);
            response.answer(answer);
        }
    }
}
