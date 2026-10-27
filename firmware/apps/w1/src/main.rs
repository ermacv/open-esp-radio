#![no_main]
#![no_std]
#![forbid(unsafe_code)]
#![feature(type_alias_impl_trait)]

//! W1: a passive scan on the product receive path. The radio system brings
//! Wi-Fi up on the configured channel (as W0); the receive runner binds its
//! arena on the radio executor (P2), receives with the MAC and WDEVPWR
//! interrupts at P3 and hands staged frames to the application (P1), which
//! feeds them to the survey for a 10 s window from the first frame. A timer
//! stress phase, two hold cycles of every staging lease (the walker
//! exhausts; a staged release restarts it while the ownership experiment
//! rechecks the held buffers, [`own`]) and the stop follow; after the exit
//! the image prints its report block and verdict ([`report`]).
//!
//! Nothing is transmitted. The stand's identity comes from the build
//! (`OER_W1_CHANNEL`, `OER_W1_BSSID`, `OER_W1_SSID_HEX`): the BSSID of the
//! stand router's AP interface, which identifies the stand, and the SSID its
//! beacons must carry; without them the image prints
//! `OER-RESULT: fail w1 unconfigured`. No SSID, unmasked BSSID or base MAC is
//! printed.

// The image declares the profile class in its manifest; `cargo fw` sets the cfg.
#[cfg(not(oer_profile))]
compile_error!(
    "w1 declares the profile class: build it with `cargo fw build w1`, check it with RUSTFLAGS=\"--cfg oer_profile\""
);

mod irq;
mod measure;
mod own;
mod report;
mod usb;

use core::convert::Infallible;

use embassy_futures::select::{Either, select};
use oer_diagnostics::{DiagPath, DiagnosticPath, Note};
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_board as board;
use oer_esp32s31_hal::boot::CpuProfile;
use oer_esp32s31_hal::platform::{
    FactoryIdentity, FromCpu0Irq, FromCpu2Irq, FromCpu3Irq, Interface, SystimerAlarm0,
    SystimerAlarm1,
};
use oer_esp32s31_hal::root::{ProtocolPartitions, Root};
use oer_esp32s31_hal::types::MacInterruptEnableState;
use oer_esp32s31_hal::usb_serial_jtag::ConsoleInterrupt;
use oer_esp32s31_hal::watchdog::{ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters};
use oer_esp32s31_ieee80211_driver::{
    runtime::{WifiRoleMaterialized, materialize_esp32s31_wifi_role},
    rx::{counters::WifiRxCounters, returns::HeldRxFrame, upper::FrameReceiver},
    startup::{
        RadioStartConfig, RadioStartFailure, WifiColdStartConfig, WifiMacPlatform,
        WifiMacStartConfig, start_esp32s31_radio,
    },
};
use oer_esp32s31_phy::{
    NoopPhyTargetObserver, PhyCalibrationIdentity, PhyConfig, PhyRegisterConfig,
    PhyTxTargetPowerProfile, concurrent::ConcurrentPhy, phy_get_rf_cal_version,
};
use oer_esp32s31_radio_client::{Failure, LpClock, PhyPreparation, RcSlow, claim};
use oer_esp32s31_radio_system::{Esp32s31Radio, RadioSystem};
use oer_espressif_executor::{
    Doorbell, Executor, InterruptExecutor, Spawner, timer_observation::Window,
};
use oer_espressif_staged_runtime::{panic, stacks};
use oer_ieee80211_runtime::survey::Survey;
use oer_ieee80211_wire::{
    address::{Bssid, IndividualAddress},
    channel::{Channel, WifiChannel, WifiChannelWidth},
};
use oer_memory::{AffineSpscQueue, AffineSpscReceiver};
use oer_radio_regulatory::{Country, RegulatoryDomain};
use oer_time::{Clock, Duration, Instant};
use static_cell::StaticCell;
use {
    oer_esp32s31_hal::ieee80211::init::{
        MAC_COLD_HANDSHAKE_DEADLINE, MAC_STATION_INTERRUPT_MASK, MacDelayEntropy,
        MacSlowClockCalibration, MacSlowClockCalibrationSource,
    },
    oer_esp32s31_ieee80211_mac::portable::rx_meta,
};

use crate::{
    irq::{BUFFER, Bound, ImageDoorbell, Lower, P1Ends, SLOTS, WifiPart},
    measure::{Phase, TickStamped, TimedChipClocks, TimerLine},
    own::{CYCLES, Check, Cycle, Fingerprint, Order},
};

