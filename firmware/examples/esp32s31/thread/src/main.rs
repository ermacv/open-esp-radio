#![no_main]
#![no_std]

//! A Thread end device on the ESP32-S31 IEEE 802.15.4 radio.
//!
//! The IEEE 802.15.4 client starts on the shared radio system, the radio
//! system runs the periodic PHY tracking, and OpenThread runs over the
//! client's runtime through the OpenThread radio adapter. The device joins
//! the network of the active dataset as a minimal end device and echoes UDP
//! datagrams on port 1212.

use core::net::{Ipv6Addr, SocketAddrV6};
use core::pin::pin;

use esp_hal::{
    clock::CpuClock,
    efuse,
    interrupt::software::SoftwareInterrupt,
    rng::{Trng, TrngSource},
    timer::{OneShotTimer, timg::TimerGroup},
};
use log::{error, info};
use oer::systems::esp32s31::embassy::ieee802154::{
    self as ieee802154, Esp32s31Ieee802154, EspHalRadioPlatform, Ieee802154CoexConfig,
    Ieee802154CoexLevel, Ieee802154Parked, Ieee802154PibDefaults, Ieee802154System,
    Ieee802154SystemBackend, Ieee802154SystemClock,
    openthread::{
        MonotonicRadioClock, OPEN_THREAD_RADIO_CAPABILITIES, OpenThreadRadio, PendingSlots,
        PortRssi,
        frames::{RoleCoexPriority, role_txrx_priority},
    },
    owner::{Ieee802154Clients, Ieee802154Setup},
    port::{Disabled, Ended, Owner, PortBackend, PortBody, run_port},
    start,
};
use oer::systems::esp32s31::embassy::radio::{self as shared_radio, RadioStart, SharedRadio};
use oer_espressif_executor_embassy::{self as platform_executor, Executor};
use openthread::{OpenThread, OtResources, OtUdpResources, SimpleRamSettings, UdpSocket};
use static_cell::{ConstStaticCell, StaticCell};

use tinyrlibc as _;

// The image's peripheral interrupt sources (`oer_espressif_staged_runtime::interrupts`).
oer_espressif_staged_runtime::interrupt_table! {
    /// Wakes the core-0 Embassy executor.
    wake: ExecutorWake = FROM_CPU_INTR0 => oer_espressif_executor_embassy::wake_handler::<0>, Priority1, ProCpu;
    /// The Embassy time driver's alarm (TIMG0 timer 0).
    alarm: TimeAlarm = TG0_T0_LEVEL => oer_espressif_executor_embassy::timer_interrupt, Priority1, ProCpu;
    /// The IEEE 802.15.4 MAC's interrupt.
    ieee802154: Ieee802154Mac = MODEM_ZB_MAC => oer::systems::esp32s31::embassy::ieee802154::ieee802154_interrupt, Priority1, ProCpu;
}

/// Frames OpenThread has not taken yet while it transmits or scans.
const RX_QUEUE: usize = 8;

/// The single-interface client's port backend.
type Backend = Ieee802154SystemBackend<Esp32s31Ieee802154>;

/// The live RSSI OpenThread reads synchronously, over the runtime's reader.
type RadioRssi = ieee802154::Ieee802154SystemRssi;

const UDP_PORT: u16 = 1212;
const UDP_BUFFER: usize = 1280;
const UDP_SOCKETS: usize = 2;

/// The CSL period in microseconds, from the build environment: with one,
/// the device runs as a synchronized sleepy end device that samples its
/// parent's channel in CSL windows; without one, as a minimal end device.
const THREAD_CSL_PERIOD_US: Option<&str> = option_env!("THREAD_CSL_PERIOD_US");

/// The active operational dataset as TLV hex, from the build environment.
const THREAD_DATASET: &str = match option_env!("THREAD_DATASET") {
    Some(dataset) => dataset,
    None => "",
};

static EXECUTOR: StaticCell<Executor<0>> = StaticCell::new();
// The entropy source must outlive every `Trng` OpenThread draws from.
static TRNG_SOURCE: StaticCell<TrngSource<'static>> = StaticCell::new();
static TRNG: StaticCell<Trng> = StaticCell::new();
static SYSTEM: StaticCell<Ieee802154System> = StaticCell::new();
static OT_RESOURCES: StaticCell<OtResources> = StaticCell::new();
static OT_UDP: StaticCell<OtUdpResources<UDP_SOCKETS, UDP_BUFFER>> = StaticCell::new();
static OT_SETTINGS_BUFFER: ConstStaticCell<[u8; 1024]> = ConstStaticCell::new([0; 1024]);
static OT_SETTINGS: StaticCell<SimpleRamSettings> = StaticCell::new();
static RADIO_CLOCK: StaticCell<MonotonicRadioClock<&'static Ieee802154SystemClock>> =
    StaticCell::new();
