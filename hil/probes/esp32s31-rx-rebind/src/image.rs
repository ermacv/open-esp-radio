//! TEMPORARY experiment (branch `probe/rx-descriptor-rebind`): one live RX
//! ring run with fixed-binding and rebind segments (A–B–B–A).
use crate::usb;
use oer_chips_esp32s31_hal::{
    peripherals::usb_serial_jtag::ConsoleInterrupt,
    peripherals::watchdog::{ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters},
    peripherals::{
        efuse::{FactoryIdentity, Interface},
        software_interrupt::{FromCpu0Irq, FromCpu2Irq},
        system_timer::{SystimerAlarm0, SystimerAlarm1},
    },
    power::ChipClocks,
    radio::ieee80211::{
        dma::{rx_rebind::RxSpareBank, rx_storage::RxDmaStorage},
        init::{
            MAC_COLD_HANDSHAKE_DEADLINE, MacSlowClockCalibration, MacSlowClockCalibrationSource,
        },
    },
    system::root::Root,
};
use oer_chips_esp32s31_platform_board as board;
use oer_chips_esp32s31_platform_runtime::Esp32s31;
use oer_chips_esp32s31_radio_client::{LpClock, RcSlow};
use oer_chips_esp32s31_radio_phy::{
    NoopPhyTargetObserver, PhyCalibrationIdentity, PhyConfig, PhyRegisterConfig,
    concurrent::ConcurrentPhy, phy_get_rf_cal_version,
};
use oer_chips_esp32s31_radio_system::Esp32s31Radio;
use oer_chips_family_platform_executor::{Executor, InterruptExecutor};
use oer_portable_embassy_time::EmbassyClock;
use oer_portable_time::{Clock, Duration, Timer};
use oer_protocols_ieee80211_wire::channel::{WifiChannel, WifiChannelWidth};
use oer_protocols_radio_regulatory::{Country, RegulatoryDomain};
use static_cell::StaticCell;

#[path = "experiment.rs"]
mod experiment;

const COUNT: usize = 32;
/// One buffer holds a whole 1500-byte MSDU frame with its RX-control prefix.
const BUFFER: usize = 1664;
const CHANNEL: u8 = 13;
type Storage = RxDmaStorage<COUNT, BUFFER, BUFFER>;
type Spares = RxSpareBank<BUFFER>;
type Backend = Esp32s31Radio<
    ChipClocks,
    oer_chips_family_platform_executor::Cooperative<Esp32s31, EmbassyClock>,
    NoopPhyTargetObserver,
    crate::phy_pilot::Client,
    oer_portable_trace::Noop,
>;
static DIRECT_BACKEND: StaticCell<Backend> = StaticCell::new();
struct DirectOutput {
    access: oer_chips_esp32s31_hal::system::owner::RadioAccess,
    _clocks: oer_chips_esp32s31_hal::radio::ieee80211::client::WifiClocksOn,
}
static DIRECT_OUTPUT: StaticCell<DirectOutput> = StaticCell::new();
static DIRECT_READY: oer_portable_memory::ClaimCell<
    Result<&'static mut DirectOutput, oer_portable_diagnostics::Name<32>>,
> = oer_portable_memory::ClaimCell::new();
oer_chips_esp32s31_hal::dma_static! { static RX: Storage = zeroed in ".dma.bss.rx_rebind_probe"; }
oer_chips_esp32s31_hal::dma_static! { static SPARES: Spares = zeroed in ".dma.bss.rx_rebind_spares"; }

use crate::phy_pilot;

