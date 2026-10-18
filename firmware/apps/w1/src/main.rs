#![no_main]
#![no_std]

//! W1: a passive scan on the product receive path. The radio system brings
//! Wi-Fi up on the configured channel (as W0); the receive runner binds its
//! arena on the radio executor (P2), receives with the MAC and WDEVPWR
//! interrupts at P3 and hands staged frames to the application (P1), which
//! feeds them to the survey for a 10 s window from the first frame. A timer
//! stress phase, a hold of every staging lease (the walker exhausts and
//! restarts) and the stop follow; after the exit the image prints its
//! report block and verdict ([`report`]).
//!
//! Nothing is transmitted. The stand's BSSID and channel come from the
//! build (`OER_W1_EXPECT_BSSID`, `OER_W1_CHANNEL`); without them the image
//! prints `OER-RESULT: fail w1 unconfigured`. No SSID, unmasked BSSID or
//! base MAC is printed.

mod irq;
mod measure;
mod report;

use core::{convert::Infallible, fmt::Write as _};

use embassy_executor::SendSpawner;
use embassy_futures::select::{Either, select};
use esp_hal::{
    Blocking, efuse, interrupt::software::SoftwareInterrupt, rng::Rng,
    usb::usb_serial_jtag::UsbSerialJtag,
};
use oer_esp32s31_hal::root::ProtocolPartitions;
use oer_esp32s31_ieee80211::{
    runtime::{WifiRoleMaterialized, materialize_esp32s31_wifi_role},
    rx::{returns::HeldRxFrame, upper::FrameReceiver},
    startup::{
        RadioStartConfig, RadioStartFailure, WifiColdStartConfig, WifiMacPlatform,
        WifiMacStartConfig, start_esp32s31_radio,
    },
};
use oer_esp32s31_ieee80211_mac::{
    init::{
        MAC_COLD_RX_INTERRUPT_MASK, MacDelayEntropy, MacSlowClockCalibration,
        MacSlowClockCalibrationSource,
    },
    portable::rx_meta,
};
use oer_esp32s31_phy::{
    HARDWARE_EDGE_LIMIT, NoopPhyTargetObserver, PhyCalibrationIdentity, PhyRegisterConfig,
    PhyTxTargetPowerProfile, concurrent::ConcurrentPhy, phy_get_rf_cal_version,
};
use oer_esp32s31_platform_board as board;
use oer_esp32s31_radio_client::{LpClock, PhyPreparation, RcSlow, claim};
use oer_esp32s31_radio_esp_hal::{PlatformClocks, RadioSingletons, take_radio};
use oer_esp32s31_radio_system::{Esp32s31Radio, RadioSystem};
use oer_espressif_executor_embassy::{
    Executor, InterruptExecutor, LevelOrder, timer_observation::Window,
};
use oer_espressif_staged_runtime::{panic, stacks};
use oer_ieee80211_mac::{
    address::{Bssid, IndividualAddress},
    channel::{Channel, WifiChannel, WifiChannelWidth},
};
use oer_ieee80211_runtime::survey::Survey;
use oer_ieee80211_softmac::WifiMacAddress;
use oer_memory::{AffineSpscQueue, AffineSpscReceiver};
use oer_time::{Clock, Duration, Instant, Timer};
use oer_time_embassy::EmbassyClock;
use static_cell::StaticCell;

use crate::{
    irq::{BUFFER, Bound, ImageDoorbell, ImageRoutes, P1Ends, SLOTS},
    measure::{Phase, TickStamped, TimedEspHal, TimerLine},
};

/// The stand W1 checks.
#[derive(Clone, Copy)]
pub struct W1Config {
    /// The 2.4 GHz channel, 1 to 13.
    pub channel: u8,
    /// The stand AP's BSSID: unicast, nonzero.
    pub stand: [u8; 6],
}

include!(concat!(env!("OUT_DIR"), "/w1_config.rs"));

