//! One bounded RX traffic capture with real take/recycle. No live CPU buffer write.
use crate::usb;
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_board as board;
use oer_esp32s31_compose::Esp32s31;
use oer_esp32s31_hal::{
    ieee80211::{
        dma::{descriptor::RxDescriptor, rx_storage::RxDmaStorage},
        init::{
            MAC_COLD_HANDSHAKE_DEADLINE, MacDelayEntropy, MacSlowClockCalibration,
            MacSlowClockCalibrationSource,
        },
    },
    peripherals::usb_serial_jtag::ConsoleInterrupt,
    peripherals::watchdog::{ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters},
    peripherals::{
        efuse::{FactoryIdentity, Interface},
        software_interrupt::{FromCpu0Irq, FromCpu2Irq},
        system_timer::{SystimerAlarm0, SystimerAlarm1},
    },
    power::ChipClocks,
    root::Root,
};
use oer_esp32s31_phy::{
    NoopPhyTargetObserver, PhyCalibrationIdentity, PhyConfig, PhyRegisterConfig,
    concurrent::ConcurrentPhy, phy_get_rf_cal_version,
};
use oer_esp32s31_radio_client::{LpClock, RcSlow};
use oer_esp32s31_radio_system::Esp32s31Radio;
use oer_espressif_executor::{Executor, InterruptExecutor};
use oer_ieee80211_wire::channel::{WifiChannel, WifiChannelWidth};
use oer_radio_regulatory::{Country, RegulatoryDomain};
use oer_time::{Clock, Duration, Timer};
use static_cell::StaticCell;

#[path = "traffic.rs"]
mod traffic;

const COUNT: usize = 32;
const BUFFER: usize = 128;
const NODES: usize = COUNT + 1;
const CHANNEL: u8 = 13;
// The active 200–209 profile keeps full patterns in these three buffers.
const FULL_PATTERN_NODES: [usize; 3] = [12, 20, 28];
const WORDS: usize = BUFFER / 4;
const _: () = assert!(BUFFER >= 4 && BUFFER.is_multiple_of(4) && COUNT <= u32::BITS as usize);
type Storage = RxDmaStorage<COUNT, BUFFER, BUFFER>;
type Rng = oer_esp32s31_hal::peripherals::entropy::Entropy<EmbassyClock>;
type Backend = Esp32s31Radio<
    ChipClocks,
    oer_espressif_executor::Cooperative<Esp32s31, EmbassyClock>,
    NoopPhyTargetObserver,
    crate::phy_pilot::Client,
>;
static DIRECT_BACKEND: StaticCell<Backend> = StaticCell::new();
struct DirectOutput {
    access: oer_esp32s31_hal::owner::RadioAccess,
    _clocks: oer_esp32s31_hal::ieee80211::client::WifiClocksOn,
}
static DIRECT_OUTPUT: StaticCell<DirectOutput> = StaticCell::new();
static DIRECT_READY: oer_memory::ClaimCell<
    Result<&'static mut DirectOutput, oer_diagnostics::Name<32>>,
> = oer_memory::ClaimCell::new();
oer_esp32s31_hal::dma_static! { static RX: Storage = zeroed in ".dma.bss.rx_chain_probe"; }

use crate::phy_pilot;