static RSSI_READER: StaticCell<RadioRssi> = StaticCell::new();
static RADIO_RSSI: StaticCell<PortRssi<'static, RadioRssi>> = StaticCell::new();
static UDP_RECEIVE: ConstStaticCell<[u8; UDP_BUFFER]> = ConstStaticCell::new([0; UDP_BUFFER]);

/// The IEEE 802.15.4 EUI-64 as ESP-IDF derives it
/// (`esp_read_mac(ESP_MAC_IEEE802154)`): the base MAC's first three bytes,
/// the eFuse MAC extension, then the base MAC's last three bytes.
fn ieee_eui64() -> [u8; 8] {
    let base = efuse::base_mac_address();
    let base = base.as_bytes();
    let extension = efuse::read_field_le::<u16>(efuse::MAC_EXT).to_le_bytes();
    [
        base[0],
        base[1],
        base[2],
        extension[0],
        extension[1],
        base[3],
        base[4],
        base[5],
    ]
}

#[unsafe(no_mangle)]
extern "C" fn runtime_main() -> ! {
    esp_println::logger::init_logger_from_env();
    if let Some(panic) = oer_espressif_staged_runtime::panic::take_previous() {
        esp_println::println!("open-radio: the previous boot panicked at {panic}");
    }
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    // SAFETY: the common stage-two entry runs after the board bootstrap,
    // with global interrupts disabled and the PSRAM mapping intact.
    let _psram =
        unsafe { oer_esp32s31_platform_board::adopt_psram(peripherals.PSRAM, INTERRUPT_TABLE) };

    let timer_group = TimerGroup::new(peripherals.TIMG0);
    let interrupts = Interrupts::take().expect("the image takes its interrupt tokens once");
    assert!(
        ieee802154::install_interrupt_route(ieee802154::Ieee802154InterruptSource::new(
            interrupts.ieee802154
        ))
        .is_ok(),
        "the image installs its IEEE 802.15.4 route once"
    );
    platform_executor::init(OneShotTimer::new(timer_group.timer0), interrupts.alarm);
    TRNG_SOURCE.init(TrngSource::new(peripherals.RNG));
    let trng = Trng::try_new().expect("ESP32-S31 TRNG must have a unique owner");
    let platform = EspHalRadioPlatform::new(
        peripherals.MODEM_SYSCON,
        peripherals.MODEM_LPCON,
        peripherals.HP_SYS_CLKRST,
        peripherals.PMU,
        peripherals.LP_AON_CLK_RST,
        peripherals.LP_PERI,
        peripherals.LP_TSENS,
        peripherals.I2C_ANA_MST,
    );
    let executor = EXECUTOR.init(Executor::<0>::new(SoftwareInterrupt::new(
        peripherals.FROM_CPU_INTR0,
    )));
    // SAFETY: timer and executor handlers are now bound on CPU0, and the staged
    // handoff has kept MIE clear since `adopt_psram`.
    unsafe { oer_espressif_staged_runtime::enable_interrupts_after_handoff() };
    executor.run(interrupts.wake, |spawner| {
        spawner.spawn(
            thread_task(spawner, platform, trng)
                .expect("thread task storage must be available once"),
        );
    })
}