oer_espressif_staged_runtime::interrupt_table! {
    /// The application executor's wake (thread mode).
    wake: WakeToken = FROM_CPU_INTR0 => oer_espressif_executor_embassy::wake_handler::<0>, Priority1, ProCpu;
    /// The radio executor (P2), polled in its handler, which times each run.
    radio: RadioToken = FROM_CPU_INTR2 => crate::irq::radio_executor, Priority2, ProCpu;
    /// The radio doorbell: P2's requests to the interrupt line.
    doorbell: DoorbellToken = FROM_CPU_INTR3 => crate::irq::doorbell, Priority3, ProCpu;
    /// The Wi-Fi MAC interrupt.
    wifi_mac: WifiMacToken = MODEM_WIFI_MAC => crate::irq::wifi_mac, Priority3, ProCpu;
    /// The Wi-Fi WDEVPWR interrupt.
    wifi_pwr: WifiPwrToken = MODEM_WIFI_PWR => crate::irq::wifi_pwr, Priority3, ProCpu;
    /// The radio executor's timer alarm; it only acknowledges and pends.
    radio_alarm: RadioAlarmToken = SYSTIMER_TARGET0 => oer_espressif_executor_embassy::alarm_interrupt::<0>, Priority3, ProCpu;
    /// The application executor's timer alarm.
    app_alarm: AppAlarmToken = SYSTIMER_TARGET1 => oer_espressif_executor_embassy::alarm_interrupt::<1>, Priority1, ProCpu;
}

/// The radio executor between the radio interrupts and the application.
const LEVELS: LevelOrder<WifiMacToken, RadioToken, WakeToken> = LevelOrder::new();

/// The survey window, from the first frame.
const WINDOW: Duration = Duration::from_millis(10_000);
const WINDOW_MS: u32 = 10_000;
/// How long the first frame may take.
const FIRST_FRAME: Duration = Duration::from_millis(5_000);
/// How long the receive runner may take to bind its arena.
const BIND: Duration = Duration::from_millis(2_000);
const BIND_POLL: Duration = Duration::from_millis(1);
/// The timer stress phase.
const STRESS: Duration = Duration::from_millis(5_000);
/// How long the hold may take to collect every staging lease.
const HOLD_FILL: Duration = Duration::from_millis(3_000);
/// How long the full hold lasts.
const HOLD: Duration = Duration::from_millis(1_000);
/// How long frames are counted after the release.
const AFTER_RELEASE: Duration = Duration::from_millis(2_000);

type Console = UsbSerialJtag<'static, Blocking>;
type Held = HeldRxFrame<'static, 'static, SLOTS, BUFFER>;
type Receiver = FrameReceiver<'static, SLOTS, BUFFER>;

static EXECUTOR: StaticCell<Executor<0>> = StaticCell::new();
static RADIO_EXECUTOR: StaticCell<InterruptExecutor<2>> = StaticCell::new();
static BOUND: StaticCell<AffineSpscQueue<Bound, 1>> = StaticCell::new();
static TIMER_LINES: StaticCell<AffineSpscQueue<TimerLine, 2>> = StaticCell::new();
static SURVEY: StaticCell<Survey<{ report::BSS }>> = StaticCell::new();

type Actor = RadioSystem<
    oer_esp32s31_radio_client::wifi::Wifi,
    RcSlow,
    TickStamped<Esp32s31Radio<PlatformClocks<TimedEspHal>, EmbassyClock, NoopPhyTargetObserver>>,
    EmbassyClock,
>;

/// What the application task needs from the boot.
struct Boot {
    console: Console,
    cell: oer_esp32s31_radio_client::MembershipCell<oer_esp32s31_radio_client::wifi::Wifi>,
    wifi: oer_esp32s31_hal::root::WifiPartition,
    station: [u8; 6],
    access_point: [u8; 6],
    radio: SendSpawner,
    routes: ImageRoutes,
    doorbell: ImageDoorbell,
    bound: (
        oer_memory::AffineSpscSender<'static, Bound, 1>,
        AffineSpscReceiver<'static, Bound, 1>,
    ),
    timer_lines: AffineSpscReceiver<'static, TimerLine, 2>,
}