// SAFETY: the production runtime script maps .hot.text.* to executable SRAM;
// each generated wrapper retains the interrupt-table ABI and owned part.
oer_espressif_interrupt_table::interrupt_table! {
    lines: oer_espressif_staged_runtime::interrupts::LINES,
    handler_attributes: [#[unsafe(link_section = ".hot.text.rx_chain_vectors")]];
    contexts { app: 1; radio_exec: 2; radio_irq: 3; }
    wake: WakeToken = FromCpu0Irq => oer_espressif_executor::wake_handler::<Esp32s31, 0, FromCpu0Irq>, line 40, context app, hart 0, Hardware;
    // SOURCE: family.toml [interrupts]; HW23_S31_P2_ARBITRATION_20261027:
    // only P2 line17 before16 is measured on S31.
    phy: PhyToken = oer_esp32s31_hal::peripherals::timg0_alarm::Timg0AlarmInterrupt => phy_pilot::interrupt, line 16, context radio_exec, hart 0, Exempt(ExecutorLevel, "SAR/PWDET steps between radio executor rounds");
    radio: RadioToken = FromCpu2Irq => oer_espressif_executor::Round<RadioTasks>, line 17, context radio_exec, hart 0, Executor;
    radio_alarm: RadioAlarm = SystimerAlarm0 => oer_espressif_executor::alarm_interrupt::<RadioTasks, 0, SystimerAlarm0>, line 18, context radio_irq, hart 0, Hardware;
    console_usb: ConsoleUsb = ConsoleInterrupt => crate::usb::interrupt, line 41, context app, hart 0, Hardware;
    app_alarm: AppAlarm = SystimerAlarm1 => oer_espressif_executor::alarm_interrupt::<AppTasks, 1, SystimerAlarm1>, line 42, context app, hart 0, Hardware;
}
oer_espressif_executor::tasks! {
    pub RadioTasks: InterruptExecutor<Esp32s31, 2> {
        direct(backend: &'static mut Backend, lp: LpClock<RcSlow>, wifi: oer_esp32s31_hal::root::WifiPartition, rng: Rng, station: [u8;6], ap: [u8;6]) = direct_start;
    }
}
oer_espressif_executor::tasks! {
    AppTasks: Executor<Esp32s31, 0> {
        capture(boot: Boot) = capture;
        watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_watchdog;
    }
}
struct Boot {
    console: usb::Console,
    _pads: oer_esp32s31_hal::peripherals::usb_serial_jtag::UsbPads,
    mac_irq: oer_esp32s31_hal::owner::MacInterruptRegisters,
    traffic: &'static mut traffic::Trace,
}
oer_esp32s31_hal::phy_fault_reporter!(oer_esp32s31_compose::phy_fault::reset);
oer_espressif_staged_runtime::entry!(
    main,
    oer_esp32s31_hal::platform::Port,
    hook = crate::diagnostic::panic,
    exception = crate::diagnostic::exception
);
fn main(handoff: oer_espressif_staged_runtime::Handoff<oer_esp32s31_hal::platform::Port>) -> ! {
    let Some(root) = Root::take() else {
        fail(oer_diagnostics::name!("root"; 32));
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
    ) = match oer_espressif_staged_runtime::adoption::run::<oer_esp32s31_compose::Esp32s31, _, _>(
        handoff,
        root.into_runtime(),
        INTERRUPT_TABLE,
        board::BootstrapPsram,
        oer_esp32s31_compose::phy_fault::report_previous,
        |hardware| {
            let _reset_causes = oer_esp32s31_compose::reset_causes::read(hardware.reset_cause);

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
        fail(oer_diagnostics::name!("interrupts"; 32));
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
            oer_esp32s31_compose::phy_fault::identity(),
        )
        .unwrap_or_else(|failure| {
            let fault = failure.fault();
            let _owners = failure;
            oer_esp32s31_compose::phy_fault::reset(fault)
        });
    let Some(lp) = LpClock::<RcSlow>::claim() else {
        fail(oer_diagnostics::name!("claim"; 32));
    };
    let phy_pilot = crate::phy_pilot::init(runtime_timg0_alarm, adopted.router(), interrupts.phy);
    let backend = Esp32s31Radio::new(
        domain,
        partitions.wifi_coex,
        runtime_clocks,
        oer_espressif_executor::Cooperative::<Esp32s31, _>::new(EmbassyClock),
        NoopPhyTargetObserver,
        PhyRegisterConfig::new(identity),
        phy_pilot,
    );
    let Some(backend) = DIRECT_BACKEND.try_init(backend) else {
        fail(oer_diagnostics::name!("direct-backend"; 32));
    };
    let timer = runtime_system_timer;
    let mut timer_ready = oer_espressif_executor::init::<Esp32s31, 2, _, _>(
        oer_espressif_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        adopted.router(),
        interrupts.radio_alarm,
        timer.alarm0,
    )
    .unwrap_or_else(|failure| failure.reset());
    oer_espressif_executor::bind::<Esp32s31, 0, _, _>(
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
    let traffic = traffic::take().unwrap_or_else(|reason| fail(reason));
    if let Err(reason) = traffic::checkpoint(traffic, 200, 0, &wifi_irqs.mac) {
        fail(reason);
    }
    let software = runtime_software_interrupts;
    let (hart, router) = adopted.parts();
    let radio = InterruptExecutor::<Esp32s31, 2>::new(software.line2)
        .start(hart, router, interrupts.radio, software.irq2, ())
        .unwrap_or_else(|failure| failure.reset());
    let rng = Rng::new(runtime_entropy, EmbassyClock);
    if RadioTasks::direct(&radio, backend, lp, partitions.wifi, rng, station, ap).is_err() {
        fail(oer_diagnostics::name!("direct-task"; 32));
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
        traffic,
    };
    match Executor::<Esp32s31, 0>::new(software.line0)
        .run(
            &hart,
            &mut router,
            interrupts.wake,
            software.irq0,
            (),
            move |spawner| {
                if AppTasks::capture(spawner, boot).is_err()
                    || AppTasks::watchdog(spawner, runtime_watchdogs.rtc).is_err()
                {
                    fail(oer_diagnostics::name!("task"; 32));
                }
            },
        )
        .unwrap_or_else(|failure| failure.reset()) {}
}
fn fail<const B: usize>(reason: oer_diagnostics::Name<B>) -> ! {
    oer_espressif_staged_runtime::fatal::boot_failure::<oer_esp32s31_hal::platform::Port>(
        reason.as_str(),
        || {
            let _ = oer_esp32s31_hal::peripherals::usb_serial_jtag::fatal_note(
                &oer_diagnostics::Note::runtime_fail(reason),
            );
        },
    )
}
type DirectRun =
    impl core::future::Future<Output = Result<DirectOutput, oer_diagnostics::Name<32>>>;
type DirectPrepare<'a> = impl core::future::Future<
        Output = Result<
            oer_esp32s31_radio_client::PhyPreparation,
            oer_esp32s31_radio_system::Fault,
        >,
    > + 'a;
type DirectTrack<'a> = impl core::future::Future<
        Output = Result<
            oer_esp32s31_radio_client::wifi::WifiTracked,
            oer_esp32s31_radio_client::Failed<oer_esp32s31_hal::ieee80211::client::WifiClocked>,
        >,
    > + 'a;
static PREPARE: static_cell::ConstStaticCell<oer_memory::StaticPlace<DirectPrepare<'static>>> =
    static_cell::ConstStaticCell::new(oer_memory::StaticPlace::new());
static TRACK: static_cell::ConstStaticCell<oer_memory::StaticPlace<DirectTrack<'static>>> =
    static_cell::ConstStaticCell::new(oer_memory::StaticPlace::new());
#[define_opaque(DirectPrepare)]
fn prepare_future(backend: &mut Backend) -> DirectPrepare<'_> {
    use oer_esp32s31_radio_system::PhyDomain;
    backend.prepare()
}
#[define_opaque(DirectTrack)]
fn track_future(
    backend: &mut Backend,
    request: oer_esp32s31_radio_client::wifi::WifiTrack,
) -> DirectTrack<'_> {
    use oer_esp32s31_radio_system::ClientBackend;
    backend.track(request)
}
#[inline(never)]
fn placed_prepare(
    backend: &mut Backend,
) -> Result<oer_memory::PlacedFuture<'_, DirectPrepare<'_>>, oer_diagnostics::Name<32>> {
    let place = PREPARE
        .try_take()
        .ok_or(oer_diagnostics::name!("direct-prepare-place"; 32))?;
    Ok(place.future(|| prepare_future(backend)))
}
#[inline(never)]
fn placed_track(
    backend: &mut Backend,
    request: oer_esp32s31_radio_client::wifi::WifiTrack,
) -> Result<oer_memory::PlacedFuture<'_, DirectTrack<'_>>, oer_diagnostics::Name<32>> {
    let place = TRACK
        .try_take()
        .ok_or(oer_diagnostics::name!("direct-track-place"; 32))?;
    Ok(place.future(|| track_future(backend, request)))
}
static DIRECT_RUN: static_cell::ConstStaticCell<oer_memory::StaticPlace<DirectRun>> =
    static_cell::ConstStaticCell::new(oer_memory::StaticPlace::new());
#[define_opaque(DirectRun)]
fn direct_future(
    backend: &'static mut Backend,
    lp: LpClock<RcSlow>,
    wifi: oer_esp32s31_hal::root::WifiPartition,
    rng: Rng,
    station: [u8; 6],
    ap: [u8; 6],
) -> DirectRun {
    async move {
        use oer_esp32s31_hal::ieee80211::{client::WifiCold, init::MacColdStartConfig};
        use oer_esp32s31_radio_client::wifi::{WifiJoin, WifiTrack};
        use oer_esp32s31_radio_system::ClientBackend;
        let channel = WifiChannel::new_2_4_ghz(CHANNEL, WifiChannelWidth::Mhz20)
            .map_err(|_| oer_diagnostics::name!("direct-channel"; 32))?;
        let domain = Country::new(*b"DE")
            .ok()
            .and_then(RegulatoryDomain::of)
            .ok_or(oer_diagnostics::name!("direct-domain"; 32))?;
        let channel = domain
            .permit(oer_ieee80211_contracts_port::RegulatedChannel::from(
                channel,
            ))
            .map_err(|_| oer_diagnostics::name!("direct-permit"; 32))?;
        let tx_ceiling = channel.tx_ceiling(board::wifi_power_limits(
            i32::from(PhyConfig::production().highest_target_power_quarter_dbm()) * 25,
            Some(1000),
        ));
        let phy = placed_prepare(backend)?
            .await
            .map_err(|_| oer_diagnostics::name!("direct-prepare"; 32))?;
        let joined = backend
            .enter(
                WifiJoin {
                    wifi: WifiCold::from_partition(wifi),
                },
                phy,
                &lp,
            )
            .await
            .map_err(|_| oer_diagnostics::name!("direct-enter"; 32))?;
        let mut platform = Platform { rng };
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
                delay_random: platform.mac_delay_random(),
            },
        )?
        .await
        .map_err(|_| oer_diagnostics::name!("direct-track"; 32))?;
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
    wifi: oer_esp32s31_hal::root::WifiPartition,
    rng: Rng,
    station: [u8; 6],
    ap: [u8; 6],
) -> oer_memory::PlacedFuture<'static, DirectRun> {
    let Some(place) = DIRECT_RUN.try_take() else {
        fail(oer_diagnostics::name!("direct-future-claimed"; 32));
    };
    place.future(|| direct_future(backend, lp, wifi, rng, station, ap))
}
async fn direct_start(
    backend: &'static mut Backend,
    lp: LpClock<RcSlow>,
    wifi: oer_esp32s31_hal::root::WifiPartition,
    rng: Rng,
    station: [u8; 6],
    ap: [u8; 6],
) {
    let result = match placed_direct(backend, lp, wifi, rng, station, ap).await {
        Ok(output) => DIRECT_OUTPUT
            .try_init(output)
            .ok_or(oer_diagnostics::name!("direct-output"; 32)),
        Err(reason) => Err(reason),
    };
    if DIRECT_READY.install(result).is_err() {
        fail(oer_diagnostics::name!("direct-publish"; 32));
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
struct Platform {
    rng: Rng,
}
impl MacDelayEntropy for Platform {
    fn mac_delay_random(&mut self) -> u32 {
        self.rng.random()
    }
}
impl MacSlowClockCalibrationSource for Platform {
    fn mac_slow_clock_calibration(&mut self) -> MacSlowClockCalibration {
        MacSlowClockCalibration::Unavailable
    }
}

fn node(storage: &Storage, index: usize) -> Option<&RxDescriptor> {
    if index == COUNT {
        Some(storage.reserved_descriptor())
    } else {
        storage.descriptors().get(index)
    }
}
// This exception is limited to this probe and explicitly authorized by the
// experiment: an asynchronous DMA observation, never a Rust buffer reference.
#[allow(
    unsafe_code,
    reason = "probe-only volatile observation of retained DMA bytes"
)]
fn read_word(
    storage: &Storage,
    descriptor: &RxDescriptor,
    expected: u32,
    offset: usize,
) -> Result<u32, oer_diagnostics::Name<32>> {
    let address = descriptor.buffer_address();
    let base = core::ptr::from_ref(storage).addr();
    let limit = base
        .checked_add(core::mem::size_of::<Storage>())
        .ok_or(oer_diagnostics::name!("arena-end"; 32))?;
    let end = (address as usize)
        .checked_add(BUFFER)
        .ok_or(oer_diagnostics::name!("buffer-end"; 32))?;
    if !offset.is_multiple_of(4)
        || offset.checked_add(4).is_none_or(|end| end > BUFFER)
        || address != expected
        || !(address as usize).is_multiple_of(4)
        || (address as usize) < base
        || end > limit
        || usize::from(descriptor.word0().size()) != BUFFER
    {
        return Err(oer_diagnostics::name!("buffer-binding"; 32));
    }
    // SAFETY: production placement/preparation published expected from this
    // permanent arena before RX started. Address identity, u32 alignment and
    // full advertised capacity containment are checked above. CPU initialization ends before publication; during observation only DMA writes
    // this storage; this load creates no reference and grants no ownership.
    Ok(unsafe { (address as *const u32).byte_add(offset).read_volatile() })
}

