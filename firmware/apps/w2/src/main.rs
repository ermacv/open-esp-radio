#![no_main]
#![no_std]
#![forbid(unsafe_code)]
#![feature(type_alias_impl_trait)]

mod irq;
mod report;
mod scan;

use esp_hal::{
    efuse, interrupt::software::SoftwareInterrupt, rng::RngOwner,
    usb::usb_serial_jtag::UsbSerialJtag,
};
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
use oer_memory::{AffineSpscQueue, AffineSpscReceiver};
use static_cell::StaticCell;

#[derive(Clone, Copy)]
pub struct W2Config {
    pub channel: u8,
    pub stand: [u8; 6],
    pub ssid: &'static [u8],
    pub country: [u8; 2],
    pub cap_dbm: i8,
}
include!(concat!(env!("OUT_DIR"), "/w2_config.rs"));

oer_espressif_staged_runtime::interrupt_table! {
    wake: WakeToken = FROM_CPU_INTR0 => oer_espressif_executor::wake_handler::<0>, Priority1, ProCpu, Hardware;
    radio: RadioToken = FROM_CPU_INTR2 => oer_espressif_executor::interrupt_handler::<RadioTasks>, Priority2, ProCpu, Executor;
    doorbell: DoorbellToken = FROM_CPU_INTR3 => crate::irq::doorbell, Priority3, ProCpu, Hardware;
    wifi_mac: WifiMacToken = MODEM_WIFI_MAC => crate::irq::wifi_mac, Priority3, ProCpu, Hardware;
    wifi_pwr: WifiPwrToken = MODEM_WIFI_PWR => crate::irq::wifi_pwr, Priority3, ProCpu, Hardware;
    radio_alarm: RadioAlarmToken = SYSTIMER_TARGET0 => oer_espressif_executor::alarm_interrupt::<0>, Priority3, ProCpu, Hardware;
    app_alarm: AppAlarmToken = SYSTIMER_TARGET1 => oer_espressif_executor::alarm_interrupt::<1>, Priority1, ProCpu, Hardware;
}

/// The application's console.
type Console = UsbSerialJtag<'static, esp_hal::Blocking>;

oer_espressif_executor::tasks! {
    /// The radio executor's tasks (P2).
    RadioTasks: InterruptExecutor<2> {
        radio_actor(actor: &'static mut Actor) = radio_actor;
        scanner(boot: scan::Boot) = scan::scanner;
    }
}

oer_espressif_executor::tasks! {
    /// The application executor's tasks.
    AppTasks: Executor<0> {
        reporting(
            console: Console,
            receiver: AffineSpscReceiver<'static, report::Report, 1>,
        ) = reporting;
    }
}
static REPORT: StaticCell<AffineSpscQueue<report::Report, 1>> = StaticCell::new();
const LEVELS: LevelOrder<WifiMacToken, RadioToken, WakeToken> = LevelOrder::new();
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

oer_espressif_staged_runtime::entry!(main);
fn main(handoff: oer_espressif_staged_runtime::Handoff) -> ! {
    let previous = oer_espressif_staged_runtime::panic::take_previous();
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let (psram, adopted) = handoff.adopt(INTERRUPT_TABLE, board::BootstrapPsram(peripherals.PSRAM));
    let mut console = UsbSerialJtag::new(peripherals.USB_DEVICE);
    if let Some(record) = previous {
        let _ = oer_espressif_staged_runtime::diagnostics::emit_previous_panic(
            |bytes| console.write(bytes),
            &record,
        );
    }
    let Some(config) = CONFIG else {
        let _ = console.write(b"OER-RESULT: fail w2 unconfigured\r\n");
        park()
    };
    let Some(interrupts) = Interrupts::take() else {
        park()
    };
    let mut base = [0; 6];
    base.copy_from_slice(efuse::base_mac_address().as_bytes());
    let mut station = [0; 6];
    station.copy_from_slice(
        efuse::interface_mac_address(efuse::InterfaceMacAddress::Station).as_bytes(),
    );
    let mut ap = [0; 6];
    ap.copy_from_slice(
        efuse::interface_mac_address(efuse::InterfaceMacAddress::AccessPoint).as_bytes(),
    );
    let identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        base_mac_address: base,
        mac_extension: efuse::read_field_le::<u16>(efuse::MAC_EXT),
    };
    let singletons = RadioSingletons {
        i2c_ana_mst: peripherals.I2C_ANA_MST,
        modem_syscon: peripherals.MODEM_SYSCON,
        modem_lpcon: peripherals.MODEM_LPCON,
        wifi: peripherals.WIFI,
        bt: peripherals.BT,
        ieee802154: peripherals.IEEE802154,
        lp_tsens: peripherals.LP_TSENS,
    };
    let (hardware, clocks) = match take_radio(singletons, &psram) {
        Ok(pair) => pair,
        Err(_) => {
            let _ = console.write(b"OER-RESULT: fail w2 take-radio\r\n");
            park()
        }
    };
    let (domain, partitions) =
        hardware.split::<oer_espressif_esp_hal_radio::EspHal, _>(ConcurrentPhy::new());
    let (Some((end, cells)), Some(lp)) = (claim(), LpClock::<RcSlow>::claim()) else {
        park()
    };
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
    let (sender, receiver) = REPORT.init(AffineSpscQueue::new()).split();
    RadioTasks::radio_actor(&radio, actor).expect("the radio actor's first start");
    RadioTasks::scanner(
        &radio,
        scan::Boot {
            cell: cells.wifi,
            wifi: partitions.wifi,
            rng: RngOwner::new(peripherals.RNG),
            station,
            ap,
            config,
            routes: irq::ImageRoutes {
                mac: interrupts.wifi_mac,
                power: interrupts.wifi_pwr,
            },
            bell,
            sender,
        },
    )
    .expect("the scanner's first start");
    let executor = Executor::<0>::new(SoftwareInterrupt::new(peripherals.FROM_CPU_INTR0));
    adopted.enable_interrupts();
    executor.run(interrupts.wake, move |spawner| {
        AppTasks::reporting(spawner, console, receiver).expect("the report's first start");
    })
}

async fn radio_actor(actor: &'static mut Actor) {
    match actor.run().await {}
}
async fn reporting(mut console: Console, receiver: AffineSpscReceiver<'static, report::Report, 1>) {
    let mut result = loop {
        if let Ok(report) = receiver.try_receive() {
            break report;
        }
        use oer_time::Timer;
        let _ = EmbassyClock
            .wait_for(oer_time::Duration::from_millis(10))
            .await;
    };
    result.irq_free =
        oer_espressif_staged_runtime::stacks::current_hart_interrupt_stack_free_bytes();
    if result.irq_free < 4096 {
        result.errors += 1;
    }
    let _ = result.notes(|note| oer_diagnostics::emit_note(note, |bytes| console.write(bytes)));
}
fn park() -> ! {
    oer_espressif_staged_runtime::idle()
}