#[unsafe(no_mangle)]
extern "C" fn runtime_main() -> ! {
    // Before anything can panic and overwrite it.
    let previous = panic::take_previous();
    let peripherals = esp_hal::init(esp_hal::Config::default());
    // SAFETY: `runtime_main` runs once on CPU0, right after `_runtime_start`,
    // with interrupts disabled and the bootstrap's mapping intact, which
    // nothing has reset or remapped; interrupts stay disabled until the
    // executors, the doorbell and the time driver are bound below.
    let psram = unsafe {
        oer_espressif_staged_runtime::adopt(INTERRUPT_TABLE, || {
            board::adopt_psram_mapping(peripherals.PSRAM)
        })
    };
    let mut console = UsbSerialJtag::new(peripherals.USB_DEVICE);
    if let Some(record) = previous {
        let _ = write!(console, "OER-PANIC previous {record}\r\n");
    }
    let Some(interrupts) = Interrupts::take() else {
        let _ = console.write(b"OER-RESULT: fail w1 interrupt-table\r\n");
        park()
    };

    let base_mac = efuse::base_mac_address();
    let mut base_mac_address = [0; 6];
    base_mac_address.copy_from_slice(base_mac.as_bytes());
    let identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        base_mac_address,
        mac_extension: efuse::read_field_le::<u16>(efuse::MAC_EXT),
    };
    let mut station = [0; 6];
    station.copy_from_slice(
        efuse::interface_mac_address(efuse::InterfaceMacAddress::Station).as_bytes(),
    );
    let mut access_point = [0; 6];
    access_point.copy_from_slice(
        efuse::interface_mac_address(efuse::InterfaceMacAddress::AccessPoint).as_bytes(),
    );

    let singletons = RadioSingletons {
        i2c_ana_mst: peripherals.I2C_ANA_MST,
        modem_syscon: peripherals.MODEM_SYSCON,
        modem_lpcon: peripherals.MODEM_LPCON,
        wifi: peripherals.WIFI,
        bt: peripherals.BT,
        ieee802154: peripherals.IEEE802154,
        lp_tsens: peripherals.LP_TSENS,
    };
    // esp-hal's clocks come back as a provider over its backend; the image
    // times the same backend instead.
    let (hardware, _esp_hal_clocks) = match take_radio(singletons, &psram) {
        Ok(radio) => radio,
        Err(error) => {
            let _ = write!(console, "OER-RESULT: fail w1 take_radio={error:?}\r\n");
            park()
        }
    };
    let (domain, partitions) = hardware.split(ConcurrentPhy::new());
    let (Some((end, cells)), Some(lp)) = (claim(), LpClock::<RcSlow>::claim()) else {
        let _ = console.write(b"OER-RESULT: fail w1 claim\r\n");
        park()
    };
    let ProtocolPartitions {
        wifi, wifi_coex, ..
    } = partitions;
    let backend = TickStamped(Esp32s31Radio::new(
        domain,
        wifi_coex,
        PlatformClocks::<TimedEspHal>::new(),
        EmbassyClock,
        NoopPhyTargetObserver,
        PhyRegisterConfig::new(identity),
    ));
    let actor = RadioSystem::new(end, backend, EmbassyClock, lp);

    // The radio executor writes the system timer; the application executor
    // publishes its deadlines to it.
    oer_espressif_executor_embassy::init::<2, _>(peripherals.SYSTIMER, interrupts.radio_alarm);
    oer_espressif_executor_embassy::bind::<0, _>(interrupts.app_alarm);
    let doorbell = match ImageDoorbell::new(
        SoftwareInterrupt::new(peripherals.FROM_CPU_INTR3),
        interrupts.doorbell,
    ) {
        Ok(doorbell) => doorbell,
        Err(error) => {
            let _ = write!(console, "OER-RESULT: fail w1 doorbell={error:?}\r\n");
            park()
        }
    };
    let (timer_sender, timer_lines) = TIMER_LINES.init(AffineSpscQueue::new()).split();
    let radio = RADIO_EXECUTOR
        .init(InterruptExecutor::<2>::new(SoftwareInterrupt::new(
            peripherals.FROM_CPU_INTR2,
        )))
        .start(interrupts.radio, LEVELS);
    // Each task's one slot is free: these are their only spawns.
    if let Ok(token) = radio_actor(actor) {
        radio.spawn(token);
    }
    if let Ok(token) = measure::p2_timers(timer_sender) {
        radio.spawn(token);
    }
    let executor = EXECUTOR.init(Executor::<0>::new(SoftwareInterrupt::new(
        peripherals.FROM_CPU_INTR0,
    )));
    // SAFETY: the vectors, the interrupt stack, the time driver, the radio
    // executor, the doorbell and, from `run`, the application executor's
    // wake are bound; the MAC and WDEVPWR sources stay silent until the
    // receive runner enables their routes, and interrupts were disabled
    // since the handoff.
    unsafe { oer_espressif_staged_runtime::enable_interrupts_after_handoff() };
    let boot = Boot {
        console,
        cell: cells.wifi,
        wifi,
        station,
        access_point,
        radio,
        routes: ImageRoutes {
            mac: interrupts.wifi_mac,
            power: interrupts.wifi_pwr,
        },
        doorbell,
        bound: BOUND.init(AffineSpscQueue::new()).split(),
        timer_lines,
    };
    executor.run(interrupts.wake, move |spawner| {
        if let Ok(token) = w1(boot) {
            spawner.spawn(token);
        }
    })
}

