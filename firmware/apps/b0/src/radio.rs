//! The first image composition: real actor join, one quiet RX event and normal
//! graph return. This stage is measured before HCI/stand integration proceeds.
use super::*;
use core::{
    convert::Infallible,
    future::{pending, poll_fn},
    task::{Context, Poll},
};
use embassy_futures::select::{Either, select};
use esp_hal::{efuse, usb::usb_serial_jtag::UsbSerialJtag};
use oer_bluetooth_le_port::{Receive, TestChannel, TestPhy};
use oer_bluetooth_le_radio::LeBand;
use oer_esp32s31_bluetooth_driver::{
    modem_timer::{
        ControllerModemTimerBegin, ControllerModemTimerReadinessClass, ControllerModemTimerRearm,
        ControllerModemTimerStep, ControllerModemTimerTask,
    },
    port::{
        Fault, Start,
        runner::{ClockRequests, Limits, Memory, Runner},
    },
    radio::ControllerLifecycle,
    runtime_resources::{ControllerEventCells, ControllerRuntimeResources},
    shutdown,
};
use oer_esp32s31_board as board;
use oer_esp32s31_hal::{
    bluetooth::engine::{
        ArenaError, BluetoothArena, ChipDma, Engine, EpochScope, PoolScope,
        SchedulerAllocationConfig, with_epoch_async, with_pool_async,
    },
    bluetooth::{ClockedOwner, ColdOwner, InterruptSetupOwner},
    dma_static::Placed,
    domain::{BluetoothClient, CoexAbsent},
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
use oer_esp32s31_radio_system::{Esp32s31Radio, BluetoothActor, BluetoothAttachment, RadioSystem};
use oer_espressif_esp_hal_radio::{EspHalClocks, RadioSingletons, take_radio};
use oer_memory::OnceValue;
use oer_radio_regulatory::{Country, PowerLimits, RegulatoryDomain};
use oer_time::{Clock, Duration, Timer};

type Shared = Esp32s31Radio<EspHalClocks, EmbassyClock, NoopPhyTargetObserver>;
type Actor = RadioSystem<Wifi, RcSlow, Shared, EmbassyClock>;
type Arena = BluetoothArena<0, 0, 0, 0, 0, 0>;
mod lifecycle;
oer_esp32s31_hal::dma_static! {static ARENA:Arena=zeroed in ".dma.bss.bluetooth.dtm";}
static CELLS: ControllerEventCells<X> =
    ControllerEventCells::new(&irq::SCHEDULER_SIGNAL, &irq::TIMER_SIGNAL);

static RESULT: OnceValue<bool> = OnceValue::new();
struct Boot {
    actor: Option<Actor>,
    inbox: Option<ClientInbox<Bluetooth>>,
    cold: Option<ColdOwner>,
    coex: Option<CoexAbsent<BluetoothClient>>,
    cell: Option<MembershipCell<Bluetooth>>,
    bell: Option<irq::SchedulerBell>,
    timer: Option<&'static mut irq::TimerPort>,
    routes: Option<irq::Inactive>,
    profile: Profile,
    address: [u8; 6],
}
static BOOT: StaticCell<Boot> = StaticCell::new();
type LiveActor<'loan, 'p, 'e> = BluetoothActor<
    'loan, RcSlow, Shared, EmbassyClock, BluetoothAttachment<lifecycle::Life<'p, 'e>>,
>;
type LiveRunner<'p, 'e> = Runner<'p, 'e, X, irq::SchedulerBell, EmbassyClock, NoClockRequests>;
mod active;
mod installed;

struct NoClockRequests;
impl ClockRequests for NoClockRequests {
    type Request = Infallible;
    fn try_request(&self) -> Option<Infallible> {
        None
    }
    fn poll_request(&self, _: &mut Context<'_>) -> Poll<Infallible> {
        Poll::Pending
    }
    fn answer(never: Infallible, _: Result<oer_bluetooth_le_radio::LeInstant, Fault>) {
        match never {}
    }
}

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
    let (inbox, cell) = bluetooth::claim().expect("one BLE membership");
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
    let (bell, timer) = irq::initialize(SoftwareInterrupt::new(p.FROM_CPU_INTR3), tokens.doorbell);
    let radio = RADIO_EXECUTOR
        .init(X::new(SoftwareInterrupt::new(p.FROM_CPU_INTR2)))
        .start(tokens.radio, LEVELS);
    let boot = BOOT.init_with(|| Boot {
        actor: Some(actor),
        inbox: Some(inbox),
        cold: Some(ColdOwner::from_partition(parts.bluetooth)),
        coex: Some(parts.bluetooth_coex),
        cell: Some(cell),
        bell: Some(bell),
        timer: Some(timer),
        routes: Some(irq::Inactive::new(tokens.primary, tokens.nrt, tokens.timer)),
        profile,
        address: base,
    });
    radio.spawn(driver(boot).expect("one P2 driver task"));
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
async fn hold<T>(owner: &mut T) -> ! {
    core::hint::black_box(owner);
    let _ = RESULT.set(false);
    pending().await
}
mod task {
    #[embassy_executor::task]
    pub(super) async fn driver(boot: &'static mut super::Boot) {
        super::run_driver(boot).await;
    }
}
use task::driver;
async fn run_driver(boot: &'static mut Boot) {
    with_pool_async(async |pool| {
        with_epoch_async(async |epoch| {
            installed::run(boot, pool, epoch).await;
        })
        .await;
    })
    .await;
}
async fn hold_success<A, B>(a: &mut A, b: &mut B) {
    core::hint::black_box((a, b));
    pending::<()>().await
}
