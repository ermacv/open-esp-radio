//! Permanent physical P2 worker. Only ordinary graph return admits another epoch.
use super::*;
use core::future::{pending, poll_fn};
use embassy_futures::select::{Either, select};
use esp_hal::{efuse, usb::usb_serial_jtag::UsbSerialJtag};
use oer_bluetooth_le_port::TxPower;
use oer_bluetooth_le_radio::LeBand;
use oer_esp32s31_bluetooth_driver::{
    modem_timer::{
        ControllerModemTimerBegin, ControllerModemTimerReadinessClass, ControllerModemTimerRearm,
        ControllerModemTimerStep, ControllerModemTimerTask,
    },
    port::{
        Fault, mailbox as rpc,
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
use oer_esp32s31_radio_system::{BluetoothActor, BluetoothAttachment, Esp32s31Radio, RadioSystem};
use oer_espressif_esp_hal_radio::{EspHalClocks, RadioSingletons, take_radio};
use oer_radio_regulatory::{Country, PowerLimits, RegulatoryDomain};
use oer_time::{Clock, Duration, Timer};

type Shared = Esp32s31Radio<
    EspHalClocks,
    oer_espressif_executor::Cooperative<EmbassyClock>,
    NoopPhyTargetObserver,
>;
type Actor = RadioSystem<Wifi, RcSlow, Shared, EmbassyClock>;
type Arena = BluetoothArena<0, 0, 0, 0, 0, 0>;
mod lifecycle;
oer_esp32s31_hal::dma_static! {static ARENA:Arena=zeroed in ".dma.bss.bluetooth.dtm";}
static CELLS: ControllerEventCells<X> =
    ControllerEventCells::new(&irq::SCHEDULER_SIGNAL, &irq::TIMER_SIGNAL);

struct Boot {
    actor: Option<Actor>,
    inbox: Option<ClientInbox<Bluetooth>>,
    cold: Option<ColdOwner>,
    coex: Option<CoexAbsent<BluetoothClient>>,
    cell: Option<MembershipCell<Bluetooth>>,
    bell: &'static irq::SchedulerBell,
    timer: Option<&'static mut irq::TimerPort>,
    routes: Option<irq::Inactive>,
    arena: Option<Placed<Arena>>,
    port_epoch: u64,
    reply: Option<(rpc::Command, bridge::Response)>,
    profile: Profile,
    address: [u8; 6],
}
static BOOT: StaticCell<Boot> = StaticCell::new();
type LiveActor<'loan, 'p, 'e> = BluetoothActor<
    'loan,
    RcSlow,
    Shared,
    EmbassyClock,
    BluetoothAttachment<lifecycle::Life<'p, 'e>>,
>;
type LiveRunner<'p, 'e> = Runner<'p, 'e, X, SchedulerAccess, EmbassyClock, bridge::RuntimeClock>;
mod active;
mod installed;

static SCHEDULER: StaticCell<irq::SchedulerBell> = StaticCell::new();
struct SchedulerAccess(&'static irq::SchedulerBell);
impl oer_esp32s31_hal::bluetooth::engine::scheduler::interrupt::SchedulerDoorbell
    for SchedulerAccess
{
    fn ring(&self) {
        oer_esp32s31_hal::bluetooth::engine::scheduler::interrupt::SchedulerDoorbell::ring(self.0);
    }
    fn poll_reply(
        &self,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<
        Result<
            (),
            oer_esp32s31_hal::bluetooth::engine::scheduler::interrupt::SchedulerInterruptError,
        >,
    > {
        oer_esp32s31_hal::bluetooth::engine::scheduler::interrupt::SchedulerDoorbell::poll_reply(
            self.0, cx,
        )
    }
}
pub fn permission(profile: Profile) -> (Permission, TxPower) {
    let domain = RegulatoryDomain::of(Country::new(profile.country).expect("build country"))
        .expect("profile country in database");
    let band = domain.permit(LeBand).expect("profile permits LE band");
    let permission = Permission::new(
        band,
        PowerLimits {
            antenna_gain_mbm: 0,
            chip_maximum_mbm: 2100,
            owner_cap_mbm: Some(i32::from(profile.cap_dbm) * 100),
        },
    );
    let power = TxPower::within(
        oer_bluetooth_le_radio::LeTxCeiling::new(band, permission.ceiling()),
        profile.cap_dbm,
    )
    .expect("configured power within its ceiling");
    permission
        .tx_power(Operation::DtmTx, power)
        .expect("profile permits DTM TX");
    (permission, power)
}

pub fn boot(handoff: oer_espressif_staged_runtime::Handoff) -> ! {
    let previous = oer_espressif_staged_runtime::panic::take_previous();
    let p = esp_hal::init(esp_hal::Config::default());
    let (psram, adopted) = handoff.adopt(INTERRUPT_TABLE, board::BootstrapPsram(p.PSRAM));
    let mut console = UsbSerialJtag::new(oer_espressif_usb_fifo::hand_over(p.USB_DEVICE));
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
        oer_espressif_executor::Cooperative::new(EmbassyClock),
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
    let bell = SCHEDULER.init(bell);
    let _ = bridge::clock_state();
    let radio = RADIO_EXECUTOR
        .init(X::new(SoftwareInterrupt::new(p.FROM_CPU_INTR2)))
        .start(tokens.radio, LEVELS);
    let boot = BOOT.init_with(|| Boot {
        actor: Some(actor),
        inbox: Some(inbox),
        cold: Some(ColdOwner::from_partition(parts.bluetooth)),
        coex: Some(parts.bluetooth_coex),
        cell: Some(cell),
        bell,
        timer: Some(timer),
        routes: Some(irq::Inactive::new(tokens.primary, tokens.nrt, tokens.timer)),
        arena: Some(ARENA.take().expect("one static arena")),
        port_epoch: 0,
        reply: None,
        profile,
        address: base,
    });
    radio.spawn(driver(boot).expect("one P2 driver task"));
    radio.spawn(bridge::supervisor().expect("one P2 port supervisor"));
    let app = EXECUTOR.init(Executor::<0>::new(SoftwareInterrupt::new(p.FROM_CPU_INTR0)));
    adopted.enable_interrupts();
    app.run(tokens.wake, |spawner| {
        let (_, power) = permission(profile);
        spawner
            .spawn(host::serve(console, p.RNG, bridge::frontend(), power).expect("one HCI task"));
    })
}
async fn hold<T>(owner: &mut T) -> ! {
    core::hint::black_box(owner);
    irq::FAULT.store(true, core::sync::atomic::Ordering::Release);
    bridge::fail_clock(Fault::Hardware);
    pending().await
}
async fn fail<T>(owner: &mut T, reply: &mut Option<(rpc::Command, bridge::Response)>) -> ! {
    bridge::complete(reply, Err(Fault::Hardware));
    hold(owner).await
}
async fn fault<T>(owner: &mut T, reply: &mut Option<(rpc::Command, bridge::Response)>, cause: Fault) -> ! {
    bridge::complete(reply, Err(cause));
    bridge::fail_clock(cause);
    hold(owner).await
}
mod task {
    #[embassy_executor::task]
    pub(super) async fn driver(boot: &'static mut super::Boot) {
        super::WORKER.take().future(|| super::worker(boot)).await;
    }
}
use task::driver;
type Worker = impl core::future::Future<Output = ()>;
static WORKER: static_cell::ConstStaticCell<oer_memory::StaticPlace<Worker>> =
    static_cell::ConstStaticCell::new(oer_memory::StaticPlace::new());
#[define_opaque(Worker)]
fn worker(boot: &'static mut Boot) -> Worker {
    async move {
        let timer: &'static irq::TimerPort = boot.timer.take().expect("one P2 timer handover");
        loop {
            let (command, reply) = bridge::command().await;
            if command.operation != rpc::Operation::Enable
                || boot.port_epoch.checked_add(1) != Some(command.epoch)
            {
                bridge::answer(command, reply, Err(Fault::Accounting));
                hold(boot).await;
            }
            boot.port_epoch = command.epoch;
            boot.reply = Some((command, reply));
            {
                let mut pool = core::pin::pin!(with_pool_async(async |pool| {
                    let mut epoch = core::pin::pin!(with_epoch_async(async |epoch| {
                        let mut installed =
                            core::pin::pin!(installed::run(boot, pool, epoch, timer));
                        installed.as_mut().await;
                    }));
                    epoch.as_mut().await;
                }));
                pool.as_mut().await;
            }
            // Every scoped future and its old generative authority ended;
            // the actual arena and cold owners are back in Boot.
            bridge::complete(&mut boot.reply, Ok(rpc::Completion::Done));
        }
    }
}