/// The stand W1 checks.
#[derive(Clone, Copy)]
pub struct W1Config {
    /// The 2.4 GHz channel, 1 to 13.
    pub channel: u8,
    /// The stand AP's BSSID: unicast, nonzero.
    pub stand: [u8; 6],
    /// The stand's SSID, 1 to 32 bytes, which the stand's beacons carry.
    pub ssid: &'static [u8],
    /// The stand router radio's regulatory country.
    pub country: [u8; 2],
}

include!(concat!(env!("OUT_DIR"), "/w1_config.rs"));

oer_espressif_staged_runtime::interrupt_table! {
    contexts { app_wake: 1; radio_executor: 2; radio: 3; }
    /// The application executor's wake (thread mode).
    wake: WakeToken = FromCpu0Irq => oer_espressif_executor::wake_handler::<0, FromCpu0Irq>, line 16, context app_wake, hart 0, Hardware;
    /// The radio executor (P2), polled in its handler, which times each run.
    radio: RadioToken = FromCpu2Irq => crate::irq::radio_executor, line 17, context radio_executor, hart 0, Executor;
    /// The radio interrupt: the Wi-Fi MAC and WDEVPWR banks and the radio
    /// doorbell, P2's requests to the line.
    wifi: WifiToken = WifiPart => crate::irq::on_wifi, line 18, context radio, hart 0, Hardware;
    /// The radio executor's timer alarm; it only acknowledges and pends.
    radio_alarm: RadioAlarmToken = SystimerAlarm0 => oer_espressif_executor::alarm_interrupt::<0, SystimerAlarm0>, line 19, context radio, hart 0, Hardware;
    /// The application executor's timer alarm.
    app_alarm: AppAlarmToken = SystimerAlarm1 => oer_espressif_executor::alarm_interrupt::<1, SystimerAlarm1>, line 20, context app_wake, hart 0, Hardware;
    /// The console's USB Serial/JTAG interrupt; it raises the endpoint's signal.
    console_usb: ConsoleUsb = ConsoleInterrupt => crate::usb::interrupt, line 21, context app_wake, hart 0, Hardware;
}

/// The CPU clock the bootstrap selects (`firmware/bootstrap/esp32s31`).
const CPU_HZ: u32 = CpuProfile::Mhz160.clock().rates.cpu_hz;

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
/// The shortest window a hold has to collect every staging lease.
const HOLD_FILL_MIN_MS: u32 = 3_000;
/// The longest such window.
const HOLD_FILL_MAX_MS: u32 = 10_000;
/// The window when the survey has no beacon interval of the stand.
const HOLD_FILL_WITHOUT_STAND_MS: u32 = 6_000;
/// Beacon intervals past the staging slots a fill waits for.
const HOLD_FILL_SPARE_BEACONS: u32 = 4;
/// How long the full hold lasts.
const HOLD: Duration = Duration::from_millis(1_000);
/// How long the application takes no frame after the first drop.
const DWELL: Duration = Duration::from_millis(1_500);
/// The wait after each later drop.
const RELEASE_STEP: Duration = Duration::from_millis(10);
/// How long frames are counted after the release.
const AFTER_RELEASE: Duration = Duration::from_millis(2_000);

use usb::Console;
type Held = HeldRxFrame<'static, 'static, SLOTS, BUFFER, crate::irq::RadioExecutor>;
type Leases = heapless::Vec<(Held, Fingerprint), SLOTS>;
type ReleaseSlots = [Option<(Held, Fingerprint)>; SLOTS];
type Receiver = FrameReceiver<'static, SLOTS, BUFFER, crate::irq::RadioExecutor>;