/// The radio system's actor, on the radio executor (P2), each poll timed.
#[embassy_executor::task]
async fn radio_actor(actor: Actor) {
    match measure::timed(actor.run(), &measure::ACTOR, || {}).await {}
}

/// The run, on the application executor.
#[embassy_executor::task]
async fn w1(boot: Boot) {
    let mut console = boot.console;
    let Some(config) = CONFIG else {
        let _ = console.write(b"OER-RESULT: fail w1 unconfigured\r\n");
        return;
    };
    // The build accepts only a unicast, nonzero BSSID.
    let Some(stand) = IndividualAddress::new(config.stand).and_then(Bssid::new) else {
        let _ = console.write(b"OER-RESULT: fail w1 stand\r\n");
        return;
    };
    let parts = Parts {
        cell: boot.cell,
        wifi: boot.wifi,
        station: boot.station,
        access_point: boot.access_point,
        radio: boot.radio,
        routes: boot.routes,
        doorbell: boot.doorbell,
        bound: boot.bound,
        timer_lines: boot.timer_lines,
    };
    if let Err(reason) = run(&mut console, config, stand, parts).await {
        let _ = write!(console, "OER-RESULT: fail w1 {reason}\r\n");
    }
}

/// The boot's owners the run takes.
struct Parts {
    cell: oer_esp32s31_radio_client::MembershipCell<oer_esp32s31_radio_client::wifi::Wifi>,
    wifi: oer_esp32s31_hal::root::WifiPartition,
    station: [u8; 6],
    access_point: [u8; 6],
    radio: SendSpawner,
    routes: ImageRoutes,
    doorbell: ImageDoorbell,
    bound: (
        oer_memory::AffineSpscSender<'static, Bound, 1>,
        AffineSpscReceiver<'static, Bound, 1>,
    ),
    timer_lines: AffineSpscReceiver<'static, TimerLine, 2>,
}

/// One frame the application took: normalized and counted, and in the
/// window handed to the survey.
fn take(
    receiver: &Receiver,
    held: &Held,
    survey: Option<&mut Survey<{ report::BSS }>>,
    channel: Channel,
    admitted: &mut u32,
) {
    let Ok(frame) = receiver.normalize(held) else {
        return;
    };
    *admitted = admitted.saturating_add(1);
    // The runner's publisher marks every handoff.
    if let (Some(survey), Some(times)) = (survey, held.times()) {
        let meta = rx_meta(frame.metadata, channel);
        survey.take(
            EmbassyClock.now(),
            frame.mpdu,
            &meta,
            frame.stamp,
            times.handoff,
        );
    }
}

/// Take and drop frames for ever.
async fn drain(receiver: &mut Receiver, channel: Channel, admitted: &mut u32) -> Infallible {
    loop {
        let held = receiver.next().await;
        take(receiver, &held, None, channel, admitted);
    }
}

fn after(duration: Duration) -> Instant {
    EmbassyClock.now().saturating_add(duration)
}