// The handlers keep the default text placement: this probe adds no unsafe
// attribute.
oer_chips_family_platform_interrupt_table::interrupt_table! {
    lines: oer_chips_family_platform_runtime::interrupts::LINES,
    handler_attributes: [];
    contexts { app: 1; radio_exec: 2; radio_irq: 3; }
    wake: WakeToken = FromCpu0Irq => oer_chips_family_platform_executor::wake_handler::<Esp32s31, 0, FromCpu0Irq>, line 40, context app, hart 0, Hardware;
    // SOURCE: family.toml [interrupts]; HW23_S31_P2_ARBITRATION_20261027:
    // only P2 line17 before16 is measured on S31.
    phy: PhyToken = oer_chips_esp32s31_hal::peripherals::timg0_alarm::Timg0AlarmInterrupt => phy_pilot::interrupt, line 16, context radio_exec, hart 0, Exempt(ExecutorLevel, "SAR/PWDET steps between radio executor rounds");
    radio: RadioToken = FromCpu2Irq => oer_chips_family_platform_executor::Round<RadioTasks>, line 17, context radio_exec, hart 0, Executor;
    radio_alarm: RadioAlarm = SystimerAlarm0 => oer_chips_family_platform_executor::alarm_interrupt::<RadioTasks, 0, SystimerAlarm0>, line 18, context radio_irq, hart 0, Hardware;
    console_usb: ConsoleUsb = ConsoleInterrupt => crate::usb::interrupt, line 41, context app, hart 0, Hardware;
    app_alarm: AppAlarm = SystimerAlarm1 => oer_chips_family_platform_executor::alarm_interrupt::<AppTasks, 1, SystimerAlarm1>, line 42, context app, hart 0, Hardware;
}
oer_chips_family_platform_executor::tasks! {
    pub RadioTasks: InterruptExecutor<Esp32s31, 2> {
        direct(backend: &'static mut Backend, lp: LpClock<RcSlow>, wifi: oer_chips_esp32s31_hal::system::root::WifiPartition, delay_random: u32, station: [u8;6], ap: [u8;6]) = direct_start;
    }
}
oer_chips_family_platform_executor::tasks! {
    AppTasks: Executor<Esp32s31, 0> {
        capture(boot: Boot) = capture;
        watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_watchdog;
    }
}
struct Boot {
    console: usb::Console,
    _pads: oer_chips_esp32s31_hal::peripherals::usb_serial_jtag::UsbPads,
    mac_irq: oer_chips_esp32s31_hal::system::owner::MacInterruptRegisters,
}
oer_chips_esp32s31_hal::phy_fault_reporter!(oer_chips_esp32s31_platform_runtime::phy_fault::reset);
oer_chips_family_platform_runtime::entry!(
    main,
    oer_chips_esp32s31_hal::system::platform::Port,
    hook = crate::diagnostic::panic,
    exception = crate::diagnostic::exception
);
fn main(
    handoff: oer_chips_family_platform_runtime::Handoff<
        oer_chips_esp32s31_hal::system::platform::Port,
    >,
) -> ! {
    let Some(root) = Root::take() else {
        fail(oer_portable_diagnostics::name!("root"; 32));
    };
    let (
        (
            runtime_efuse,
            runtime_radio,
            runtime_timg0_alarm,
            runtime_clocks,
            runtime_system_timer,
            runtime_wifi_interrupts,
            runtime_software_interrupts,
            runtime_entropy,
            runtime_watchdogs,
        ),
        _psram,
        mut adopted,
    ) = match oer_chips_family_platform_runtime::adoption::run::<
        oer_chips_esp32s31_platform_runtime::Esp32s31,
        _,
        _,
    >(
        handoff,
        root.into_runtime(),
        INTERRUPT_TABLE,
        board::BootstrapPsram,
        oer_chips_esp32s31_platform_runtime::phy_fault::report_previous,
        |hardware| {
            let _reset_causes =
                oer_chips_esp32s31_platform_runtime::reset_causes::read(hardware.reset_cause);

            crate::diagnostic::report_previous();
            (
                hardware.efuse,
                hardware.radio,
                hardware.timg0_alarm,
                hardware.clocks,
                hardware.system_timer,
                hardware.wifi_interrupts,
                hardware.software_interrupts,
                hardware.entropy,
                hardware.watchdogs,
            )
        },
    ) {
        Ok(value) => value.into_parts(),
        Err(error) => fail(error.name()),
    };
    let Some(interrupts) = Interrupts::take() else {
        fail(oer_portable_diagnostics::name!("interrupts"; 32));
    };
    crate::diagnostic::phase(1);
    let factory = FactoryIdentity::read(runtime_efuse);
    let identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        base_mac_address: factory.base_mac(),
        mac_extension: factory.mac_extension(),
    };
    let station = factory.address(Interface::Station);
    let ap = factory.address(Interface::AccessPoint);
    let (domain, partitions) = runtime_radio
        .split::<ChipClocks, _>(
            ConcurrentPhy::new(),
            oer_chips_esp32s31_platform_runtime::phy_fault::identity(),
        )
        .unwrap_or_else(|failure| {
            let fault = failure.fault();
            let _owners = failure;
            oer_chips_esp32s31_platform_runtime::phy_fault::reset(fault)
        });
    let Some(lp) = LpClock::<RcSlow>::claim() else {
        fail(oer_portable_diagnostics::name!("claim"; 32));
    };
    let phy_pilot = crate::phy_pilot::init(runtime_timg0_alarm, adopted.router(), interrupts.phy);
    let backend = Esp32s31Radio::new(
        domain,
        partitions.wifi_coex,
        runtime_clocks,
        oer_chips_family_platform_executor::Cooperative::<Esp32s31, _>::new(EmbassyClock),
        NoopPhyTargetObserver,
        oer_portable_trace::Noop,
        PhyRegisterConfig::new(identity),
        phy_pilot,
    );
    let Some(backend) = DIRECT_BACKEND.try_init(backend) else {
        fail(oer_portable_diagnostics::name!("direct-backend"; 32));
    };
    let timer = runtime_system_timer;
    let mut timer_ready = oer_chips_family_platform_executor::init::<Esp32s31, 2, _, _, _>(
        oer_chips_family_platform_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        adopted.router(),
        interrupts.radio_alarm,
        timer.alarm0,
    )
    .unwrap_or_else(|failure| failure.reset());
    oer_chips_family_platform_executor::bind::<Esp32s31, 0, _, _, _>(
        &mut timer_ready,
        adopted.router(),
        interrupts.app_alarm,
        timer.alarm1,
    )
    .unwrap_or_else(|failure| failure.reset());
    // No Wi-Fi interrupt consumer is installed: this experiment polls only.
    // The MAC bank stays with the polling task; no CPU route is installed.
    let mut wifi_irqs = runtime_wifi_interrupts;
    wifi_irqs.mac.mask_all_mac_interrupts();
    wifi_irqs.power.mask_all_power_interrupts();
    let software = runtime_software_interrupts;
    let (hart, router) = adopted.parts();
    let radio = InterruptExecutor::<Esp32s31, 2>::new(software.line2)
        .start(hart, router, interrupts.radio, software.irq2, ())
        .unwrap_or_else(|failure| failure.reset());
    let mut rng =
        oer_chips_esp32s31_hal::peripherals::entropy::Entropy::new(runtime_entropy, EmbassyClock);
    let delay_random = match rng.random() {
        Ok(word) => word,
        Err(_) => fail(oer_portable_diagnostics::name!("entropy"; 32)),
    };
    if RadioTasks::direct(
        &radio,
        backend,
        lp,
        partitions.wifi,
        delay_random,
        station,
        ap,
    )
    .is_err()
    {
        fail(oer_portable_diagnostics::name!("direct-task"; 32));
    }
    let (console, pads) = match usb::open(adopted.router(), interrupts.console_usb) {
        Ok(value) => value,
        Err(error) => fail(error.reason()),
    };
    let (mut router, hart) = match adopted.enable_interrupts() {
        Ok(value) => value,
        Err(error) => fail(error.name()),
    };
    let boot = Boot {
        console,
        _pads: pads,
        mac_irq: wifi_irqs.mac,
    };
    match Executor::<Esp32s31, 0>::new(software.line0)
        .run(
            &hart,
            &mut router,
            interrupts.wake,
            software.irq0,
            (),
            (),
            move |spawner| {
                if AppTasks::capture(spawner, boot).is_err()
                    || AppTasks::watchdog(spawner, runtime_watchdogs.rtc).is_err()
                {
                    fail(oer_portable_diagnostics::name!("task"; 32));
                }
            },
        )
        .unwrap_or_else(|failure| failure.reset()) {}
}
fn fail<const B: usize>(reason: oer_portable_diagnostics::Name<B>) -> ! {
    oer_chips_family_platform_runtime::fatal::boot_failure::<
        oer_chips_esp32s31_hal::system::platform::Port,
    >(reason.as_str(), || {
        let _ = oer_chips_esp32s31_hal::peripherals::usb_serial_jtag::fatal_note(
            &oer_portable_diagnostics::Note::runtime_fail(reason),
        );
    })
}
type DirectRun =
    impl core::future::Future<Output = Result<DirectOutput, oer_portable_diagnostics::Name<32>>>;