oer_espressif_executor::tasks! {
    /// The radio executor's tasks (P2).
    RadioTasks: InterruptExecutor<2> {
        radio_actor(actor: &'static mut Actor) = radio_actor;
        p2_timers(lines: oer_memory::AffineSpscSender<'static, TimerLine, 2>) = measure::p2_timers;
        rx_task(
            registers: oer_esp32s31_hal::owner::RadioAccess,
            bound_part: oer_interrupt_table::Bound<WifiToken>,
            doorbell: ImageDoorbell,
            bound: oer_memory::AffineSpscSender<'static, Bound, 1>,
        ) = irq::rx_task;
    }
}

oer_espressif_executor::tasks! {
    /// The application executor's tasks.
    AppTasks: Executor<0> {
        w1(boot: Boot) = w1;
        rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_rtc_watchdog;
    }
}
static BOUND: static_cell::ConstStaticCell<AffineSpscQueue<Bound, 1>> =
    static_cell::ConstStaticCell::new(AffineSpscQueue::new());
static TIMER_LINES: static_cell::ConstStaticCell<AffineSpscQueue<TimerLine, 2>> =
    static_cell::ConstStaticCell::new(AffineSpscQueue::new());
static SURVEY: StaticCell<Survey<{ report::BSS }>> = StaticCell::new();
static LAGS: StaticCell<report::LagLog> = StaticCell::new();
static TSF: StaticCell<report::TsfLog> = StaticCell::new();
static HOLD_CYCLES: StaticCell<[Cycle; CYCLES]> = StaticCell::new();
static TIMERS: StaticCell<heapless::Vec<TimerLine, 4>> = StaticCell::new();
static HOLD_LEASES: StaticCell<Leases> = StaticCell::new();
static RELEASE_SLOTS: StaticCell<ReleaseSlots> = StaticCell::new();

static ACTOR: StaticCell<Actor> = StaticCell::new();

type Actor = RadioSystem<
    oer_esp32s31_radio_client::wifi::Wifi,
    RcSlow,
    TickStamped<
        Esp32s31Radio<
            TimedChipClocks,
            oer_espressif_executor::Cooperative<EmbassyClock>,
            NoopPhyTargetObserver,
        >,
    >,
    EmbassyClock,
>;

/// What the application task needs from the boot.
struct Boot {
    console: Console,
    /// The console's pads, held so that nothing switches them off.
    _pads: oer_esp32s31_hal::usb_serial_jtag::UsbPads,
    cell: oer_esp32s31_radio_client::MembershipCell<oer_esp32s31_radio_client::wifi::Wifi>,
    wifi: oer_esp32s31_hal::root::WifiPartition,
    rng: Rng,
    station: [u8; 6],
    access_point: [u8; 6],
    radio: Spawner<RadioTasks>,
    wifi_bound: oer_interrupt_table::Bound<WifiToken>,
    doorbell: ImageDoorbell,
    bound: (
        oer_memory::AffineSpscSender<'static, Bound, 1>,
        AffineSpscReceiver<'static, Bound, 1>,
    ),
    timer_lines: AffineSpscReceiver<'static, TimerLine, 2>,
}

/// The image's entropy owner.
pub(crate) type Rng = oer_esp32s31_hal::platform::Entropy<EmbassyClock>;

oer_esp32s31_hal::phy_fault_reporter!(oer_espressif_staged_runtime::phy_fault::reset);
oer_espressif_staged_runtime::entry!(main);

fn main(handoff: oer_espressif_staged_runtime::Handoff) -> ! {
    // Before anything can panic and overwrite it.
    let previous = panic::take_previous();
    let Some(root) = Root::take() else {
        fail("root-taken")
    };
    let runtime = root
        .into_runtime()
        .unwrap_or_else(|error| fail(error.name()));
    oer_espressif_staged_runtime::wifi_fault::report_previous();
    let (_psram, mut adopted) = match handoff.adopt(
        INTERRUPT_TABLE,
        runtime.interrupts.controller,
        board::BootstrapPsram,
    ) {
        Ok(adopted) => adopted,
        Err(error) => fail(error.name()),
    };
    if let Some(record) = previous {
        usb::early_previous_panic(&record);
    }
    let Some(interrupts) = Interrupts::take() else {
        fail("interrupt-table")
    };

    let factory = FactoryIdentity::read(runtime.efuse);
    let identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        base_mac_address: factory.base_mac(),
        mac_extension: factory.mac_extension(),
    };
    let station = factory.address(Interface::Station);
    let access_point = factory.address(Interface::AccessPoint);

    // The image times the root's platform clock owner.
    let (domain, partitions) = runtime.radio.split::<TimedChipClocks, _>(
        ConcurrentPhy::new(),
        oer_espressif_staged_runtime::phy_fault::identity(),
    );
    let (Some((end, cells)), Some(lp)) = (claim(), LpClock::<RcSlow>::claim()) else {
        fail("claim")
    };
    let ProtocolPartitions {
        wifi, wifi_coex, ..
    } = partitions;
    let backend = TickStamped(Esp32s31Radio::new(
        domain,
        wifi_coex,
        TimedChipClocks::new(runtime.clocks),
        oer_espressif_executor::Cooperative::new(EmbassyClock),
        NoopPhyTargetObserver,
        PhyRegisterConfig::new(identity),
    ));
    let actor = ACTOR.init_with(|| RadioSystem::new(end, backend, EmbassyClock, lp));

    // The radio executor writes the system timer; the application executor
    // publishes its deadlines to it.
    let timer = runtime.system_timer;
    oer_espressif_executor::init::<2, _, _>(
        oer_espressif_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        adopted.router(),
        interrupts.radio_alarm,
        timer.alarm0,
    );
    oer_espressif_executor::bind::<0, _, _>(adopted.router(), interrupts.app_alarm, timer.alarm1);
    // The radio handler's part holds both banks and the doorbell's lowering
    // half from the start; the doorbell rings once the part is bound.
    let software = runtime.software_interrupts;
    let (bell, acknowledger) = oer_espressif_executor::DoorbellLine::<3>::split(software.line3);
    let Ok(wifi_bound) = adopted.router().bind(
        interrupts.wifi,
        WifiPart::new(runtime.wifi_interrupts, software.irq3, Lower(acknowledger)),
    ) else {
        fail("wifi-interrupt")
    };
    let doorbell = ImageDoorbell(Doorbell::new::<FromCpu3Irq>(bell, &wifi_bound));
    let (timer_sender, timer_lines) = TIMER_LINES.take().split();
    let radio = InterruptExecutor::<2>::new(software.line2).start(
        adopted.router(),
        interrupts.radio,
        software.irq2,
    );
    if RadioTasks::radio_actor(&radio, actor).is_err()
        || RadioTasks::p2_timers(&radio, timer_sender).is_err()
    {
        fail("radio-tasks")
    }
    let (console, pads) = match usb::open(adopted.router(), interrupts.console_usb) {
        Ok(console) => console,
        Err(error) => fail(error.reason()),
    };
    let executor = Executor::<0>::new(software.line0);
    let mut router = match adopted.enable_interrupts() {
        Ok(router) => router,
        Err(error) => fail(error.name()),
    };
    let rng = Rng::new(runtime.entropy, EmbassyClock);
    let rtc_watchdog = runtime.watchdogs.rtc;
    executor.run(
        &mut router,
        interrupts.wake,
        software.irq0,
        move |spawner| {
            let boot = Boot {
                console,
                _pads: pads,
                cell: cells.wifi,
                wifi,
                rng,
                station,
                access_point,
                radio,
                wifi_bound,
                doorbell,
                bound: BOUND.take().split(),
                timer_lines,
            };
            if AppTasks::w1(spawner, boot).is_err()
                || AppTasks::rtc_watchdog(spawner, rtc_watchdog).is_err()
            {
                fail("app-tasks")
            }
        },
    )
}

/// Feed the RTC watchdog for good: the image resets when this executor
/// stops polling.
async fn feed_rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) {
    let period = Duration::from_millis(RTC_WATCHDOG_FEED_PERIOD_MS);
    match watchdog.feed_every(&EmbassyClock, period).await {}
}

