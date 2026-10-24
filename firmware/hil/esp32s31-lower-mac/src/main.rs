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
use esp_hal::interrupt::software::SoftwareInterrupt;
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
    wake: WakeToken = FROM_CPU_INTR0 => oer_espressif_executor::wake_handler::<0>, Priority1, ProCpu, Hardware;
    radio: RadioToken = FROM_CPU_INTR2 => oer_espressif_executor::interrupt_handler::<RadioTasks>, Priority2, ProCpu, Executor;
    doorbell: DoorbellToken = FROM_CPU_INTR3 => crate::irq::doorbell, Priority3, ProCpu, Hardware;
    wifi_mac: WifiMacToken = MODEM_WIFI_MAC => crate::irq::wifi_mac, Priority3, ProCpu, Hardware;
    wifi_pwr: WifiPwrToken = MODEM_WIFI_PWR => crate::irq::wifi_pwr, Priority3, ProCpu, Hardware;
    radio_alarm: RadioAlarmToken = SYSTIMER_TARGET0 => oer_espressif_executor::alarm_interrupt::<0>, Priority3, ProCpu, Hardware;
    console_usb: ConsoleUsb = USB_DEVICE => crate::transport::usb_interrupt, Priority1, ProCpu, Hardware;
    app_alarm: AppAlarmToken = SYSTIMER_TARGET1 => oer_espressif_executor::alarm_interrupt::<1>, Priority1, ProCpu, Hardware;
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
    }
}
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
static ACTOR: StaticCell<Actor> = StaticCell::new();

type Actor = RadioSystem<
    oer_esp32s31_radio_client::wifi::Wifi,
    RcSlow,
    Esp32s31Radio<
        EspHalClocks,
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
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let (psram, adopted) = handoff.adopt(INTERRUPT_TABLE, board::BootstrapPsram(peripherals.PSRAM));
    let interrupts = Interrupts::take().expect("conformance interrupt table claimed once");
    let factory = oer_esp32s31_hal::platform::FactoryIdentity::read(
        oer_espressif_esp_hal_soc::handoff::take_efuse()
            .expect("the image takes the eFuse words once"),
    );
    let identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        base_mac_address: factory.base_mac(),
        mac_extension: factory.mac_extension(),
    };
    let station = factory.address(oer_esp32s31_hal::platform::Interface::Station);
    let ap = factory.address(oer_esp32s31_hal::platform::Interface::AccessPoint);
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
    );
    let (domain, partitions) = hardware.split(ConcurrentPhy::new());
    let (end, cells) = claim().expect("diagnostic radio actor");
    let lp = LpClock::<RcSlow>::claim().expect("diagnostic low power clock");
    let backend = Esp32s31Radio::new(
        domain,
        partitions.wifi_coex,
        clocks,
        oer_espressif_executor::Cooperative::new(EmbassyClock),
        NoopPhyTargetObserver,
        PhyRegisterConfig::new(identity),
    );
    let actor = ACTOR.init_with(|| RadioSystem::new(end, backend, EmbassyClock, lp));
    oer_espressif_executor::init::<2, _>(peripherals.SYSTIMER, interrupts.radio_alarm);
    oer_espressif_executor::bind::<0, _>(interrupts.app_alarm);
    let bell = irq::ImageDoorbell::new(
        SoftwareInterrupt::new(peripherals.FROM_CPU_INTR3),
        interrupts.doorbell,
    );
    let radio = InterruptExecutor::<2>::new(SoftwareInterrupt::new(peripherals.FROM_CPU_INTR2))
        .start(interrupts.radio, LEVELS);
    let mut rng = Rng::new(
        oer_espressif_esp_hal_soc::handoff::take_entropy(peripherals.RNG, peripherals.LP_PERI),
        EmbassyClock,
    );
    let boot = (u64::from(rng.random()) << 32 | u64::from(rng.random())).max(1);
    let (driver_channel, frontend_channel) = suite::channels();
    RadioTasks::radio_actor(&radio, actor).expect("radio actor task");
    RadioTasks::serve_driver(
        &radio,
        suite::Boot {
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
        },
        driver_channel,
    )
    .expect("conformance task");
    let poster = SLOT.claim().expect("one diagnostic requester");
    let chip = transport::Chip(oer_esp32s31_hal::platform::ResetCauses::read(
        oer_espressif_esp_hal_soc::handoff::take_reset_causes(peripherals.LP_AON_CLK_RST),
    ));
    let usb = transport::open_usb(
        oer_espressif_esp_hal_soc::handoff::take_usb_serial_jtag(peripherals.USB_DEVICE)
            .expect("no early record holds the USB Serial/JTAG at the console handoff"),
        interrupts.console_usb,
    );
    transport::CONSOLE.start(boot);
    adopted.enable_interrupts();
    Executor::<0>::new(SoftwareInterrupt::new(peripherals.FROM_CPU_INTR0)).run(
        interrupts.wake,
        move |spawner| {
            AppTasks::console(spawner, usb, boot, chip).expect("diagnostic console task");
            AppTasks::dispatch(spawner, poster).expect("diagnostic dispatch task");
            AppTasks::serve_port(spawner, frontend_channel).expect("P1 conformance body task");
        },
    );
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