type DirectPrepare<'a> = impl core::future::Future<
        Output = Result<
            oer_chips_esp32s31_radio_client::PhyPreparation,
            oer_chips_esp32s31_radio_system::Fault,
        >,
    > + 'a;
type DirectTrack<'a> = impl core::future::Future<
        Output = Result<
            oer_chips_esp32s31_radio_client::wifi::WifiTracked,
            oer_chips_esp32s31_radio_client::Failed<
                oer_chips_esp32s31_hal::radio::ieee80211::client::WifiClocked,
            >,
        >,
    > + 'a;
static PREPARE: static_cell::ConstStaticCell<
    oer_portable_memory::StaticPlace<DirectPrepare<'static>>,
> = static_cell::ConstStaticCell::new(oer_portable_memory::StaticPlace::new());
static TRACK: static_cell::ConstStaticCell<oer_portable_memory::StaticPlace<DirectTrack<'static>>> =
    static_cell::ConstStaticCell::new(oer_portable_memory::StaticPlace::new());
#[define_opaque(DirectPrepare)]
fn prepare_future(backend: &mut Backend) -> DirectPrepare<'_> {
    use oer_chips_esp32s31_radio_system::PhyDomain;
    backend.prepare()
}
#[define_opaque(DirectTrack)]
fn track_future(
    backend: &mut Backend,
    request: oer_chips_esp32s31_radio_client::wifi::WifiTrack,
) -> DirectTrack<'_> {
    use oer_chips_esp32s31_radio_system::ClientBackend;
    backend.track(request)
}
#[inline(never)]
fn placed_prepare(
    backend: &mut Backend,
) -> Result<
    oer_portable_memory::PlacedFuture<'_, DirectPrepare<'_>>,
    oer_portable_diagnostics::Name<32>,