#[embassy_executor::task]
async fn thread_task(
    spawner: embassy_executor::Spawner,
    platform: EspHalRadioPlatform,
    trng: Trng,
) {
    let (radio, partitions) = shared_radio::start(spawner, platform, RadioStart::new())
        .expect("the shared radio starts once");
    let defaults = Ieee802154PibDefaults::default();
    let parked = Ieee802154Parked::new(partitions.ieee802154, partitions.ieee802154_coex, defaults)
        .expect("the IEEE 802.15.4 engine frames are taken once");
    // Bring-up registers and calibrates the shared PHY; pin its future.
    let started = {
        let started = pin!(start(radio, parked, defaults));
        started.await
    };
    let Ok((system, backend)) = started else {
        error!("the IEEE 802.15.4 client did not start");
        return;
    };
    let system = SYSTEM.init(system);
    let rssi = RSSI_READER.init(system.runtime().recent_rssi_reader());

    let ot_settings = OT_SETTINGS.init(SimpleRamSettings::new(OT_SETTINGS_BUFFER.take()));
    // OpenThread's `SubMac` reads the radio capabilities when the instance
    // is built: transmit security then stays with the radio.
    let ot_resources = OT_RESOURCES.init(OtResources::new());
    ot_resources.set_radio_caps(OPEN_THREAD_RADIO_CAPABILITIES);
    let ot = OpenThread::new_with_udp(
        ieee_eui64(),
        TRNG.init(trng),
        ot_settings,
        // OpenThread reads the port's own clock and live RSSI.
        RADIO_CLOCK.init(MonotonicRadioClock::new(system.runtime().clock())),
        Some(RADIO_RSSI.init(PortRssi::new(rssi))),
        ot_resources,
        OT_UDP.init(OtUdpResources::new()),
    )
    .expect("OpenThread must initialize once");
    // OpenThread runs in the port's body for the image's life; the port's
    // driver runs the CSMA-CA backoffs beside it.
    spawner.spawn(openthread_task(ot.clone(), backend).expect("OpenThread task storage"));
    spawner.spawn(role_task(ot.clone(), system, radio).expect("role task storage"));

    if THREAD_DATASET.is_empty() {
        error!("build with THREAD_DATASET set to the active operational dataset TLV hex");
        return;
    }
    ot.set_active_dataset_tlv_hexstr(THREAD_DATASET)
        .expect("THREAD_DATASET must be a valid dataset");
    // An end device without the router role, with stable network data only:
    // minimal (receiver on when idle), or synchronized sleepy with a CSL
    // period, sampling in the windows its radio schedules.
    let csl_period = THREAD_CSL_PERIOD_US.map(|period| {
        period
            .parse::<u32>()
            .expect("THREAD_CSL_PERIOD_US must be a period in microseconds")
    });
    ot.set_link_mode(csl_period.is_none(), false, false)
        .expect("the MTD link mode must be accepted");
    if let Some(period) = csl_period {
        ot.set_csl_period(period)
            .expect("THREAD_CSL_PERIOD_US must be a valid CSL period");
        info!("CSL period {period} us");
    }
    ot.enable_ipv6(true).expect("IPv6 must come up");
    ot.enable_thread(true).expect("Thread must start");

    let socket = UdpSocket::bind(
        ot,
        &SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, UDP_PORT, 0, 0),
    )
    .expect("the UDP port must be free");
    info!("echoing UDP on port {UDP_PORT}");
    let buffer = UDP_RECEIVE.take();
    loop {
        let Ok((length, local, remote)) = socket.recv(buffer).await else {
            continue;
        };
        info!("{length} bytes from {remote}");
        if socket
            .send(&buffer[..length], Some(&local), &remote)
            .await
            .is_err()
        {
            error!("the echo to {remote} failed");
        }
    }
}

/// The port body OpenThread runs in: configure interface 0, enable the
/// radio and lend its running phase to the OpenThread radio, for the
/// image's life.
struct ThreadBody {
    ot: OpenThread<'static>,
}

impl PortBody<Backend> for ThreadBody {
    async fn run<'p, 'id>(
        self,
        mut owner: Owner<'p, 'id, Backend, Disabled>,
        resources: <Backend as PortBackend>::Resources<'id>,
    ) -> Ended<'id, Backend> {
        let interface = resources.interfaces;
        let zero = owner.setup().configure(interface.interface);
        let [seat, ..] = resources.operation;
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(failed) => {
                error!("the IEEE 802.15.4 radio did not enable");
                return failed.fail_stop();
            }
        };
        let parts = owner.split();
        let mut events = parts.events;
        let mut control = parts.control;
        let client = parts.clients.client(&zero);
        let radio: OpenThreadRadio<'_, '_, _, _, _, RX_QUEUE, _> = OpenThreadRadio::new(
            &mut *events,
            &mut *control,
            client,
            seat,
            PendingSlots {
                short: interface.short,
                extended: interface.extended,
            },
        );
        // The stack's radio, alarm and tasklet loops share one large future.
        match pin!(self.ot.run(radio)).await {}
    }
}

#[embassy_executor::task]
async fn openthread_task(ot: OpenThread<'static>, backend: Backend) {
    let _ = pin!(run_port(backend, ThreadBody { ot })).await;
    error!("the IEEE 802.15.4 port ended");
}

/// Log the device's role and addresses when OpenThread's state changes, and
/// set the IEEE 802.15.4 coexistence level of a role change as ESP-IDF's
/// `handle_ot_role_change` does.
#[embassy_executor::task]
async fn role_task(
    ot: OpenThread<'static>,
    system: &'static mut Ieee802154System,
    radio: &'static SharedRadio,
) -> ! {
    let mut role = None;
    loop {
        ot.wait_changed().await;
        let current = ot.device_role();
        if role != Some(current) {
            role = Some(current);
            let txrx = match role_txrx_priority(ot.rx_on_when_idle()) {
                RoleCoexPriority::Low => Ieee802154CoexLevel::Low,
                RoleCoexPriority::Middle => Ieee802154CoexLevel::Middle,
            };
            let config = Ieee802154CoexConfig {
                txrx,
                ..system.coex_config()
            };
            system.update_coexistence(radio, config).await;
        }
        info!("role {:?}, rloc16 {:#06x}", current, ot.rloc16());
        let _ = ot.ipv6_addrs(|address| {
            if let Some((address, prefix)) = address {
                info!("address {address}/{prefix}");
            }
            Ok(())
        });
    }
}