fn pattern(index: usize, word: usize) -> u32 {
    if word == 0 && !FULL_PATTERN_NODES.contains(&index) {
        // Diagnostic initialization before BASE publication; retain the
        // same byte profile as the recorded 200–209 observations.
        0xdead_beef
    } else {
        0xa500_0000 | ((index as u32) << 12) | (word as u32)
    }
}

// This probe paints patterns before BASE publication, while the stopped
// MAC cannot receive. No production constructor is changed.
#[allow(
    unsafe_code,
    reason = "probe-only initialization before DMA publication"
)]
fn prepare_patterns(storage: &Storage) -> Result<(), oer_diagnostics::Name<32>> {
    for index in 0..NODES {
        let descriptor = node(storage, index).ok_or(oer_diagnostics::name!("node"; 32))?;
        let address = descriptor.buffer_address();
        // Reuse the observation's complete arena/alignment/capacity checks.
        let _ = read_word(storage, descriptor, address, 0)?;
        for word in 0..WORDS {
            // SAFETY: RX and its walker are stopped and BASE has not been
            // published. The checked permanent allocation belongs only to
            // this retained probe arena; no byte reference is created.
            unsafe {
                (address as *mut u32)
                    .add(word)
                    .write_volatile(pattern(index, word));
            }
        }
    }
    oer_riscv::fence();
    Ok(())
}

