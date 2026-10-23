//! Common runtime and shared PHY actor, without a BLE installation.
use super::*;
use esp_hal::{efuse, usb::usb_serial_jtag::UsbSerialJtag};
use oer_esp32s31_board as board;
use oer_esp32s31_phy::{
    NoopPhyTargetObserver, PhyCalibrationIdentity, PhyRegisterConfig, concurrent::ConcurrentPhy,
    phy_get_rf_cal_version,
};
use oer_esp32s31_radio_client::{LpClock, RcSlow, wifi::Wifi};
use oer_esp32s31_radio_system::{Esp32s31Radio, RadioSystem};
use oer_espressif_esp_hal_radio::{EspHalClocks, RadioSingletons, take_radio};
use oer_memory::OnceValue;
use oer_time::{Duration, Timer};
type Shared = Esp32s31Radio<EspHalClocks, EmbassyClock, NoopPhyTargetObserver>;
type Actor = RadioSystem<Wifi, RcSlow, Shared, EmbassyClock>;
static ACTOR: StaticCell<Actor> = StaticCell::new();
static RESULT: OnceValue<bool> = OnceValue::new();
pub fn boot(handoff: oer_espressif_staged_runtime::Handoff) -> ! {
    let previous = oer_espressif_staged_runtime::panic::take_previous();
    let p = esp_hal::init(esp_hal::Config::default());
    let (psram, adopted) = handoff.adopt(INTERRUPT_TABLE, board::BootstrapPsram(p.PSRAM));
    let mut console = UsbSerialJtag::new(p.USB_DEVICE);
    if previous.is_some() {
        let _ = console.write(b"OER-B0-BRINGUP: previous panic\r\n");
    }
    let Some(profile) = PROFILE else {
        let _ = console.write(b"OER-B0-BRINGUP: unconfigured\r\n");
        oer_espressif_staged_runtime::idle();
    };
    let tokens = Interrupts::take().expect("one image interrupt table");
    let mut base = [0; 6];
    base.copy_from_slice(efuse::base_mac_address().as_bytes());
    let identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        base_mac_address: base,
        mac_extension: efuse::read_field_le::<u16>(efuse::MAC_EXT),
    };
    let singletons = RadioSingletons {
        i2c_ana_mst: p.I2C_ANA_MST,
        modem_syscon: p.MODEM_SYSCON,
        modem_lpcon: p.MODEM_LPCON,
        wifi: p.WIFI,
        bt: p.BT,
        ieee802154: p.IEEE802154,
        lp_tsens: p.LP_TSENS,
    };
    let (hardware, clocks) = take_radio(singletons, &psram).ok().expect("one radio root");
    let (domain, parts) =
        hardware.split::<oer_espressif_esp_hal_radio::EspHal, _>(ConcurrentPhy::new());
    let (end, _) = oer_esp32s31_radio_client::claim().expect("one shared actor");
    let backend = Esp32s31Radio::new(
        domain,
        parts.wifi_coex,
        clocks,
        EmbassyClock,
        NoopPhyTargetObserver,
        PhyRegisterConfig::new(identity),
    );
    let actor = RadioSystem::new(
        end,
        backend,
        EmbassyClock,
        LpClock::<RcSlow>::claim().expect("one LP source"),
    );
    oer_espressif_executor::init::<2, _>(p.SYSTIMER, tokens.radio_alarm);
    oer_espressif_executor::bind::<0, _>(tokens.app_alarm);
    irq::initialize(SoftwareInterrupt::new(p.FROM_CPU_INTR3), tokens.doorbell);
    let radio = RADIO_EXECUTOR
        .init(X::new(SoftwareInterrupt::new(p.FROM_CPU_INTR2)))
        .start(tokens.radio, LEVELS);
    let actor = ACTOR.init(actor);
    radio.spawn(driver(actor).expect("one P2 driver task"));
    let app = EXECUTOR.init(Executor::<0>::new(SoftwareInterrupt::new(p.FROM_CPU_INTR0)));
    adopted.enable_interrupts();
    app.run(tokens.wake, |spawner| {
        spawner.spawn(report(console).expect("one report task"));
    })
}
#[embassy_executor::task]
async fn report(mut console: UsbSerialJtag<'static, esp_hal::Blocking>) {
    loop {
        if let Some(success) = RESULT.get() {
            let bytes: &[u8] = if success {
                b"OER-B0-BRINGUP: returned graph\r\n"
            } else {
                b"OER-B0-BRINGUP: fail\r\n"
            };
            let _ = console.write(bytes);
            return;
        }
        let _ = EmbassyClock.wait_for(Duration::from_millis(10)).await;
    }
}
#[embassy_executor::task]
async fn driver(actor: &'static mut Actor) {
    match actor.run().await {}
}