/// Halt the image on a failed boot step, its reason on the console while
/// no one holds the block.
fn fail(reason: &'static str) -> ! {
    oer_espressif_staged_runtime::fatal_halt(|_path| {
        let _note = oer_esp32s31_hal::usb_serial_jtag::fatal_note(&Note::w1_fail(reason));
    })
}

/// The radio system's actor, on the radio executor (P2), each poll timed.
async fn radio_actor(actor: &'static mut Actor) {
    match measure::timed(core::pin::pin!(actor.run()), &measure::ACTOR, || {}).await {}
}

/// The run, on the application executor.
async fn w1(boot: Boot) {
    let mut console = boot.console;
    let Some(config) = CONFIG else {
        usb::text(&mut console, b"OER-RESULT: fail w1 unconfigured\r\n").await;
        return;
    };
    // The build accepts only a unicast, nonzero BSSID.
    let Some(stand) = IndividualAddress::new(config.stand).and_then(Bssid::new) else {
        usb::text(&mut console, b"OER-RESULT: fail w1 stand\r\n").await;
        return;
    };
    let parts = Parts {
        cell: boot.cell,
        wifi: boot.wifi,
        rng: boot.rng,
        station: boot.station,
        access_point: boot.access_point,
        radio: boot.radio,
        wifi_bound: boot.wifi_bound,
        doorbell: boot.doorbell,
        bound: boot.bound,
        timer_lines: boot.timer_lines,
    };
    if let Err(reason) = run(&mut console, config, stand, parts).await {
        note(&mut console, &Note::w1_fail(reason)).await;
    }
}

