mod future;
pub(crate) mod irq;
mod network;
mod radio;
mod station;
use crate::transport::CONSOLE;
use oer_hil_protocol::{
    RequestIdentity,
    wifi::{StationLink, StationLinkState, StationStart},
};
use oer_ieee80211_rsn::{PmkDerivation, nonce::SNonceSource, sae::SaePassword};
use oer_time::Clock;

use esp_hal::{efuse, interrupt::software::SoftwareInterrupt};
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_board as board;
use oer_esp32s31_phy::{
    NoopPhyTargetObserver, PhyCalibrationIdentity, PhyRegisterConfig, concurrent::ConcurrentPhy,
    phy_get_rf_cal_version,
};
use oer_esp32s31_radio_client::{LpClock, RcSlow, claim};
use oer_esp32s31_radio_system::{Esp32s31Radio, RadioSystem};
use oer_espressif_esp_hal_radio::{EspHalClocks, RadioSingletons, take_radio};
use oer_espressif_executor::{InterruptExecutor, LevelOrder};

use static_cell::StaticCell;

#[derive(Clone, Copy)]
pub struct RadioConfig {
    pub channel: u8,
    pub country: [u8; 2],
    pub cap_dbm: i8,
}
// The laboratory image's regulatory installation; no credentials are built in.
const CONFIG: RadioConfig = RadioConfig {
    channel: 1,
    country: *b"DE",
    cap_dbm: 10,
};

static RADIO: StaticCell<InterruptExecutor<2>> = StaticCell::new();

const LEVELS: LevelOrder<crate::WifiMacToken, crate::RadioToken, crate::ExecutorWake> =
    LevelOrder::new();
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

pub(crate) fn start(handoff: oer_espressif_staged_runtime::Handoff) -> ! {
    crate::early(&oer_diagnostics::Note::runtime_start(crate::PROFILE_NAME));
    crate::fatal::report_previous();
    crate::validate_runtime_layout();
    crate::system::take_platform_panic();
    let peripherals =
        esp_hal::init(esp_hal::Config::default().with_cpu_clock(esp_hal::clock::CpuClock::max()));
    let (psram, adopted) = handoff.adopt(
        crate::INTERRUPT_TABLE,
        board::BootstrapPsram(peripherals.PSRAM),
    );
    crate::exception::report_stack_guard();
    let config = CONFIG;
    let interrupts = crate::Interrupts::take().expect("station interrupt tokens taken once");
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
            crate::early(&oer_diagnostics::Note::runtime_fail("take-radio"));
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
    oer_espressif_executor::bind::<0, _>(interrupts.alarm);
    let bell = irq::ImageDoorbell::new(
        SoftwareInterrupt::new(peripherals.FROM_CPU_INTR3),
        interrupts.doorbell,
    );
    let radio = RADIO
        .init(InterruptExecutor::<2>::new(SoftwareInterrupt::new(
            peripherals.FROM_CPU_INTR2,
        )))
        .start(interrupts.radio, LEVELS);
    let mut rng = esp_hal::rng::TrngSource::new(peripherals.RNG);
    if !rng.status().is_healthy() {
        crate::fail("entropy-source");
    }
    let nonces = SNonceSource::from_entropy(|seed| rng.read(seed).expect("station LP TRNG seed"));
    let mut boot_bytes = [0; 8];
    rng.read(&mut boot_bytes).expect("station boot entropy");
    let seed = u64::from_le_bytes(boot_bytes);
    let boot = seed.max(1);
    let mut tx_bytes = [0; 4];
    rng.read(&mut tx_bytes).expect("station TX entropy");
    let entropy = u32::from_le_bytes(tx_bytes);
    CONSOLE.start(boot);
    let (driver_channel, frontend_channel) = radio::channels();
    if let Ok(task) = radio_actor(actor) {
        radio.spawn(task);
    }
    if let Ok(task) = driver(
        radio::Boot {
            cell: cells.wifi,
            wifi: partitions.wifi,
            rng,
            station,
            ap,
            config,
            routes: irq::ImageRoutes {
                mac: interrupts.wifi_mac,
                power: interrupts.wifi_pwr,
            },
            bell,
            entropy,
        },
        driver_channel,
    ) {
        radio.spawn(task);
    }
    let executor = crate::EXECUTOR.init(crate::Executor::<0>::new(SoftwareInterrupt::new(
        peripherals.FROM_CPU_INTR0,
    )));
    let (usb, _pads) = crate::transport::Usb::new(peripherals.USB_DEVICE, interrupts.console_usb);
    adopted.enable_interrupts();
    executor.run(interrupts.wake, move |spawner| {
        spawner.spawn(console(usb, boot).expect("station console task"));
        spawner
            .spawn(station_task(frontend_channel, station, seed, nonces).expect("station P1 task"));
    })
}