> {
    let place = PREPARE
        .try_take()
        .ok_or(oer_portable_diagnostics::name!("direct-prepare-place"; 32))?;
    Ok(place.future(|| prepare_future(backend)))
}
#[inline(never)]
fn placed_track(
    backend: &mut Backend,
    request: oer_chips_esp32s31_radio_client::wifi::WifiTrack,
) -> Result<
    oer_portable_memory::PlacedFuture<'_, DirectTrack<'_>>,
    oer_portable_diagnostics::Name<32>,
> {
    let place = TRACK
        .try_take()
        .ok_or(oer_portable_diagnostics::name!("direct-track-place"; 32))?;
    Ok(place.future(|| track_future(backend, request)))
}
static DIRECT_RUN: static_cell::ConstStaticCell<oer_portable_memory::StaticPlace<DirectRun>> =
    static_cell::ConstStaticCell::new(oer_portable_memory::StaticPlace::new());
#[define_opaque(DirectRun)]
fn direct_future(
    backend: &'static mut Backend,
    lp: LpClock<RcSlow>,
    wifi: oer_chips_esp32s31_hal::system::root::WifiPartition,
    delay_random: u32,
    station: [u8; 6],
    ap: [u8; 6],
) -> DirectRun {
    async move {
        use oer_chips_esp32s31_hal::radio::ieee80211::{
            client::WifiCold, init::MacColdStartConfig,
        };
        use oer_chips_esp32s31_radio_client::wifi::{WifiJoin, WifiTrack};
        use oer_chips_esp32s31_radio_system::ClientBackend;
        let channel = WifiChannel::new_2_4_ghz(CHANNEL, WifiChannelWidth::Mhz20)
            .map_err(|_| oer_portable_diagnostics::name!("direct-channel"; 32))?;
        let domain = Country::new(*b"DE")
            .ok()
            .and_then(RegulatoryDomain::of)
            .ok_or(oer_portable_diagnostics::name!("direct-domain"; 32))?;
        let channel = domain
            .permit(oer_protocols_ieee80211_contracts_port::RegulatedChannel::from(channel))
            .map_err(|_| oer_portable_diagnostics::name!("direct-permit"; 32))?;
        let tx_ceiling = channel.tx_ceiling(board::wifi_power_limits(
            i32::from(PhyConfig::production().highest_target_power_quarter_dbm()) * 25,
            Some(1000),
        ));
        let phy = placed_prepare(backend)?
            .await
            .map_err(|_| oer_portable_diagnostics::name!("direct-prepare"; 32))?;
        let joined = backend
            .enter(
                WifiJoin {
                    wifi: WifiCold::from_partition(wifi),
                },
                phy,
                &lp,
            )
            .await
            .map_err(|_| oer_portable_diagnostics::name!("direct-enter"; 32))?;
        let mut platform = Platform;
        let tracked = placed_track(
            backend,
            WifiTrack {
                wifi: joined.wifi,
                channel,
                tx_ceiling,
                mac: MacColdStartConfig {
                    handshake_deadline: MAC_COLD_HANDSHAKE_DEADLINE,
                    station_address: station,
                    access_point_address: ap,
                },
                slow_clock: platform.mac_slow_clock_calibration(),
                delay_random,
            },
        )?
        .await
        .map_err(|_| oer_portable_diagnostics::name!("direct-track"; 32))?;
        let (access, clocks) = tracked.wifi.into_running();
        Ok(DirectOutput {
            access,
            _clocks: clocks,
        })
    }
}
#[inline(never)]
fn placed_direct(
    backend: &'static mut Backend,
    lp: LpClock<RcSlow>,
    wifi: oer_chips_esp32s31_hal::system::root::WifiPartition,
    delay_random: u32,
    station: [u8; 6],
    ap: [u8; 6],
) -> oer_portable_memory::PlacedFuture<'static, DirectRun> {
    let Some(place) = DIRECT_RUN.try_take() else {
        fail(oer_portable_diagnostics::name!("direct-future-claimed"; 32));
    };
    place.future(|| direct_future(backend, lp, wifi, delay_random, station, ap))
}
async fn direct_start(
    backend: &'static mut Backend,
    lp: LpClock<RcSlow>,
    wifi: oer_chips_esp32s31_hal::system::root::WifiPartition,
    delay_random: u32,
    station: [u8; 6],
    ap: [u8; 6],
) {
    let result = match placed_direct(backend, lp, wifi, delay_random, station, ap).await {
        Ok(output) => DIRECT_OUTPUT
            .try_init(output)
            .ok_or(oer_portable_diagnostics::name!("direct-output"; 32)),
        Err(reason) => Err(reason),
    };
    if DIRECT_READY.install(result).is_err() {
        fail(oer_portable_diagnostics::name!("direct-publish"; 32));
    }
}
async fn feed_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) {
    match watchdog
        .feed_every(
            &EmbassyClock,
            Duration::from_millis(RTC_WATCHDOG_FEED_PERIOD_MS),
        )
        .await {}
}
struct Platform;
impl MacSlowClockCalibrationSource for Platform {
    fn mac_slow_clock_calibration(&mut self) -> MacSlowClockCalibration {
        MacSlowClockCalibration::Unavailable
    }
}

