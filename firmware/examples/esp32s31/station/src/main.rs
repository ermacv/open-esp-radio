#![no_main]
#![no_std]
#![recursion_limit = "256"]

#[cfg(not(feature = "owned-network"))]
compile_error!("owned Xarxa is the only network integration: enable the owned-network feature");

use core::num::NonZeroU16;

use embassy_executor::Spawner;

#[cfg(feature = "owned-network")]
use embassy_net_owned as embassy_net;

use esp_hal::{
    clock::CpuClock,
    interrupt::software::SoftwareInterrupt,
    rng::{Trng, TrngSource},
    timer::{OneShotTimer, timg::TimerGroup},
};

use oer::wifi::{
    Preference, StaReconnectPolicy, StationRequest, StationScanChannels, StationScanPolicy,
    StationSecurity, WifiChannel, WifiScanRequest, WifiSsid,
};

use oer_espressif_executor_embassy::{self as platform_executor, Executor};

use oer::systems::esp32s31::embassy::{
    radio::{self as shared_radio, ConcurrentPartitions, RadioStart},
    wifi::{
        self as integration, DeadlineBudget, DeadlineWatchdog, EspHalRadioPlatform,
        EspHalWifiPlatform, RadioConfig, WatchdogConfig, WifiParts, WifiStarted,
    },
};

use static_cell::StaticCell;

// The image's peripheral interrupt sources (`oer_espressif_staged_runtime::interrupts`).
oer_espressif_staged_runtime::interrupt_table! {
    /// Wakes the core-0 Embassy executor.
    wake: ExecutorWake = FROM_CPU_INTR0 => oer_espressif_executor_embassy::wake_handler::<0>, Priority1, ProCpu;
    /// The Embassy time driver's alarm (TIMG0 timer 0).
    alarm: TimeAlarm = TG0_T0_LEVEL => oer_espressif_executor_embassy::timer_interrupt, Priority1, ProCpu;
    /// The Wi-Fi MAC's interrupt.
    wifi_mac: WifiMac = MODEM_WIFI_MAC => oer::systems::esp32s31::embassy::wifi::mac_interrupt, Priority1, ProCpu;
    /// The Wi-Fi power interrupt.
    wifi_power: WifiPower = MODEM_WIFI_PWR => oer::systems::esp32s31::embassy::wifi::power_interrupt, Priority1, ProCpu;
}

static EXECUTOR: StaticCell<Executor<0>> = StaticCell::new();
// The shared radio outlives every client and its periodic PHY tracking task.
// The entropy source owns RNG hardware for the entire process. It must not be
// dropped while the radio keeps the nested `Trng` owner across await points.
static TRNG_SOURCE: StaticCell<TrngSource<'static>> = StaticCell::new();
// Socket/IP state belongs to the application, not to the radio driver. Static
// placement avoids moving the stack arena through the executor task frame.
mod network;

const STA_SSID: &str = match option_env!("ESP32S31_WIFI_SSID") {
    Some(value) => value,
    None => "",
};
const STA_PASSPHRASE: &str = match option_env!("ESP32S31_WIFI_PASSPHRASE") {
    Some(value) => value,
    None => "",
};

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

    static WATCHDOG: StaticCell<DeadlineWatchdog> = StaticCell::new();
    let watchdog = WATCHDOG.init(DeadlineWatchdog::new(peripherals.TIMG1));
    let timer_group = TimerGroup::new(peripherals.TIMG0);
    let interrupts = Interrupts::take().expect("the image takes its interrupt tokens once");
    platform_executor::init(OneShotTimer::new(timer_group.timer0), interrupts.alarm);

    TRNG_SOURCE.init(TrngSource::new(peripherals.RNG));
    let trng = Trng::try_new().expect("ESP32-S31 TRNG must have a unique owner");
    let wifi_platform =
        EspHalWifiPlatform::new(peripherals.WIFI, interrupts.wifi_mac, interrupts.wifi_power);
    let radio_platform = EspHalRadioPlatform::new(
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
        let task = station_task(spawner, radio_platform, wifi_platform, trng, watchdog)
            .expect("station task storage must be available once");
        spawner.spawn(task);
    })
}

