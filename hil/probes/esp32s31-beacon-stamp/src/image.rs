//! TEMPORARY experiment (branch `probe/beacon-timestamp`): who writes the
//! Timestamp of a transmitted beacon. The image brings Wi-Fi up on one
//! channel through the production client path, starts the AP TSF from zero,
//! transmits the rounds of `experiment.rs` from the AP interface and prints
//! its records after the last round. The host program
//! (`hil/scenarios/beacon-stamp`) decides from the records and a capture.
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
        dma::{rx_storage::RxDmaStorage, space::ChipDma, tx_storage::TxDmaStorage},
        init::{
            MAC_COLD_HANDSHAKE_DEADLINE, MacSlowClockCalibration, MacSlowClockCalibrationSource,
        },
    },
    system::{memory::dma_static::PlacedTxPool, root::Root},
};
use oer_chips_esp32s31_platform_board as board;
use oer_chips_esp32s31_platform_runtime::Esp32s31;
use oer_chips_esp32s31_radio_client::{LpClock, RcSlow};
use oer_chips_esp32s31_radio_ieee80211_backend::mac::tx::TxSlot;
use oer_chips_esp32s31_radio_phy::{
    NoopPhyTargetObserver, PhyCalibrationIdentity, PhyConfig, PhyRegisterConfig,
    PhyTxTargetPowerProfile, concurrent::ConcurrentPhy, phy_get_rf_cal_version,
};
use oer_chips_esp32s31_radio_system::Esp32s31Radio;
use oer_chips_family_platform_executor::{Executor, InterruptExecutor};
use oer_portable_embassy_time::EmbassyClock;
use oer_portable_memory::PinnedDmaTxPool;
use oer_portable_time::{Clock, Duration, Timer};
use oer_protocols_ieee80211_wire::channel::{WifiChannel, WifiChannelWidth};
use oer_protocols_radio_regulatory::{Country, RegulatoryDomain, TxCeiling};
use static_cell::StaticCell;

#[path = "experiment.rs"]
mod experiment;

/// The receive ring the MAC runtime needs; nothing reads what it receives.
const COUNT: usize = 32;
const RX_BUFFER: usize = 128;
/// One transmit buffer: the backend's metadata, the blocker's 1524-octet
/// MPDU and its FCS.
pub(crate) const TX_BUFFER: usize = 1536;
pub(crate) const CHANNEL: u8 = 13;
type Storage = RxDmaStorage<COUNT, RX_BUFFER, RX_BUFFER>;
/// The subject's buffer and the blocker's.
type TxPool = PinnedDmaTxPool<TX_BUFFER, 0, 0, 2>;
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
    tx_power: PhyTxTargetPowerProfile,
    tx_ceiling: TxCeiling,
}
static DIRECT_OUTPUT: StaticCell<DirectOutput> = StaticCell::new();
static DIRECT_READY: oer_portable_memory::ClaimCell<
    Result<&'static mut DirectOutput, oer_portable_diagnostics::Name<32>>,