/// The boot's owners the run takes.
struct Parts {
    cell: oer_esp32s31_radio_client::MembershipCell<oer_esp32s31_radio_client::wifi::Wifi>,
    wifi: oer_esp32s31_hal::root::WifiPartition,
    rng: Rng,
    station: [u8; 6],
    access_point: [u8; 6],
    radio: Spawner<RadioTasks>,
    wifi_bound: oer_interrupt_table::Bound<WifiToken>,
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
    survey: Option<(
        &mut Survey<{ report::BSS }>,
        &mut report::LagLog,
        &mut report::TsfLog,
        Bssid,
    )>,
    channel: Channel,
    admitted: &mut u32,
) {
    let Ok(frame) = receiver.normalize(held) else {
        return;
    };
    *admitted = admitted.saturating_add(1);
    // The runner's publisher marks every handoff.
    if let (Some((survey, lags, tsf, stand)), Some(times)) = (survey, held.times()) {
        if let Some(stamp) = frame.stamp {
            lags.take(stamp, times.handoff);
        }
        let meta = rx_meta(frame.metadata, channel);
        let before = tsf.before(survey);
        survey.take(
            EmbassyClock.now(),
            frame.mpdu,
            &meta,
            frame.stamp,
            times.handoff,
        );
        tsf.after(survey, stand, before, times.handoff);
    }
}

/// Take and drop frames for ever.
async fn drain(receiver: &mut Receiver, channel: Channel, admitted: &mut u32) -> Infallible {
    loop {
        let held = receiver.next().await;
        take(receiver, &held, None, channel, admitted);
    }
}

/// How long a hold may take to collect every staging lease, in ms:
/// `(SLOTS + 4)` beacon intervals of the stand (`bi_tu`, 1024 µs each, from
/// the survey), at least [`HOLD_FILL_MIN_MS`] and at most
/// [`HOLD_FILL_MAX_MS`]; [`HOLD_FILL_WITHOUT_STAND_MS`] without the stand or
/// at a zero interval. On a quiet channel the stand's beacons are most of
/// the frames, so a fixed 3 s fill could end before the pool was full.
fn hold_fill_ms(bi_tu: Option<u16>) -> u32 {
    match bi_tu {
        Some(bi_tu) if bi_tu != 0 => {
            let slots = u32::try_from(SLOTS).unwrap_or(u32::MAX);
            let fill_us = slots
                .saturating_add(HOLD_FILL_SPARE_BEACONS)
                .saturating_mul(u32::from(bi_tu))
                .saturating_mul(1_024);
            (fill_us / 1_000).clamp(HOLD_FILL_MIN_MS, HOLD_FILL_MAX_MS)
        }
        _ => HOLD_FILL_WITHOUT_STAND_MS,
    }
}

