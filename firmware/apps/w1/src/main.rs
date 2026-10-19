#![no_main]
#![no_std]
#![forbid(unsafe_code)]

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

use core::{convert::Infallible, fmt::Write as _};

use embassy_executor::SendSpawner;
use embassy_futures::select::{Either, select};
use esp_hal::{
    Blocking, efuse, interrupt::software::SoftwareInterrupt, rng::RngOwner,
    usb::usb_serial_jtag::UsbSerialJtag,
};
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_board as board;
use oer_esp32s31_hal::root::ProtocolPartitions;
use oer_esp32s31_ieee80211_driver::{
    runtime::{WifiRoleMaterialized, materialize_esp32s31_wifi_role},
    rx::{counters::WifiRxCounters, returns::HeldRxFrame, upper::FrameReceiver},
    startup::{
        RadioStartConfig, RadioStartFailure, WifiColdStartConfig, WifiMacPlatform,
        WifiMacStartConfig, start_esp32s31_radio,
    },
};
use oer_esp32s31_ieee80211_mac::{
    init::{
        MAC_COLD_HANDSHAKE_DEADLINE, MAC_COLD_RX_INTERRUPT_MASK, MacDelayEntropy,
        MacSlowClockCalibration, MacSlowClockCalibrationSource,
    },
    portable::rx_meta,
};
use oer_esp32s31_phy::{
    NoopPhyTargetObserver, PhyCalibrationIdentity, PhyConfig, PhyRegisterConfig,
    PhyTxTargetPowerProfile, concurrent::ConcurrentPhy, phy_get_rf_cal_version,
};
use oer_esp32s31_radio_client::{LpClock, PhyPreparation, RcSlow, claim};
use oer_esp32s31_radio_system::{Esp32s31Radio, RadioSystem};
use oer_espressif_esp_hal_radio::{PlatformClocks, RadioSingletons, take_radio};
use oer_espressif_executor::{Executor, InterruptExecutor, LevelOrder, timer_observation::Window};
use oer_espressif_staged_runtime::{panic, stacks};
use oer_ieee80211_mac::{
    address::{Bssid, IndividualAddress},
    channel::{Channel, WifiChannel, WifiChannelWidth},
};
use oer_ieee80211_runtime::survey::Survey;
use oer_ieee80211_softmac::WifiMacAddress;
use oer_memory::{AffineSpscQueue, AffineSpscReceiver};
use oer_radio_regulatory::{Country, RegulatoryDomain};
use oer_time::{Clock, Duration, Instant};
use static_cell::StaticCell;

use crate::{
    irq::{BUFFER, Bound, ImageDoorbell, ImageRoutes, P1Ends, SLOTS},
    measure::{Phase, TickStamped, TimedEspHal, TimerLine},
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
    /// The application executor's wake (thread mode).
    wake: WakeToken = FROM_CPU_INTR0 => oer_espressif_executor::wake_handler::<0>, Priority1, ProCpu;
    /// The radio executor (P2), polled in its handler, which times each run.
    radio: RadioToken = FROM_CPU_INTR2 => crate::irq::radio_executor, Priority2, ProCpu;
    /// The radio doorbell: P2's requests to the interrupt line.
    doorbell: DoorbellToken = FROM_CPU_INTR3 => crate::irq::doorbell, Priority3, ProCpu;
    /// The Wi-Fi MAC interrupt.
    wifi_mac: WifiMacToken = MODEM_WIFI_MAC => crate::irq::wifi_mac, Priority3, ProCpu;
    /// The Wi-Fi WDEVPWR interrupt.
    wifi_pwr: WifiPwrToken = MODEM_WIFI_PWR => crate::irq::wifi_pwr, Priority3, ProCpu;
    /// The radio executor's timer alarm; it only acknowledges and pends.
    radio_alarm: RadioAlarmToken = SYSTIMER_TARGET0 => oer_espressif_executor::alarm_interrupt::<0>, Priority3, ProCpu;
    /// The application executor's timer alarm.
    app_alarm: AppAlarmToken = SYSTIMER_TARGET1 => oer_espressif_executor::alarm_interrupt::<1>, Priority1, ProCpu;
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
    rng: RngOwner<'static>,
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

oer_espressif_staged_runtime::entry!(main);

fn main(handoff: oer_espressif_staged_runtime::Handoff) -> ! {
    // Before anything can panic and overwrite it.
    let previous = panic::take_previous();
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let (psram, adopted) = handoff.adopt(INTERRUPT_TABLE, board::BootstrapPsram(peripherals.PSRAM));
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
    oer_espressif_executor::init::<2, _>(peripherals.SYSTIMER, interrupts.radio_alarm);
    oer_espressif_executor::bind::<0, _>(interrupts.app_alarm);
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
    adopted.enable_interrupts();
    let boot = Boot {
        console,
        cell: cells.wifi,
        wifi,
        rng: RngOwner::new(peripherals.RNG),
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
        rng: boot.rng,
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
    rng: RngOwner<'static>,
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
) {
    let fill = Duration::from_millis(fill_ms);
    cycle.fill_ms = fill_ms;
    let mut leases = heapless::Vec::<(Held, Fingerprint), SLOTS>::new();
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
    for (_, print) in &leases {
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
    for (held, print) in &leases {
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
    let mut slots: [Option<(Held, Fingerprint)>; SLOTS] = [const { None }; SLOTS];
    for (slot, lease) in slots.iter_mut().zip(leases) {
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
        WifiMacStartConfig::new(MAC_COLD_HANDSHAKE_DEADLINE, station, access_point),
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
        measure::wait_until(measure::P1_COMPARATOR, after(BIND_POLL)).await;
    };
    let P1Ends {
        mut stop,
        mut receiver,
        counters,
        ring_base,
        storage,
    } = match bound {
        Bound::Ends(ends) => ends,
        Bound::Refused(error) => {
            let _ = write!(console, "OER w1 bind refused {error:?}\r\n");
            return Err("bind");
        }
        Bound::Claimed => {
            let _ = write!(console, "OER w1 bind refused: the arena was claimed\r\n");
            return Err("bind");
        }
    };

    let survey = SURVEY.init(Survey::new(stand));
    let mut lags = report::LagLog::new();
    let mut tsf = report::TsfLog::new();
    let mut admitted = 0_u32;
    let mut window_us = 0;
    let mut cycles: [Cycle; CYCLES] = core::array::from_fn(|cycle| Cycle::new(Order::of(cycle)));
    let mut timers = heapless::Vec::<TimerLine, 4>::new();
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
            Some((&mut *survey, &mut lags, &mut tsf, stand)),
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
                Some((&mut *survey, &mut lags, &mut tsf, stand)),
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
        for cycle in &mut cycles {
            hold_cycle(
                &mut receiver,
                counters,
                ring_base,
                storage,
                rx_channel,
                &mut admitted,
                fill_ms,
                cycle,
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
    if exit.cause != oer_esp32s31_ieee80211_driver::rx::control::WifiRxCause::StopRequested
        || exit.end != oer_esp32s31_ieee80211_driver::rx::control::WifiRxEnd::Halted
    {
        let _ = write!(console, "OER w1 exit {exit:?}\r\n");
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
        cycles: &cycles,
        lags: &lags,
        tsf: &tsf,
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
struct Platform {
    rng: RngOwner<'static>,
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

fn park() -> ! {
    oer_espressif_staged_runtime::idle()
}