> = oer_portable_memory::ClaimCell::new();
oer_chips_esp32s31_hal::dma_static! {
    static RX: Storage = zeroed in ".dma.bss.beacon_stamp_rx";
    static TX_POOL: TxPool = zeroed in ".dma.bss.beacon_stamp_tx_pool";
    static TX_SUBJECT: TxDmaStorage = zeroed in ".dma.bss.beacon_stamp_tx_subject";
    static TX_BLOCKER: TxDmaStorage = zeroed in ".dma.bss.beacon_stamp_tx_blocker";
}
static SUBJECT_SLOT: StaticCell<TxSlot<TX_BUFFER>> = StaticCell::new();
static BLOCKER_SLOT: StaticCell<TxSlot<TX_BUFFER>> = StaticCell::new();

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
    ap: [u8; 6],
    run: u32,
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
    // The run's identifier in every frame's tag: one boot's frames are not
    // taken for another's in a capture that spans several runs.
    let run = match rng.random() {
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
        ap,
        run,
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
            i32::from(PhyConfig::production().highest_target_power_quarter_dbm())
                .saturating_mul(25),
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
        let tx_power = tracked.report.tx_power;
        let (access, clocks) = tracked.wifi.into_running();
        Ok(DirectOutput {
            access,
            _clocks: clocks,
            tx_power,
            tx_ceiling,
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
        ap,
        run,
    } = boot;
    let booted = usb::text(&mut console, b"STAMP boot\n").await;
    let outcome = run_direct(&mut console, mac_irq, ap, run).await;
    let mut line = crate::line::Line::<192>::new();
    if result_line(&outcome, &mut line).is_err() {
        line = crate::line::Line::new();
        let _ = line.text(b"OER-RESULT: fail beacon-stamp report-overflow\n");
    }
    let written = usb::text(&mut console, line.as_bytes()).await;
    // The console is this image's only output: a write it refuses has no
    // other place to be reported, and no other line replaces the result.
    let _ = (booted, written);
}

type Name = oer_portable_diagnostics::Name<32>;

fn finish_name(finish: &crate::shutdown::Finish<Name>) -> &[u8] {
    match finish {
        crate::shutdown::Finish::NotRun => b"not-run",
        crate::shutdown::Finish::Passed => b"passed",
        crate::shutdown::Finish::Failed(error) => error.as_str().as_bytes(),
    }
}

/// The run's one result line: a pass only for a completed run.
fn result_line<const N: usize>(
    outcome: &Result<crate::shutdown::RunOutcome<Name>, Name>,
    line: &mut crate::line::Line<N>,
) -> Result<(), crate::line::LineFull> {
    use crate::shutdown::RunOutcome;
    match outcome {
        Err(reason) => {
            line.text(b"OER-RESULT: fail beacon-stamp error=")?
                .text(reason.as_str().as_bytes())?
                .text(b"\n")?;
        }
        Ok(RunOutcome::Completed) => {
            line.text(b"OER-RESULT: pass beacon-stamp\n")?;
        }
        Ok(RunOutcome::Failed {
            cause,
            mac,
            dma,
            finish,
        }) => {
            line.text(b"OER-RESULT: fail beacon-stamp cleanup cause=")?
                .text(
                    cause
                        .as_ref()
                        .map_or(b"none".as_slice(), |c| c.as_str().as_bytes()),
                )?
                .text(b" mac=")?
                .text(mac.name().as_bytes())?
                .text(b" dma=")?
                .text(dma.name().as_bytes())?
                .text(b" finish=")?
                .text(finish_name(finish))?
                .text(b"\n")?;
        }
    }
    Ok(())
}

/// Wait `by` on the executor's timer.
async fn wait(by: Duration) -> Result<(), Name> {
    let at = EmbassyClock
        .now()
        .checked_add(by)
        .ok_or(oer_portable_diagnostics::name!("wait-deadline"; 32))?;
    EmbassyClock.wait_until(at).await;
    Ok(())
}

/// Everything up to the walker start returns early: nothing runs yet.
/// From the start on, every path goes through the one shared cleanup.
async fn run_direct(
    console: &mut usb::Console,
    mut mac_irq: oer_chips_esp32s31_hal::system::owner::MacInterruptRegisters,
    ap: [u8; 6],
    run: u32,
) -> Result<crate::shutdown::RunOutcome<Name>, Name> {
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
        wait(Duration::from_millis(1)).await?;
    };
    let tx_power = output.tx_power;
    let tx_ceiling = output.tx_ceiling;
    let access = &mut output.access;
    // The transmit side and the records are prepared before the walker
    // starts: a refusal here returns before anything runs.
    let records = experiment::records()?;
    let pool = PlacedTxPool::pin(
        TX_POOL
            .take()
            .ok_or(oer_portable_diagnostics::name!("tx-pool-claimed"; 32))?,
    )
    .map_err(|_| oer_portable_diagnostics::name!("tx-pool-place"; 32))?;
    let mut subject = slot(
        &SUBJECT_SLOT,
        TX_SUBJECT
            .take()
            .ok_or(oer_portable_diagnostics::name!("tx-subject-claimed"; 32))?,
        &pool,
        0,
    )?;
    let mut blocker = slot(
        &BLOCKER_SLOT,
        TX_BLOCKER
            .take()
            .ok_or(oer_portable_diagnostics::name!("tx-blocker-claimed"; 32))?,
        &pool,
        1,
    )?;
    let placed = RxDmaStorage::place(
        RX.take()
            .ok_or(oer_portable_diagnostics::name!("rx-claimed"; 32))?,
    )
    .map_err(|_| oer_portable_diagnostics::name!("rx-place"; 32))?;
    let prepared = placed
        .prepare_ring(access.rx_dma())
        .map_err(|_| oer_portable_diagnostics::name!("rx-prepare"; 32))?;
    let ready = access.wifi_mac_hal().prepare_mac_runtime(prepared);
    let stopped_ring = ready
        .publish(access.rx_dma())
        .map_err(|_| oer_portable_diagnostics::name!("rx-publish"; 32))?;
    experiment::begin(console, run, ap).await?;
    let ring = stopped_ring
        .try_start(access.rx_dma())
        .map_err(|_| oer_portable_diagnostics::name!("rx-start"; 32))?;
    // From here every path, the last round, a fatal RAW right after the
    // start, a transmit preparation's or a round's error, goes through the
    // one shared cleanup (`shutdown`): no early return before both stops.
    let mut ran = experiment::Ran::new();
    let stale = mac_irq.mac_interrupt_raw();
    let measured = if stale.fatal_assertion_raw().is_some() {
        Err(oer_portable_diagnostics::name!("initial-fatal"; 32))
    } else {
        mac_irq.acknowledge_mac_interrupt_raw(stale);
        // The production order: the walker runs, then the MAC runtime.
        access.wifi_mac_hal().start_mac_runtime(&ring);
        mac_irq.mask_all_mac_interrupts();
        let mut resources = experiment::Resources {
            access: &mut *access,
            subject: subject.as_mut(),
            blocker: blocker.as_mut(),
            tx_power,
            tx_ceiling,
            ap,
            run,
        };
        experiment::measure(&mut resources, records, &mut ran).await
    };
    // A register read: it touches no buffer.
    let rx_full = access.wifi_mac_hal().rx_buffer_full_count();
    let mut stop = ProbeStop {
        access,
        ring: Some(ring),
        halted: None,
    };
    let (mac, dma) = crate::shutdown::shut_down(&mut stop).await;
    // The final check runs only on a confirmed walker stop; otherwise the
    // ring, the slots and their buffers stay held, never reused, whether or
    // not the board is reset after the run.
    let outcome = crate::shutdown::conclude(measured, mac, dma, || {
        experiment::slots_free(&subject, &blocker)
    });
    let (cause, finish) = match &outcome {
        crate::shutdown::RunOutcome::Completed => (None, b"passed".as_slice()),
        crate::shutdown::RunOutcome::Failed { cause, finish, .. } => {
            (cause.as_ref(), finish_name(finish))
        }
    };
    experiment::dump(
        console,
        &ran,
        records,
        experiment::End {
            rx_full,
            cause,
            mac: mac.name(),
            dma: dma.name(),
            finish,
        },
    )
    .await?;
    Ok(outcome)
}