/// One hold cycle into `cycle`: take every staging lease (at most the fill
/// window, [`hold_fill_ms`]; one short, until that window passed without a
/// unit since the last lease, which may leave the hold stuck at LAST, see
/// [`own::fullness`]) and fingerprint each, hold them [`HOLD`] and recheck
/// them, drop the first in `cycle.order`, dwell [`DWELL`] without taking a
/// frame and recheck the rest, drop them one at a time [`RELEASE_STEP`]
/// apart, each rechecked just before its drop, and take frames for
/// [`AFTER_RELEASE`].
async fn hold_cycle(
    receiver: &mut Receiver,
    counters: &'static WifiRxCounters,
    ring_base: u32,
    storage: &'static irq::Storage,
    channel: Channel,
    admitted: &mut u32,
    fill_ms: u32,
    cycle: &mut Cycle,
    leases: &mut Leases,
    slots: &mut ReleaseSlots,
) {
    let fill = Duration::from_millis(fill_ms);
    cycle.fill_ms = fill_ms;
    let fill_deadline = after(fill);
    // When the last lease was taken, and the units received then.
    let mut last_lease = (EmbassyClock.now(), counters.report().units);
    while !leases.is_full() {
        // One short: the fill waits until no unit came for the fill
        // timeout, so a stuck hold is told from a slow one.
        let deadline = if leases.len() + 1 == SLOTS {
            fill_deadline.max(last_lease.0.saturating_add(fill))
        } else {
            fill_deadline
        };
        match select(
            receiver.next(),
            measure::wait_until(measure::P1_COMPARATOR, deadline),
        )
        .await
        {
            Either::First(held) => {
                take(receiver, &held, None, channel, admitted);
                let segment = held.segment();
                let print = Fingerprint::of(segment.buffer, segment.descriptor_address, ring_base);
                let _full = leases.push((held, print));
                last_lease = (EmbassyClock.now(), counters.report().units);
            }
            Either::Second(()) => break,
        }
    }
    cycle.held = leases.len();
    let mut indices = heapless::Vec::<Option<usize>, SLOTS>::new();
    for (_, print) in leases.iter() {
        let _full = indices.push(print.index);
    }
    let filled = counters.report();
    cycle.end = filled.drain_end;
    let quiet = filled.units == last_lease.1
        && EmbassyClock.now().saturating_duration_since(last_lease.0) >= fill;
    cycle.full = own::fullness(&indices, cycle.end.as_ref(), quiet, ring_base);
    let before = counters.report();
    measure::wait_until(measure::P1_COMPARATOR, after(HOLD)).await;
    cycle.hold = report::Hold::between(leases.len(), &before, &counters.report());
    for (held, print) in leases.iter() {
        if print.changed(held.segment().buffer) {
            cycle.mismatch(Check::Hold, print.index);
        }
        // A held lease's descriptor carries the owner bit clear.
        if own::owner_clear(storage.descriptors(), print.index) {
            cycle.owner_clear = cycle.owner_clear.saturating_add(1);
        }
    }

    // The release order: the ring's when the leases cover it (one short
    // of it, for a stuck hold), otherwise the order they were taken in
    // (forward) or its reverse.
    let ring = own::release_order(&indices, cycle.order);
    let mut positions = heapless::Vec::<usize, SLOTS>::new();
    for step in 0..leases.len() {
        let position = match &ring {
            Some(ring) => ring.get(step).copied().unwrap_or(step),
            None if cycle.order == Order::Forward => step,
            None => leases.len() - 1 - step,
        };
        let _full = positions.push(position);
    }
    cycle.first_drop = ring
        .and_then(|ring| ring.first().copied())
        .and_then(|position| indices.get(position).copied().flatten());
    for (slot, lease) in slots.iter_mut().zip(leases.drain(..)) {
        *slot = Some(lease);
    }
    let released = counters.report();
    let mut steps = positions.iter();
    // The first drop, then the dwell: no frame is taken, so the queue
    // keeps what the walker stages meanwhile.
    if let Some((held, print)) = steps
        .next()
        .and_then(|&position| slots.get_mut(position))
        .and_then(Option::take)
    {
        if print.changed(held.segment().buffer) {
            cycle.mismatch(Check::Release, print.index);
        }
        drop(held);
    }
    measure::wait_until(measure::P1_COMPARATOR, after(DWELL)).await;
    let dwelt = counters.report();
    cycle.snapshots = dwelt.exhaustion.snapshots;
    if dwelt.exhaustion.snapshots != released.exhaustion.snapshots {
        cycle.exhaust = dwelt.exhaustion.latest;
    }
    for (held, print) in slots.iter().flatten() {
        if print.changed(held.segment().buffer) {
            cycle.mismatch(Check::Dwell, print.index);
        }
    }
    for &position in steps {
        let Some((held, print)) = slots.get_mut(position).and_then(Option::take) else {
            continue;
        };
        if print.changed(held.segment().buffer) {
            cycle.mismatch(Check::Release, print.index);
        }
        drop(held);
        measure::wait_until(measure::P1_COMPARATOR, after(RELEASE_STEP)).await;
    }

    let deadline = after(AFTER_RELEASE);
    while let Either::First(held) = select(
        receiver.next(),
        measure::wait_until(measure::P1_COMPARATOR, deadline),
    )
    .await
    {
        take(receiver, &held, None, channel, admitted);
        cycle.frames_after_release = cycle.frames_after_release.saturating_add(1);
        let _full = cycle.first_after_release.push(own::descriptor_index(
            held.segment().descriptor_address,
            ring_base,
        ));
    }
    cycle.order_breaks = own::order_breaks(&cycle.first_after_release);
    let end = counters.report();
    cycle.chained = end.chained_drops.wrapping_sub(released.chained_drops);
    cycle.oversized = end.oversized_drops.wrapping_sub(released.oversized_drops);
    let (from, to) = (own::discards(&released), own::discards(&end));
    for ((delta, to), from) in cycle.discards.iter_mut().zip(to).zip(from) {
        *delta = to.wrapping_sub(from);
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
        rng,
        station,
        access_point,
        radio,
        wifi_bound,
        doorbell,
        bound: (bound_sender, bound),
        timer_lines,
    } = parts;
    let (Ok(station), Ok(access_point)) = (
        IndividualAddress::for_interface(station),
        IndividualAddress::for_interface(access_point),
    ) else {
        return Err("efuse-mac");
    };
    let channel =
        WifiChannel::new_2_4_ghz(config.channel, WifiChannelWidth::Mhz20).map_err(|_| "channel")?;
    let domain = Country::new(config.country)
        .ok()
        .and_then(RegulatoryDomain::of)
        .ok_or("domain")?;
    let channel = domain
        .permit(channel)
        .map_err(|_| "channel-not-permitted")?;
    let ceiling = channel.tx_ceiling(board::wifi_power_limits(
        i32::from(PhyConfig::production().highest_target_power_quarter_dbm()) * 25,
        None,
    ));
    let start = RadioStartConfig::new(
        WifiColdStartConfig::new(channel, ceiling),
        WifiMacStartConfig::new(MAC_COLD_HANDSHAKE_DEADLINE, station, access_point)
            .map_err(|_| "mac-address")?,
    );
    let (_cell, started) = start_esp32s31_radio(
        cell,
        wifi,
        Platform {
            rng,
            tx_power: None,
        },
        start,
    )
    .await;
    let stopped = match started {
        Ok(stopped) => stopped,
        Err(RadioStartFailure::Join { failed, .. }) => {
            note(console, &Note::w1_join_failed(failure_name(failed.cause))).await;
            return Err("join");
        }
        Err(RadioStartFailure::Track { failed, .. }) => {
            note(console, &Note::w1_track_failed(failure_name(failed.cause))).await;
            return Err("track");
        }
    };
    // The owner and the cell stay held until the image parks.
    let WifiRoleMaterialized {
        owner,
        registers,
        resources: (),
    } = materialize_esp32s31_wifi_role(stopped, ());
    let start_report = owner.start_report();
    let applied = owner.current_channel();
    let rx_channel = Channel::from(applied);
    // A second start is refused: the receive runner binds its arena once.
    if RadioTasks::rx_task(&radio, registers, wifi_bound, doorbell, bound_sender).is_err() {
        return Err("rx-spawn");
    }
    let bind_deadline = after(BIND);
    let bound = loop {
        if let Ok(bound) = bound.try_receive() {
            break bound;
        }
        if EmbassyClock.now() >= bind_deadline {
            return Err("no-bind");
        }
        measure::wait_until(measure::P1_COMPARATOR, after(BIND_POLL)).await;
    };
    let P1Ends {
        mut stop,
        mut receiver,
        counters,
        ring_base,
        storage,
        requests,
    } = match bound {
        Bound::Ends(ends) => ends,
        Bound::Refused(error) => {
            note(
                console,
                &Note::w1_bind_refused(match error {
                    oer_esp32s31_hal::ieee80211::dma::rx_storage::RxDmaStorageError::AddressWidth => "AddressWidth",
                    oer_esp32s31_hal::ieee80211::dma::rx_storage::RxDmaStorageError::Count => "Count",
                }),
            ).await;
            return Err("bind");
        }
        Bound::Claimed => {
            note(console, &Note::w1_bind_claimed()).await;
            return Err("bind");
        }
    };

    let survey = SURVEY.init(Survey::new(stand));
    let lags = LAGS.init(report::LagLog::new());
    let tsf = TSF.init(report::TsfLog::new());
    let mut admitted = 0_u32;
    let mut window_us = 0;
    let cycles =
        HOLD_CYCLES.init_with(|| core::array::from_fn(|cycle| Cycle::new(Order::of(cycle))));
    let timers = TIMERS.init(heapless::Vec::new());
    measure::enter(Phase::FirstFrame);
    let first = select(
        receiver.next(),
        measure::wait_until(measure::P1_COMPARATOR, after(FIRST_FRAME)),
    )
    .await;
    let heard = matches!(first, Either::First(_));
    if let Either::First(held) = first {
        // The window: every frame from the first one goes to the survey.
        let window_start = EmbassyClock.now();
        measure::enter(Phase::Window);
        let window = Window::begin();
        take(
            &receiver,
            &held,
            Some((&mut *survey, lags, tsf, stand)),
            rx_channel,
            &mut admitted,
        );
        drop(held);
        let deadline = window_start.saturating_add(WINDOW);
        while let Either::First(held) = select(
            receiver.next(),
            measure::wait_until(measure::P1_COMPARATOR, deadline),
        )
        .await
        {
            take(
                &receiver,
                &held,
                Some((&mut *survey, lags, tsf, stand)),
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

        // The holds: every staging lease held, the walker exhausts; the
        // staged release restarts it. The second cycle starts after the
        // first one's frames after the release: the ring has recovered.
        measure::enter(Phase::Hold);
        let fill_ms = hold_fill_ms(
            survey
                .entries()
                .find(|(record, _)| record.bssid == stand)
                .map(|(record, _)| record.beacon_interval_tu),
        );
        let leases = HOLD_LEASES.init(heapless::Vec::new());
        let slots = RELEASE_SLOTS.init([const { None }; SLOTS]);
        for cycle in cycles.iter_mut() {
            hold_cycle(
                &mut receiver,
                counters,
                ring_base,
                storage,
                rx_channel,
                &mut admitted,
                fill_ms,
                cycle,
                leases,
                slots,
            )
            .await;
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
    let cpu_hz = CPU_HZ;
    // The MAC interrupt enable image the line read after its last requests.
    let mac_enable = requests.mac_interrupt_enable();

    // Outside the block: what its lines do not carry.
    match start_report.phy {
        PhyPreparation::Registered {
            calibration_path,
            micros,
            ..
        } => {
            let mut path = DiagPath::new();
            calibration_path.push_path(&mut path);
            note(
                console,
                &Note::w1_start(
                    start_report.epoch.get(),
                    path.names().first().copied().unwrap_or(""),
                    micros,
                ),
            )
            .await;
        }
        other => {
            let name = match other {
                PhyPreparation::AlreadyOpen => "AlreadyOpen",
                PhyPreparation::Woke { .. } => "Woke",
                PhyPreparation::Registered { .. } => "Registered",
            };
            note(console, &Note::w1_start_phy(start_report.epoch.get(), name)).await;
        }
    }
    note(
        console,
        &Note::w1_start_track(
            start_report
                .track
                .tracking_micros
                .map(|micros| u32::try_from(micros).unwrap_or(u32::MAX)),
            match mac_enable {
                Some(MacInterruptEnableState::Disabled) => "Disabled",
                Some(MacInterruptEnableState::Station) => "Station",
                Some(MacInterruptEnableState::Unknown) => "Unknown",
                None => "Unread",
            },
            MAC_STATION_INTERRUPT_MASK.bits(),
        ),
    )
    .await;
    note(
        console,
        &Note::w1_irq(
            rx.spurious_mac_irqs,
            rx.auxiliary_mac_irqs,
            rx.unhandled_mac_irqs,
            rx.closed_irqs,
        ),
    )
    .await;
    if !heard {
        note(console, &Note::w1_no_frame()).await;
    }
    if exit.cause != oer_esp32s31_ieee80211_driver::rx::control::WifiRxCause::StopRequested
        || exit.end != oer_esp32s31_ieee80211_driver::rx::control::WifiRxEnd::Halted
    {
        note(
            console,
            &Note::w1_exit_unexpected(report::cause(exit.cause), report::end_name(exit.end)),
        )
        .await;
    }
    let run = report::Run {
        channel: config.channel,
        applied_channel: applied.primary(),
        stand,
        ssid: config.ssid,
        window_ms: WINDOW_MS,
        cpu_hz,
        survey: &*survey,
        window_us,
        rx,
        unserviced_mac: irq::UNSERVICED_MAC.load(core::sync::atomic::Ordering::Relaxed),
        unserviced_pwr: irq::UNSERVICED_PWR.load(core::sync::atomic::Ordering::Relaxed),
        cycles,
        lags,
        tsf,
        admitted,
        exit,
        ticks: &ticks,
        timers,
        irq_bytes: stacks::irq_stack_bytes(oer_esp32s31_hal::platform::BOOT_HART),
        irq_free,
    };
    let _ = report::print(
        async |note| oer_espressif_usb_serial_jtag::write_note(console, note).await,
        &run,
    )
    .await;
    // The owners stay held: nothing is released or reset.
    let _owner = owner;
    core::future::pending::<()>().await;
    Ok(())
}

/// Wi-Fi's own platform values for the MAC bring-up.
struct Platform {
    rng: Rng,
    tx_power: Option<PhyTxTargetPowerProfile>,
}

impl MacDelayEntropy for Platform {
    fn mac_delay_random(&mut self) -> u32 {
        self.rng.random()
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

/// Writes `note` to the console as a typed diagnostic line.
async fn note(console: &mut Console, note: &Note<'_>) {
    let _ = oer_espressif_usb_serial_jtag::write_note(console, note).await;
}

fn failure_name(cause: Failure) -> &'static str {
    match cause {
        Failure::Refused => "Refused",
        Failure::ClientFailed => "ClientFailed",
        Failure::Poisoned(_) => "Poisoned",
    }
}