#[embassy_executor::task]
async fn radio_actor(actor: &'static mut Actor) {
    match actor.run().await {}
}
fn park() -> ! {
    oer_espressif_staged_runtime::idle()
}

#[derive(Clone, Copy)]
pub struct Attempt {
    pub request: RequestIdentity,
}
static EPOCH: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
impl Attempt {
    fn advance(self) {
        use core::sync::atomic::Ordering;
        EPOCH
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |epoch| {
                epoch.checked_add(1)
            })
            .expect("station epoch exhausted; reset required");
    }
    pub fn publish(self, state: StationLinkState) {
        CONSOLE.publish(
            self.request.session_id,
            self.request.request_id,
            &StationLink {
                epoch: EPOCH.load(core::sync::atomic::Ordering::Relaxed),
                state,
            },
        );
    }
}

type Starts = embassy_sync::channel::Channel<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    (RequestIdentity, StationStart),
    1,
>;
static STARTS: Starts = Starts::new();
oer_hil_agent::requests! {
    pub enum Requests (sessions = false) { Start(oer_hil_protocol::wifi::StartStation) }
}
#[embassy_executor::task]
async fn console(usb: crate::transport::Usb, boot: u64) {
    use oer_hil_protocol::base::{Accepted, RejectReason, Rejected};
    let mut configured = false;
    crate::transport::serve(
        usb,
        boot,
        0,
        move |request, Requests::Start(start): Requests| {
            let reason = if configured {
                Some(RejectReason::InvalidState)
            } else if start.0.credentials.validate().is_err() {
                Some(RejectReason::InvalidConfiguration)
            } else if STARTS.try_send((request, start.0)).is_err() {
                Some(RejectReason::Busy)
            } else {
                configured = true;
                None
            };
            if let Some(reason) = reason {
                CONSOLE.publish(request.session_id, request.request_id, &Rejected(reason));
            } else {
                CONSOLE.publish(request.session_id, request.request_id, &Accepted);
            }
        },
    )
    .await
}

#[embassy_executor::task]
async fn driver(boot: radio::Boot, channel: radio::DriverChannel) {
    radio::drive(boot, channel).await;
}
type StationTaskFuture = impl core::future::Future<Output = ()>;
static STATION_TASK: embassy_executor::raw::TaskStorage<StationTaskFuture> =
    embassy_executor::raw::TaskStorage::new();
fn station_task(
    channel: radio::FrontendChannel,
    address: [u8; 6],
    seed: u64,
    nonces: SNonceSource,
) -> Result<embassy_executor::SpawnToken<impl Sized>, embassy_executor::SpawnError> {
    STATION_TASK.spawn(move || station_task_future(channel, address, seed, nonces))
}
#[define_opaque(StationTaskFuture)]
fn station_task_future(
    channel: radio::FrontendChannel,
    address: [u8; 6],
    seed: u64,
    nonces: SNonceSource,
) -> StationTaskFuture {
    async move {
        let (request, mut start) = STARTS.receive().await;
        let attempt = Attempt { request };
        attempt.advance();
        let ssid = heapless::Vec::from_slice(start.credentials.ssid()).expect("decoded SSID fits");
        let password = SaePassword::new(start.credentials.passphrase())
            .expect("decoded personal password fits");
        let mut derive =
            PmkDerivation::new(start.credentials.passphrase(), start.credentials.ssid())
                .expect("decoded personal credentials fit");
        // All retained copies now have their own zeroizing owner. Erase the
        // received command before the first cooperative cancellation point.
        start.credentials.clear_passphrase();
        drop(start);
        let begin = EmbassyClock.now();
        let pmk = loop {
            if let Some(pmk) = derive.step() {
                break pmk;
            }
            embassy_futures::yield_now().await;
        };
        let elapsed = EmbassyClock
            .now()
            .as_micros()
            .saturating_sub(begin.as_micros());
        CONSOLE
            .note_reliably(&oer_diagnostics::Note::station_pmk(elapsed))
            .await;
        radio::serve(
            channel,
            station::Body {
                address,
                seed,
                ssid,
                password,
                pmk,
                nonces,
                attempt,
            },
        )
        .await;
    }
}

use oer_ieee80211_arbiter::{
    ArbiterPolicy, ArbiterPolicyError, CheckedArbiterPolicy, NoReservation,
};
use oer_time::Duration;
pub fn arbiter(
    publication_bound: Duration,
) -> Result<CheckedArbiterPolicy<NoReservation>, ArbiterPolicyError> {
    let mut policy = ArbiterPolicy::DEFAULT;
    let absence = publication_bound
        .as_micros()
        .checked_add(policy.retune.as_micros())
        .and_then(|one| one.checked_mul(2))
        .and_then(|moves| moves.checked_add(policy.min_listen.as_micros()))
        .ok_or(ArbiterPolicyError::AbsenceBudgetOverflow)?;
    policy.absence = Duration::from_micros(absence);
    policy.checked(publication_bound)
}