/// The cleanup's port: the MAC channel, the clock for its bounded wait and
/// the walker. A walker that does not read back as stopped stays here.
struct ProbeStop<'a> {
    access: &'a mut oer_chips_esp32s31_hal::system::owner::RadioAccess,
    ring:
        Option<oer_chips_esp32s31_hal::radio::ieee80211::dma::rx_ring::RxRingLive<'static, COUNT>>,
    halted: Option<
        oer_chips_esp32s31_hal::radio::ieee80211::dma::rx_ring::RxRingHalted<'static, COUNT>,
    >,
}

impl crate::shutdown::StopPort for ProbeStop<'_> {
    type Deadline = oer_portable_time::Instant;

    fn request_mac_stop(&mut self) {
        self.access.wifi_mac_hal().request_channel_stop();
    }

    fn mac_active(&mut self) -> bool {
        self.access.wifi_mac_hal().channel_active_state() != 0
    }

    fn deadline(&mut self) -> Option<Self::Deadline> {
        EmbassyClock
            .now()
            .checked_add(Duration::from_millis(crate::shutdown::MAC_STOP_MS))
    }

    fn passed(&mut self, deadline: &Self::Deadline) -> bool {
        EmbassyClock.now() >= *deadline
    }

    async fn tick(&mut self) -> Result<(), ()> {
        let at = EmbassyClock
            .now()
            .checked_add(Duration::from_millis(1))
            .ok_or(())?;
        EmbassyClock.wait_until(at).await;
        Ok(())
    }

    fn stop_walker(&mut self) -> bool {
        let Some(ring) = self.ring.take() else {
            return self.halted.is_some();
        };
        match ring.try_stop(self.access.rx_dma()) {
            Ok(halted) => {
                self.halted = Some(halted);
                true
            }
            Err((ring, _)) => {
                self.ring = Some(ring);
                false
            }
        }
    }
}

/// One transmit slot over `descriptor`, with buffer `index` of `pool`
/// attached for the whole run.
fn slot(
    cell: &'static StaticCell<TxSlot<TX_BUFFER>>,
    descriptor: oer_chips_esp32s31_hal::system::memory::dma_static::Placed<TxDmaStorage>,
    pool: &PlacedTxPool<TX_BUFFER, 0, 0, 2>,
    index: u8,
) -> Result<core::pin::Pin<&'static mut TxSlot<TX_BUFFER>>, oer_portable_diagnostics::Name<32>> {
    let dma = TxDmaStorage::pin::<ChipDma, _>(descriptor)
        .map_err(|_| oer_portable_diagnostics::name!("tx-descriptor-pin"; 32))?;
    let slot = cell
        .try_init(TxSlot::from_dma(dma))
        .ok_or(oer_portable_diagnostics::name!("tx-slot-claimed"; 32))?;
    let mut slot = core::pin::Pin::new(slot);
    // The buffer is written in its entirety before every publication; a
    // network writer publishes it empty only to hand it to the radio.
    pool.claim_network(index)
        .map_err(|_| oer_portable_diagnostics::name!("tx-buffer-claimed"; 32))?
        .publish(oer_portable_memory::FrameLen::constant::<0>(), |_| {});
    let backing = pool
        .claim_radio(index)
        .map_err(|_| oer_portable_diagnostics::name!("tx-buffer-radio"; 32))?;
    slot.as_mut()
        .attach(backing)
        .map_err(|_| oer_portable_diagnostics::name!("tx-buffer-attach"; 32))?;
    Ok(slot)
}

oer_chips_family_platform_executor::time_driver!(Esp32s31);