#[embassy_executor::task]
async fn station_task(
    spawner: Spawner,
    radio_platform: EspHalRadioPlatform,
    wifi_platform: EspHalWifiPlatform,
    trng: Trng,
    watchdog: &'static DeadlineWatchdog,
) {
    let ssid = WifiSsid::new(STA_SSID.as_bytes()).expect("station SSID must be valid");
    let security = StationSecurity::wpa2_personal(STA_PASSPHRASE.as_bytes(), ssid.as_bytes())
        .expect("station credentials must be valid");
    let request = StationRequest::new(
        ssid,
        security,
        StaReconnectPolicy::new(3, 100, 1_000, 100)
            .expect("station reconnect policy must be valid"),
        StationScanPolicy::new(
            StationScanChannels::CHANNELS_1_TO_13,
            NonZeroU16::new(200).expect("scan dwell is nonzero"),
            Preference::PreferHe20,
        ),
    );
    // Board-selected engineering limits, not qualified timing bounds.
    use core::num::NonZeroU32;
    static WATCHDOG_CONFIG: StaticCell<WatchdogConfig> = StaticCell::new();
    let watchdog = WATCHDOG_CONFIG.init(WatchdogConfig::new(
        watchdog,
        DeadlineBudget::from_micros(NonZeroU32::new(5_000_000).unwrap()),
        DeadlineBudget::from_micros(NonZeroU32::new(1_000_000).unwrap()),
    ));
    let config = RadioConfig::from_efuse(
        watchdog,
        WifiChannel::mhz20(1).expect("initial channel is valid"),
    )
    .expect("the eFuse holds unicast interface addresses");
    // The network stack's random seed, taken before Wi-Fi owns the TRNG.
    let network_seed = (u64::from(trng.random()) << 32) | u64::from(trng.random());
    let (radio, partitions) = shared_radio::start(spawner, radio_platform, RadioStart::new())
        .expect("the shared radio starts once");
    let ConcurrentPartitions {
        wifi: partition,
        wifi_coex,
        ..
    } = partitions;
    let WifiStarted {
        wifi,
        initialization: _,
        runner: radio_runner,
    } = integration::await_stack_boundary!(integration::new(
        radio,
        partition,
        wifi_coex,
        wifi_platform,
        trng,
        config
    ))
    .expect("Wi-Fi initialization must succeed once");
    spawner.spawn(radio_task(spawner, radio_runner).expect("radio task storage is available once"));
    let WifiParts {
        control: wifi,
        station_device,
        access_point_device: _,
        monitor_frames: _,
        station_status: _,
        access_point_status: _,
    } = wifi.into_parts();
    let network = network::run(station_device, network_seed);
    let application = async move {
        let completed = wifi
            .scan(WifiScanRequest::new(
                StationScanChannels::CHANNELS_1_TO_13,
                NonZeroU16::new(50).expect("standalone scan dwell is nonzero"),
            ))
            .await
            .unwrap_or_else(|error| panic!("standalone scan failed: {error:?}"));
        let (wifi, report) = completed.into_parts();
        esp_println::println!(
            "open-radio: scan generation={} networks={}",
            report.generation().value(),
            report.results().len(),
        );
        match wifi.start_station(request).await {
            Ok(station) => {
                esp_println::println!(
                    "open-radio: station active generation={}",
                    station.generation().value(),
                );
                let _station = station;
            }
            Err(error) => esp_println::println!("open-radio: station start failed: {error:?}"),
        }
        core::future::pending::<()>().await;
    };
    embassy_futures::join::join(application, network).await;
}

#[embassy_executor::task]
#[allow(
    large_assignments,
    reason = "the sole radio runner enters its static task arena once; the final ELF's stack gate and runtime stack painting check CPU stack use"
)]
async fn radio_task(spawner: embassy_executor::Spawner, runner: integration::SystemRunner) {
    runner.run(spawner).await;
}