async fn capture(boot: Boot) {
    let Boot {
        mut console,
        _pads,
        mac_irq,
        traffic,
    } = boot;
    usb::text(&mut console, b"RXCHAIN boot\n").await;
    let result = run_direct(&mut console, traffic, mac_irq).await;
    if let Err(reason) = result {
        usb::text(&mut console, reason.as_str().as_bytes()).await;
        usb::text(&mut console, b"\nOER-RESULT: fail rx-chain\n").await;
    } else {
        usb::text(&mut console, b"OER-RESULT: pass rx-chain\n").await;
    }
}
async fn run_direct(
    console: &mut usb::Console,
    trace: &mut traffic::Trace,
    mut mac_irq: oer_esp32s31_hal::owner::MacInterruptRegisters,
) -> Result<(), oer_diagnostics::Name<32>> {
    let deadline = EmbassyClock
        .now()
        .checked_add(Duration::from_secs(5))
        .ok_or(oer_diagnostics::name!("direct-deadline"; 32))?;
    let output = loop {
        if let Ok(result) = DIRECT_READY.take() {
            break result?;
        }
        if EmbassyClock.now() >= deadline {
            return Err(oer_diagnostics::name!("direct-ready-timeout"; 32));
        }
        EmbassyClock
            .wait_until(
                EmbassyClock
                    .now()
                    .checked_add(Duration::from_millis(1))
                    .ok_or(oer_diagnostics::name!("direct-wait"; 32))?,
            )
            .await;
    };
    let access = &mut output.access;
    traffic::checkpoint(trace, 201, 0, &mac_irq)?;
    let placed = RxDmaStorage::place(RX.take().ok_or(oer_diagnostics::name!("rx-claimed"; 32))?)
        .map_err(|_| oer_diagnostics::name!("rx-place"; 32))?;
    let storage = placed.storage();
    let prepared = placed
        .prepare_ring(access.rx_dma())
        .map_err(|_| oer_diagnostics::name!("rx-prepare"; 32))?;
    prepare_patterns(storage)?;
    let ready = access.wifi_mac_hal().prepare_mac_runtime(prepared);
    let stopped_ring = ready
        .publish(access.rx_dma())
        .map_err(|_| oer_diagnostics::name!("rx-publish"; 32))?;
    let mut ring = stopped_ring
        .try_start(access.rx_dma())
        .map_err(|_| oer_diagnostics::name!("rx-start"; 32))?;
    let stale = mac_irq.mac_interrupt_raw();
    if stale.fatal_assertion_raw().is_some() {
        return Err(oer_diagnostics::name!("traffic-initial-fatal"; 32));
    }
    mac_irq.acknowledge_mac_interrupt_raw(stale);
    access.wifi_mac_hal().configure_open_promiscuous_receive();
    mac_irq.mask_all_mac_interrupts();
    traffic::checkpoint(trace, 202, 0, &mac_irq)?;
    usb::text(
        console,
        b"TRAFFIC begin v=1 selections=200-209 channel=13 cap_ms=700 mask_ms=350 take_recycle=1\n",
    )
    .await;
    let observation = traffic::collect(trace, storage, &mut ring, access, &mut mac_irq);
    access.wifi_mac_hal().request_channel_stop();
    let deadline = EmbassyClock
        .now()
        .checked_add(Duration::from_millis(50))
        .ok_or(oer_diagnostics::name!("traffic-stop-deadline"; 32))?;
    while access.wifi_mac_hal().channel_active_state() != 0 {
        if EmbassyClock.now() >= deadline {
            return Err(oer_diagnostics::name!("traffic-mac-stop"; 32));
        }
        EmbassyClock
            .wait_until(
                EmbassyClock
                    .now()
                    .checked_add(Duration::from_millis(1))
                    .ok_or(oer_diagnostics::name!("traffic-stop-wait"; 32))?,
            )
            .await;
    }
    let stopped = ring
        .try_stop(access.rx_dma())
        .map_err(|_| oer_diagnostics::name!("traffic-walker-stop"; 32));
    let clear = traffic::clears(trace, &mut mac_irq);
    traffic::dump(trace, console).await?;
    observation?;
    stopped?;
    clear?;
    // Static backend/output retain every power/clock/register owner until the
    // leased host reset; this diagnostic returns no reusable radio/storage.
    Ok(())
}

oer_espressif_executor::time_driver!(Esp32s31);