async fn capture(boot: Boot) {
    let Boot {
        mut console,
        _pads,
        mac_irq,
    } = boot;
    usb::text(&mut console, b"REBIND boot\n").await;
    match run_direct(&mut console, mac_irq).await {
        Err(reason) => {
            usb::text(&mut console, reason.as_str().as_bytes()).await;
            usb::text(&mut console, b"\nOER-RESULT: fail rx-rebind error\n").await;
        }
        Ok(crate::verdict::Verdict::Pass) => {
            usb::text(&mut console, b"OER-RESULT: pass rx-rebind\n").await;
        }
        Ok(crate::verdict::Verdict::Fail(kind)) => {
            usb::text(&mut console, b"OER-RESULT: fail rx-rebind violation=").await;
            usb::text(&mut console, kind.name().as_bytes()).await;
            usb::text(&mut console, b"\n").await;
        }
        Ok(crate::verdict::Verdict::Insufficient(shortfall)) => {
            usb::text(
                &mut console,
                b"OER-RESULT: fail rx-rebind insufficient-experiment=",
            )
            .await;
            usb::text(&mut console, shortfall.name().as_bytes()).await;
            usb::text(&mut console, b"\n").await;
        }
    }
}
async fn run_direct(
    console: &mut usb::Console,
    mut mac_irq: oer_chips_esp32s31_hal::system::owner::MacInterruptRegisters,
) -> Result<crate::verdict::Verdict, oer_portable_diagnostics::Name<32>> {
    let deadline = EmbassyClock
        .now()
        .checked_add(Duration::from_secs(5))
        .ok_or(oer_portable_diagnostics::name!("direct-deadline"; 32))?;
    let output = loop {
        if let Ok(result) = DIRECT_READY.take() {
            break result?;
        }
        if EmbassyClock.now() >= deadline {
            return Err(oer_portable_diagnostics::name!("direct-ready-timeout"; 32));
        }
        EmbassyClock
            .wait_until(
                EmbassyClock
                    .now()
                    .checked_add(Duration::from_millis(1))
                    .ok_or(oer_portable_diagnostics::name!("direct-wait"; 32))?,
            )
            .await;
    };
    let access = &mut output.access;
    let (state, frame) = experiment::take()?;
    // Every buffer the walker can reach is a spare, painted while it is
    // still the CPU's; the bank stays with the arena until reset.
    let (placed, bank) = RxDmaStorage::place_with_spares(
        RX.take()
            .ok_or(oer_portable_diagnostics::name!("rx-claimed"; 32))?,
        SPARES
            .take()
            .ok_or(oer_portable_diagnostics::name!("spares-claimed"; 32))?,
        |spare, bytes| {
            for (word, chunk) in bytes.chunks_exact_mut(4).enumerate() {
                chunk.copy_from_slice(&experiment::canary(spare, word).to_le_bytes());
            }
        },
    )
    .map_err(|_| oer_portable_diagnostics::name!("rx-place"; 32))?;
    let storage = placed.storage();
    let prepared = placed
        .prepare_ring(access.rx_dma())
        .map_err(|_| oer_portable_diagnostics::name!("rx-prepare"; 32))?;
    let ready = access.wifi_mac_hal().prepare_mac_runtime(prepared);
    let stopped_ring = ready
        .publish(access.rx_dma())
        .map_err(|_| oer_portable_diagnostics::name!("rx-publish"; 32))?;
    let mut ring = stopped_ring
        .try_start(access.rx_dma())
        .map_err(|_| oer_portable_diagnostics::name!("rx-start"; 32))?;
    let stale = mac_irq.mac_interrupt_raw();
    if stale.fatal_assertion_raw().is_some() {
        return Err(oer_portable_diagnostics::name!("initial-fatal"; 32));
    }
    mac_irq.acknowledge_mac_interrupt_raw(stale);
    access.wifi_mac_hal().configure_open_promiscuous_receive();
    mac_irq.mask_all_mac_interrupts();
    usb::text(
        console,
        b"REBIND begin v=2 channel=13 count=32 buffer=1664 spares=96 pool=64 segment_ms=2500 action_ms=80 arms=fixed,rebind,rebind,fixed-late\n",
    )
    .await;
    let observation =
        experiment::run(state, frame, storage, bank, &mut ring, access, &mut mac_irq).await;
    access.wifi_mac_hal().request_channel_stop();
    let deadline = EmbassyClock
        .now()
        .checked_add(Duration::from_millis(50))
        .ok_or(oer_portable_diagnostics::name!("stop-deadline"; 32))?;
    while access.wifi_mac_hal().channel_active_state() != 0 {
        if EmbassyClock.now() >= deadline {
            return Err(oer_portable_diagnostics::name!("mac-stop"; 32));
        }
        EmbassyClock
            .wait_until(
                EmbassyClock
                    .now()
                    .checked_add(Duration::from_millis(1))
                    .ok_or(oer_portable_diagnostics::name!("stop-wait"; 32))?,
            )
            .await;
    }
    // The final sweep runs only on the stopped walker.
    let _halted = ring
        .try_stop(access.rx_dma())
        .map_err(|_| oer_portable_diagnostics::name!("walker-stop"; 32))?;
    let finished = experiment::finish(state, storage, bank);
    experiment::dump(state, console).await?;
    observation?;
    finished?;
    // The verdict is decided only now: after the walker stopped and the
    // evidence was printed. The static backend, storage and spares retain
    // every owner until the leased host reset; nothing reaches the network.
    Ok(crate::verdict::decide(&experiment::observations(state)))
}

oer_chips_family_platform_executor::time_driver!(Esp32s31);