/// Bring Wi-Fi up, run the phases, stop and print the block. `Err` is a
/// failure before the receive epoch handed over its ends: no block.
async fn run(
    console: &mut Console,
    config: W1Config,
    stand: Bssid,
    parts: Parts,
) -> Result<(), &'static str> {
    let Parts {
        cell,
        wifi,
        station,
        access_point,
        radio,
        routes,
        doorbell,
        bound: (bound_sender, bound),
        timer_lines,
    } = parts;
    let (Ok(station), Ok(access_point)) = (
        WifiMacAddress::new(station),
        WifiMacAddress::new(access_point),
    ) else {
        return Err("efuse-mac");
    };
    let channel =
        WifiChannel::new_2_4_ghz(config.channel, WifiChannelWidth::Mhz20).map_err(|_| "channel")?;
    let start = RadioStartConfig::new(
        WifiColdStartConfig::new(channel),
        WifiMacStartConfig::new(u32::from(HARDWARE_EDGE_LIMIT), station, access_point),
    );
    let (_cell, started) = start_esp32s31_radio(cell, wifi, Platform::default(), start).await;
    let stopped = match started {
        Ok(stopped) => stopped,
        Err(RadioStartFailure::Join { failed, .. }) => {
            let _ = write!(console, "OER w1 join failed cause={:?}\r\n", failed.cause);
            return Err("join");
        }
        Err(RadioStartFailure::Track { failed, .. }) => {
            let _ = write!(console, "OER w1 track failed cause={:?}\r\n", failed.cause);
            return Err("track");
        }
    };
    // The owner and the cell stay held until the image parks.
    let WifiRoleMaterialized {
        owner,
        registers,
        interrupt_setup,
        resources: (),
    } = materialize_esp32s31_wifi_role(stopped, ());
    let start_report = owner.start_report();
    let cold_mask = owner.transition_report().cold_interrupt_mask;
    let applied = owner.current_channel();
    let rx_channel = Channel::from(applied);
    match irq::rx_task(registers, interrupt_setup, routes, doorbell, bound_sender) {
        Ok(token) => radio.spawn(token),
        Err(_) => return Err("rx-spawn"),
    }
    let bind_deadline = after(BIND);
    let bound = loop {
        if let Ok(bound) = bound.try_receive() {
            break bound;
        }
        if EmbassyClock.now() >= bind_deadline {
            return Err("no-bind");
        }
        EmbassyClock.wait_until(after(BIND_POLL)).await;
    };
    let P1Ends {
        mut stop,
        mut receiver,
        counters,
    } = match bound {
        Bound::Ends(ends) => ends,
        Bound::Refused(error) => {
            let _ = write!(console, "OER w1 bind refused {error:?}\r\n");
            return Err("bind");
        }
    };

    let survey = SURVEY.init(Survey::new(stand));
    let mut admitted = 0_u32;
    let mut window_us = 0;
    let mut frames_after_release = 0_u32;
    let mut timers = heapless::Vec::<TimerLine, 4>::new();
    measure::enter(Phase::FirstFrame);
    let first = select(receiver.next(), EmbassyClock.wait_until(after(FIRST_FRAME))).await;
    let heard = matches!(first, Either::First(_));
    if let Either::First(held) = first {
        // The window: every frame from the first one goes to the survey.
        let window_start = EmbassyClock.now();
        measure::enter(Phase::Window);
        let window = Window::begin();
        take(
            &receiver,
            &held,
            Some(&mut *survey),
            rx_channel,
            &mut admitted,
        );
        drop(held);
        let deadline = window_start.saturating_add(WINDOW);
        while let Either::First(held) =
            select(receiver.next(), EmbassyClock.wait_until(deadline)).await
        {
            take(
                &receiver,
                &held,
                Some(&mut *survey),
                rx_channel,
                &mut admitted,
            );
        }
        window_us = EmbassyClock
            .now()
            .saturating_duration_since(window_start)
            .as_micros();
        if let Some(window) = window {
            let _full = timers.push(TimerLine {
                comparator: measure::P1_COMPARATOR,
                phase: Phase::Window,
                report: window.finish(),
            });
        }

        // The timer stress: P2 waits 37 µs, P1 113 µs, frames keep flowing.
        measure::enter(Phase::Stress);
        let window = Window::begin();
        let _stressed = select(
            drain(&mut receiver, rx_channel, &mut admitted),
            measure::p1_stress(after(STRESS)),
        )
        .await;
        if let Some(window) = window {
            let _full = timers.push(TimerLine {
                comparator: measure::P1_COMPARATOR,
                phase: Phase::Stress,
                report: window.finish(),
            });
        }

        // The hold: every staging lease held, the walker exhausts; the
        // release restarts it.
        measure::enter(Phase::Hold);
        let mut holding = heapless::Vec::<Held, SLOTS>::new();
        let deadline = after(HOLD_FILL);
        while !holding.is_full() {
            match select(receiver.next(), EmbassyClock.wait_until(deadline)).await {
                Either::First(held) => {
                    take(&receiver, &held, None, rx_channel, &mut admitted);
                    let _full = holding.push(held);
                }
                Either::Second(()) => break,
            }
        }
        EmbassyClock.wait_until(after(HOLD)).await;
        holding.clear();
        let deadline = after(AFTER_RELEASE);
        while let Either::First(held) =
            select(receiver.next(), EmbassyClock.wait_until(deadline)).await
        {
            take(&receiver, &held, None, rx_channel, &mut admitted);
            frames_after_release = frames_after_release.saturating_add(1);
        }
    }

    // The stop: keep draining until the exit is published.
    measure::enter(Phase::Stop);
    stop.request_stop();
    let exit = loop {
        match select(receiver.next(), stop.exit()).await {
            Either::First(held) => take(&receiver, &held, None, rx_channel, &mut admitted),
            Either::Second(exit) => break exit,
        }
    };
    measure::enter(Phase::Report);

    let irq_free = stacks::current_hart_interrupt_stack_free_bytes();
    while let Ok(line) = timer_lines.try_receive() {
        let _full = timers.push(line);
    }
    let ticks = measure::ticks();
    let rx = counters.report();
    let cpu_hz = esp_hal::clock::cpu_clock().as_hz();

    // Outside the block: what its lines do not carry.
    match start_report.phy {
        PhyPreparation::Registered {
            calibration_path,
            micros,
            ..
        } => {
            let _ = write!(
                console,
                "OER w1 start epoch={} calibration_path={calibration_path:?} registration_us={micros}",
                start_report.epoch.get()
            );
        }
        other => {
            let _ = write!(
                console,
                "OER w1 start epoch={} phy={other:?}",
                start_report.epoch.get()
            );
        }
    }
    let _ = write!(
        console,
        " tracking_us={:?} cold_mask={cold_mask:?} expected_mask={:#010x}\r\n",
        start_report.track.tracking_micros,
        MAC_COLD_RX_INTERRUPT_MASK.bits()
    );
    let _ = write!(
        console,
        "OER w1 irq spurious_mac={} auxiliary_mac={} unhandled_mac={} spurious_power={} route_errors={}\r\n",
        rx.spurious_mac_irqs,
        rx.auxiliary_mac_irqs,
        rx.unhandled_mac_irqs,
        rx.spurious_power_irqs,
        irq::ROUTE_ERRORS.load(core::sync::atomic::Ordering::Relaxed),
    );
    if !heard {
        let _ = console.write(b"OER w1 no frame within 5 s: the window did not open\r\n");
    }
    if exit.cause != oer_esp32s31_ieee80211::rx::control::WifiRxCause::StopRequested
        || exit.end != oer_esp32s31_ieee80211::rx::control::WifiRxEnd::Halted
    {
        let _ = write!(console, "OER w1 exit {exit:?}\r\n");
    }
    let run = report::Run {
        channel: config.channel,
        applied_channel: applied.primary(),
        stand,
        window_ms: WINDOW_MS,
        cpu_hz,
        survey: &*survey,
        window_us,
        rx,
        unserviced_mac: irq::UNSERVICED_MAC.load(core::sync::atomic::Ordering::Relaxed),
        unserviced_pwr: irq::UNSERVICED_PWR.load(core::sync::atomic::Ordering::Relaxed),
        frames_after_release,
        admitted,
        exit,
        ticks: &ticks,
        timers: &timers,
        irq_bytes: stacks::irq_stack_bytes(),
        irq_free,
    };
    let _ = report::print(console, &run);
    // The owners stay held: nothing is released or reset.
    let _owner = owner;
    core::future::pending::<()>().await;
    Ok(())
}

/// Wi-Fi's own platform values for the MAC bring-up.
#[derive(Default)]
struct Platform {
    tx_power: Option<PhyTxTargetPowerProfile>,
}

impl MacDelayEntropy for Platform {
    fn mac_delay_random(&mut self) -> u32 {
        Rng::new().random()
    }
}

impl MacSlowClockCalibrationSource for Platform {
    fn mac_slow_clock_calibration(&mut self) -> MacSlowClockCalibration {
        // W1 calibrates no RTC slow clock; the MAC records that none was
        // supplied.
        MacSlowClockCalibration::Unavailable
    }
}

impl WifiMacPlatform for Platform {
    fn install_phy_tx_power_profile(&mut self, profile: PhyTxTargetPowerProfile) {
        self.tx_power = Some(profile);
    }
}

fn park() -> ! {
    loop {
        // SAFETY: `wfi` only waits until the next interrupt.
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}
