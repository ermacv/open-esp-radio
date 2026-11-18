//! The probe's boot and its radio session. The session holds the radio
//! domain itself and brings Wi-Fi up by the public operations of the PHY
//! crate and the HAL that `Esp32s31Radio` calls, in the same order
//! (README, "The order"), so that `RadioDomain::reset_wifi_mac_block` can pulse the
//! MAC's block reset with the receive walker live.
#![cfg_attr(
    any(feature = "transmit", feature = "receiver"),
    allow(
        dead_code,
        reason = "part two's image leaves part one's series out; its helpers stay shared"
    )
)]
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
    radio::domain::RadioDomain,
    radio::ieee80211::{
        client::{WifiClocked, WifiCold},
        dma::{descriptor::RxDescriptor, rx_ring::RxRingLive, rx_storage::RxDmaStorage},
        init::{
            MAC_COLD_HANDSHAKE_DEADLINE, MacCoexEvent, MacCoexPti, MacCoexPtiSource,
            MacColdStartConfig, MacDelayEntropy, MacSlowClockCalibration,
            MacSlowClockCalibrationSource, MacTxPowerSource, initialize_wifi_mac,
        },
        registers::{MacCcmpKeyIdentity, MacKeyEntryIndex, MacKeyInstallOutcome},
    },
    system::owner::{MacInterruptRegisters, MacPowerInterruptRegisters, RadioAccess},
    system::root::Root,
    types::MacTxPowerPair,
};
use oer_chips_esp32s31_platform_board as board;
use oer_chips_esp32s31_platform_runtime::Esp32s31;
use oer_chips_esp32s31_radio_client::{LpClock, RcSlow};
use oer_chips_esp32s31_radio_phy::{
    NoopPhyTargetObserver, PhyCalibrationIdentity, PhyConfig, PhyRegisterConfig,
    PhyTxTargetPowerProfile, RomShortDelay,
    concurrent::{ConcurrentAcquire, ConcurrentPhy, PhyClientAccess, phy_ready},
    maintain_concurrent_phy, phy_get_rf_cal_version, register_concurrent_phy,
    select_concurrent_wifi_channel,
    wifi_client::{WifiPhyMembership, join_wifi, leave_wifi, set_wifi_rx},
};
use oer_chips_family_platform_executor::{Executor, InterruptExecutor};
use oer_portable_diagnostics::{Name, name};
use oer_portable_embassy_time::EmbassyClock;
use oer_portable_time::{Clock, Duration, Timer, settle::Settle, settle::SettleLimit};
use oer_protocols_ieee80211_wire::channel::{ChannelWidth, Ghz2_4Channel};
use oer_protocols_radio_regulatory::{Country, RegulatoryDomain};

/// The descriptors of the receive ring and their buffers: a frame of the
/// air takes one or two, and no unit can span the whole ring
/// (`rx_ring_outspans_any_unit`, which the product's geometry asserts too).
const COUNT: usize = 32;
const BUFFER: usize = 1_600;
const _: () = {
    if !oer_chips_esp32s31_hal::radio::ieee80211::dma::rx_ring::rx_ring_outspans_any_unit(
        COUNT, BUFFER,
    ) {
        panic!("one receive unit could span the whole ring");
    }
};
const NODES: usize = COUNT + 1;
const WORDS: usize = BUFFER / 4;
/// The stand's busy channel.
const CHANNEL: u8 = 13;
/// The pulses of one boot (questions 1, 2 and 3).
const PULSES: usize = 100;
/// The ordinary stops of one boot (question 4).
const STOPS: usize = 1_000;
/// The pulses with the walker stopped first (question 1's walker flag).
const QUIET: usize = 10;
/// The pulses left without the cold preparation for [`HOLD_MS`].
const HOLDS: usize = 10;
/// How long the block stays in its reset state under the load.
const HOLD_MS: u32 = 20;
/// How long a run receives before its pulse or its stop, and after a
/// second run's start.
const RECEIVE_MS: u32 = 20;
/// The longest delay before a pulse, in microseconds: a pulse lands at a
/// point spread over a received frame's time (a 1 500-octet frame at
/// 1 Mb/s is 12 ms; at 54 Mb/s 0.25 ms).
const PULSE_SPREAD_US: u32 = 2_000;
/// A stop that has not ended in this time is recorded with its registers.
const STOP_BOUND_MS: u32 = 10;
/// The key entry the probe installs before its pulses.
const KEY_ENTRY: u8 = 0;

/// A reload doorbell's settle, as the backend names it.
///
/// SOURCE: `oer-chips-esp32s31-radio-ieee80211-backend` `rx::receive::RELOAD_SETTLE_LIMIT`
/// (20 µs; the W1 runs of 2026-10-19).
const RELOAD_SETTLE: SettleLimit = match SettleLimit::new(Duration::from_micros(20)) {
    Some(limit) => limit,
    None => panic!("20 µs is a valid settle limit"),
};

type Storage = RxDmaStorage<COUNT, BUFFER, BUFFER>;
type Radio = RadioDomain<ChipClocks, ConcurrentPhy>;
type CooperativeTimer = oer_chips_family_platform_executor::Cooperative<Esp32s31, EmbassyClock>;

oer_chips_esp32s31_hal::dma_static! { static RX: Storage = zeroed in ".dma.bss.mac_reset_probe"; }

use crate::phy_pilot;

// SAFETY: the production runtime script maps .hot.text.* to executable SRAM;
// each generated wrapper retains the interrupt-table ABI and owned part.
oer_chips_family_platform_interrupt_table::interrupt_table! {
    lines: oer_chips_family_platform_runtime::interrupts::LINES,
    handler_attributes: [#[unsafe(link_section = ".hot.text.mac_reset_vectors")]];
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

/// What the session is given at boot.
pub struct SessionParts {
    radio: Radio,
    clocks: ChipClocks,
    wifi: oer_chips_esp32s31_hal::system::root::WifiPartition,
    coex: oer_chips_esp32s31_hal::radio::domain::CoexAbsent<
        oer_chips_esp32s31_hal::radio::domain::WifiClient,
    >,
    pilot: phy_pilot::Client,
    identity: PhyCalibrationIdentity,
    mac_irq: MacInterruptRegisters,
    power_irq: MacPowerInterruptRegisters,
    seed: u32,
    station: [u8; 6],
    ap: [u8; 6],
}

oer_chips_family_platform_executor::tasks! {
    pub RadioTasks: InterruptExecutor<Esp32s31, 2> {
        session(parts: SessionParts, lp: LpClock<RcSlow>) = session_task;
    }
}
oer_chips_family_platform_executor::tasks! {
    AppTasks: Executor<Esp32s31, 0> {
        report(console: usb::Console, pads: oer_chips_esp32s31_hal::peripherals::usb_serial_jtag::UsbPads) = report;
        watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_watchdog;
    }
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
        fail(name!("root"; 32));
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
        _,
    >(
        handoff,
        root.into_runtime(),
        InterruptTable,
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
        fail(name!("interrupts"; 32));
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
    let (radio, partitions) = runtime_radio
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
        fail(name!("claim"; 32));
    };
    let pilot = crate::phy_pilot::init(runtime_timg0_alarm, adopted.router(), interrupts.phy);
    let timer = runtime_system_timer;
    let mut timer_ready = oer_chips_family_platform_executor::init::<Esp32s31, 2, _, _, _, _>(
        oer_chips_family_platform_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        adopted.router(),
        interrupts.radio_alarm,
        timer.alarm0,
    )
    .unwrap_or_else(|failure| failure.reset());
    oer_chips_family_platform_executor::bind::<Esp32s31, 0, _, _, _, _>(
        &mut timer_ready,
        adopted.router(),
        interrupts.app_alarm,
        timer.alarm1,
    )
    .unwrap_or_else(|failure| failure.reset());
    // No Wi-Fi interrupt consumer is installed: the session polls.
    let mut wifi_irqs = runtime_wifi_interrupts;
    wifi_irqs.mac.mask_all_mac_interrupts();
    wifi_irqs.power.mask_all_power_interrupts();
    let software = runtime_software_interrupts;
    let (hart, router) = adopted.parts();
    let radio_executor = InterruptExecutor::<Esp32s31, 2>::new(software.line2)
        .start(hart, router, interrupts.radio, software.irq2, ())
        .unwrap_or_else(|failure| failure.reset());
    let mut rng =
        oer_chips_esp32s31_hal::peripherals::entropy::Entropy::new(runtime_entropy, EmbassyClock);
    let seed = match rng.random() {
        Ok(word) => word,
        Err(_) => fail(name!("entropy"; 32)),
    };
    let parts = SessionParts {
        radio,
        clocks: runtime_clocks,
        wifi: partitions.wifi,
        coex: partitions.wifi_coex,
        pilot,
        identity,
        mac_irq: wifi_irqs.mac,
        power_irq: wifi_irqs.power,
        seed,
        station,
        ap,
    };
    if RadioTasks::session(&radio_executor, parts, lp).is_err() {
        fail(name!("session-task"; 32));
    }
    let (console, pads) = match usb::open(adopted.router(), interrupts.console_usb) {
        Ok(value) => value,
        Err(error) => fail(error.reason()),
    };
    let (mut router, hart) = match adopted.enable_interrupts() {
        Ok(value) => value,
        Err(error) => fail(error.name()),
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
                if AppTasks::report(spawner, console, pads).is_err()
                    || AppTasks::watchdog(spawner, runtime_watchdogs.rtc).is_err()
                {
                    fail(name!("task"; 32));
                }
            },
        )
        .unwrap_or_else(|failure| failure.reset()) {}
}

fn fail<const B: usize>(reason: Name<B>) -> ! {
    oer_chips_family_platform_runtime::fatal::boot_failure::<
        oer_chips_esp32s31_hal::system::platform::Port,
    >(reason.as_str(), || {
        let _ = oer_chips_esp32s31_hal::peripherals::usb_serial_jtag::fatal_note(
            &oer_portable_diagnostics::Note::runtime_fail(reason),
        );
    })
}

async fn feed_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) {
    match watchdog
        .feed_every(
            &EmbassyClock,
            Duration::from_millis(RTC_WATCHDOG_FEED_PERIOD_MS),
        )
        .await {}
}

// ---------------------------------------------------------------- journal

/// What one record says.
#[derive(Clone, Copy)]
#[repr(u32)]
enum Kind {
    /// The bring-up's end: units received in the first run.
    Up = 1,
    /// One ordinary stop.
    Stop = 2,
    /// One pulse, its observations and the second run after it.
    Pulse = 3,
    /// The series' end: cycles and microseconds over it, for the rate.
    End = 4,
    /// A failure: its phase and its index.
    Failed = 5,
    /// One pulse with the walker stopped before it.
    Quiet = 6,
    /// One pulse under load left in the block's reset state.
    Hold = 7,
    /// One recovery case of `tasks/097` and its window.
    Case = 8,
    /// Part two: one frame sent and awaited.
    #[cfg(feature = "transmit")]
    Tx = 9,
    /// Part two: a pulse with a frame of its own on the air, and the
    /// second run's frame.
    #[cfg(feature = "transmit")]
    TxPulse = 10,
    /// Part two: an ordinary stop with a frame on the air.
    #[cfg(feature = "transmit")]
    TxStop = 11,
    /// Part two: every abandoned chain read again at the run's end.
    #[cfg(feature = "transmit")]
    Abandoned = 12,
    /// Part two: the peer's address, learned from its frames.
    #[cfg(feature = "transmit")]
    Peer = 13,
    /// Part two's receiver: what it received from the transmitting probe.
    #[cfg(feature = "receiver")]
    Rx = 14,
    /// Case C's sender: one case's frames and how they ended.
    #[cfg(feature = "amsdu")]
    AmsduSent = 15,
    /// Cell E's sender: one case's protected frames and how they ended.
    #[cfg(feature = "ccmp")]
    CcmpSent = 16,
    /// Cell F's sender: one aggregate size's ends.
    #[cfg(feature = "ampdu")]
    AmpduSent = 17,
    /// Cell F's sender: the Block Ack's bit of each subframe, 32 to a word.
    #[cfg(feature = "ampdu")]
    AmpduAcked = 18,
    /// `tasks/184` A: the foreign words at one point of one repetition.
    #[cfg(feature = "foreign")]
    Foreign = 19,
}

const WIDTH: usize = 22;
#[cfg(not(any(feature = "transmit", feature = "receiver", feature = "foreign")))]
const ROWS: usize = 4 + STOPS + PULSES + QUIET + HOLDS + CASES;
#[cfg(feature = "foreign")]
const ROWS: usize = 2 + 6 * FOREIGN_REPETITIONS;
/// `tasks/184` A: the repetitions of the pulse between two reads.
#[cfg(feature = "foreign")]
const FOREIGN_REPETITIONS: usize = 10;
#[cfg(feature = "receiver")]
const ROWS: usize = 8;
#[cfg(feature = "transmit")]
const ROWS: usize = transmit::ROWS;
/// The recovery cases' records: a baseline and cases 0, 1 and 3.
const CASES: usize = 4;
/// The window each recovery case is measured over.
const CASE_WINDOW_MS: u32 = 1_000;

/// The session's records, filled on hart 0's radio executor and printed by
/// the console's task once the session has ended.
#[derive(bytemuck::Zeroable)]
struct Journal {
    rows: [[u32; WIDTH]; ROWS],
    used: usize,
}

impl Journal {
    fn record(&mut self, kind: Kind, values: &[u32]) {
        if let Some(row) = self.rows.get_mut(self.used) {
            *row = [0; WIDTH];
            if let Some(first) = row.first_mut() {
                *first = kind as u32;
            }
            for (cell, value) in row.iter_mut().skip(1).zip(values) {
                *cell = *value;
            }
            self.used = self.used.saturating_add(1);
        }
    }
}

static JOURNAL: oer_portable_memory::zeroed::ZeroedStatic<Journal> =
    oer_portable_memory::zeroed::ZeroedStatic::new();
static DONE: oer_portable_memory::ClaimCell<&'static mut Journal> =
    oer_portable_memory::ClaimCell::new();
/// The phase the session is in, printed as it changes so that the host
/// can place the peer's lines beside the cases (`MARK <n>`).
static MARK: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

fn mark(phase: u32) {
    MARK.store(phase, core::sync::atomic::Ordering::Relaxed);
}

async fn report(
    mut console: usb::Console,
    _pads: oer_chips_esp32s31_hal::peripherals::usb_serial_jtag::UsbPads,
) {
    use core::fmt::Write as _;
    usb::text(&mut console, b"MACRESET boot v=1\n").await;
    let mut shown = 0_u32;
    let journal = loop {
        let now = MARK.load(core::sync::atomic::Ordering::Relaxed);
        if now != shown {
            shown = now;
            let mut line = heapless::String::<32>::new();
            let _ = writeln!(line, "MARK {now}");
            usb::text(&mut console, line.as_bytes()).await;
        }
        if let Ok(journal) = DONE.take() {
            break journal;
        }
        match EmbassyClock.now().checked_add(Duration::from_millis(10)) {
            Some(at) => EmbassyClock.wait_until(at).await,
            None => return,
        }
    };
    let mut line = heapless::String::<256>::new();
    for row in journal.rows.iter().take(journal.used) {
        line.clear();
        let _ = line.push_str("R");
        for value in row {
            let _ = write!(line, " {value}");
        }
        let _ = line.push('\n');
        usb::text(&mut console, line.as_bytes()).await;
    }
    let failed = journal
        .rows
        .iter()
        .take(journal.used)
        .any(|row| row.first() == Some(&(Kind::Failed as u32)));
    if failed {
        usb::text(&mut console, b"OER-RESULT: fail mac-reset\n").await;
    } else {
        usb::text(&mut console, b"OER-RESULT: pass mac-reset\n").await;
    }
}

// ---------------------------------------------------------------- session

/// The platform values the MAC cold start reads, as the radio client's
/// `MacColdSources` holds them.
///
/// SOURCE: `oer-chips-esp32s31-radio-client` `wifi::MacColdSources` and
/// `MAC_COEX_EVENTS` (the four events in `MacColdCoexPti::query` order).
struct Sources {
    pti: [MacCoexPti; 4],
    tx_power: PhyTxTargetPowerProfile,
    delay_random: u32,
}

impl MacCoexPtiSource for Sources {
    fn mac_coex_pti(&mut self, event: MacCoexEvent) -> MacCoexPti {
        let [one, three, ten, fifteen] = self.pti;
        match event {
            MacCoexEvent::Event1 => one,
            MacCoexEvent::Event3 => three,
            MacCoexEvent::Event10 => ten,
            MacCoexEvent::Event15 => fifteen,
        }
    }
}

impl MacDelayEntropy for Sources {
    fn mac_delay_random(&mut self) -> u32 {
        self.delay_random
    }
}

impl MacSlowClockCalibrationSource for Sources {
    fn mac_slow_clock_calibration(&mut self) -> MacSlowClockCalibration {
        MacSlowClockCalibration::Unavailable
    }
}

impl MacTxPowerSource for Sources {
    fn mac_tx_power_pair(&mut self, rate: u8) -> MacTxPowerPair {
        let pair = self.tx_power.pair(rate);
        MacTxPowerPair {
            primary: pair.primary,
            alternate: pair.alternate,
        }
    }
}

/// A small generator of the pulses' and the stops' delays.
struct Spread(u32);

impl Spread {
    fn next(&mut self, bound: u32) -> u32 {
        // xorshift32.
        let mut x = self.0 | 1;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x % bound.max(1)
    }
}

// The session's future is built in a static place. Each of its large
// steps is pinned where it is awaited and polled in a frame of its own
// (`apart!`), and the PHY's operations answer in place (`poll_placed`), as
// `Esp32s31Radio` runs them (`owned_operation::run`): inlined into one
// poll, or built and returned on the radio executor's stack, their
// temporaries would lie beside the PHY's deepest calls on its interrupt
// stack.

/// Await `$future` pinned in place, polled by [`poll_apart`].
macro_rules! apart {
    ($future:expr) => {{
        let mut future = core::pin::pin!($future);
        core::future::poll_fn(|context| poll_apart(future.as_mut(), context)).await
    }};
}
#[cfg(feature = "ampdu")]
mod ampdu;
#[cfg(feature = "amsdu")]
mod amsdu;
#[cfg(feature = "ccmp")]
mod ccmp;
#[cfg(feature = "receiver")]
mod receiver;
#[cfg(feature = "stream")]
mod stream;
#[cfg(feature = "transmit")]
mod transmit;
#[cfg(all(feature = "transmit", feature = "receiver"))]
compile_error!("part two's transmit and receiver are two images: choose one");

#[cfg(any(feature = "transmit", feature = "receiver"))]
/// A received frame's Address 1 and Address 2. The frame follows the
/// control prefix and its tail; a frame with HE-SIG-B options between them
/// is not the peer's (6 Mb/s).
pub(super) fn addresses(bytes: &[u8]) -> Option<([u8; 6], [u8; 6])> {
    let prefix = oer_chips_esp32s31_hal::layouts::WifiRxControlPrefix::new(bytes.first_chunk()?);
    if prefix.he_sig_b().present() {
        return None;
    }
    let frame = bytes.get(
        oer_chips_esp32s31_hal::layouts::WifiRxControlPrefix::SIZE
            + oer_chips_esp32s31_hal::layouts::WifiRxControlTail::SIZE..,
    )?;
    Some((
        frame.get(4..10)?.try_into().ok()?,
        frame.get(10..16)?.try_into().ok()?,
    ))
}

type SessionRun = impl core::future::Future<Output = ()>;
static SESSION: static_cell::ConstStaticCell<oer_portable_memory::StaticPlace<SessionRun>> =
    static_cell::ConstStaticCell::new(oer_portable_memory::StaticPlace::new());
#[define_opaque(SessionRun)]
fn session_future(parts: SessionParts, lp: LpClock<RcSlow>) -> SessionRun {
    async move {
        let Some(journal) = JOURNAL.try_take() else {
            return;
        };
        journal.used = 0;
        if let Err((phase, index)) = apart!(session(parts, &lp, journal)) {
            journal.record(Kind::Failed, &[phase, index]);
        }
        let _ = DONE.install(journal);
    }
}

async fn session_task(parts: SessionParts, lp: LpClock<RcSlow>) {
    let Some(place) = SESSION.try_take() else {
        return;
    };
    place.future(|| session_future(parts, lp)).await;
}

/// The phases a failure names.
mod phase {
    pub const REGISTER: u32 = 1;
    pub const POWER: u32 = 2;
    pub const CLOCKS: u32 = 3;
    pub const INITIALIZED: u32 = 4;
    pub const JOIN: u32 = 5;
    pub const MAINTAIN: u32 = 6;
    pub const RX_ENABLE: u32 = 7;
    pub const CHANNEL: u32 = 8;
    pub const POWER_PROFILE: u32 = 9;
    pub const MAC_COLD: u32 = 10;
    pub const RING: u32 = 11;
    pub const RECEIVE: u32 = 12;
    pub const KEY: u32 = 13;
    pub const STOP: u32 = 14;
    pub const WALKER_STOP: u32 = 15;
    pub const RESTART: u32 = 16;
    pub const SECOND_COLD: u32 = 17;
    pub const SECOND_RING: u32 = 18;
    pub const CEILING: u32 = 19;
}

type Failure = (u32, u32);

/// The session's radio, where it stays: taken by a PHY operation that
/// owns it and put back by its answer, as `Esp32s31Radio` holds it.
fn held(slot: &mut Option<Radio>) -> Result<&mut Radio, Failure> {
    slot.as_mut().ok_or((phase::REGISTER, 9))
}

/// The PHY's registration over the radio in `slot`.
async fn register(
    slot: &mut Option<Radio>,
    timer: &CooperativeTimer,
    platform: &mut (),
    clocks: &ChipClocks,
    identity: PhyCalibrationIdentity,
) -> Result<(), Failure> {
    let radio = slot.take().ok_or((phase::REGISTER, 1))?;
    let mut operation = core::pin::pin!(register_concurrent_phy::<_, (), RomShortDelay, _>(
        timer,
        radio,
        platform,
        clocks,
        PhyRegisterConfig::new(identity),
        NoopPhyTargetObserver,
    ));
    let mut completion = core::mem::MaybeUninit::uninit();
    let (returned, result) = oer_portable_memory::poll_placed(operation.as_mut(), &mut completion)
        .await
        .split();
    *slot = Some(returned.read());
    // The outcome (its calibration path) is the registration's report, as
    // `Esp32s31Radio::prepare` reads it.
    let _outcome = result.read().map_err(|_| (phase::REGISTER, 0))?.outcome();
    Ok(())
}

/// The PHY's tracking over the radio in `slot`.
async fn maintain(
    slot: &mut Option<Radio>,
    timer: &CooperativeTimer,
    platform: &mut (),
    pilot: &mut phy_pilot::Client,
) -> Result<(), Failure> {
    let radio = slot.take().ok_or((phase::MAINTAIN, 1))?;
    let mut operation = core::pin::pin!(maintain_concurrent_phy::<_, (), RomShortDelay, _>(
        timer,
        radio,
        platform,
        NoopPhyTargetObserver,
        pilot,
    ));
    let mut completion = core::mem::MaybeUninit::uninit();
    let (returned, result) = oer_portable_memory::poll_placed(operation.as_mut(), &mut completion)
        .await
        .split();
    *slot = Some(returned.read());
    result.read().map_err(|_| (phase::MAINTAIN, 0))?;
    Ok(())
}

/// The channel's selection over the radio in `slot`: a 20 MHz channel is
/// lowered to its primary number and CBW 0 (`radio-system`
/// `channel::lower_ghz2_4_channel`).
async fn tune(
    slot: &mut Option<Radio>,
    clocked: &mut WifiClocked,
    timer: &CooperativeTimer,
    platform: &mut (),
    channel: Ghz2_4Channel,
) -> Result<(), Failure> {
    let radio = slot.take().ok_or((phase::CHANNEL, 2))?;
    let transaction =
        radio.detach(oer_chips_esp32s31_hal::radio::domain::transaction::Operation::Channel);
    let mut observer = NoopPhyTargetObserver;
    let selection = clocked.channel_transaction(platform, transaction);
    let (by_frequency, cbw) = lowered(channel.width());
    let channel_or_frequency = if by_frequency {
        channel.center_frequency_mhz()
    } else {
        u16::from(channel.primary())
    };
    let mut operation = core::pin::pin!(select_concurrent_wifi_channel::<RomShortDelay, _, _, _>(
        timer,
        channel_or_frequency,
        cbw,
        selection,
        &mut observer,
    ));
    let mut completion = core::mem::MaybeUninit::uninit();
    let (returned, result) = oer_portable_memory::poll_placed(operation.as_mut(), &mut completion)
        .await
        .split();
    *slot = Some(returned.read());
    result.read().map_err(|_| (phase::CHANNEL, 1))?;
    Ok(())
}

/// The lowered channel width of the probe's channel, as `radio-system`'s
/// `channel::lower_ghz2_4_channel` gives it: what the radio system's MAC
/// reset writes back into the PHY's channel field.
const fn probe_cbw() -> u8 {
    lowered(probe_width()).1
}

/// The probe's channel width: 40 MHz below for the foreign words, 20 MHz
/// otherwise.
const fn probe_width() -> ChannelWidth {
    if cfg!(feature = "foreign") {
        ChannelWidth::Mhz40Below
    } else {
        ChannelWidth::Mhz20
    }
}

/// `radio-system`'s `channel::lower_ghz2_4_channel` for the probe's channel:
/// the PHY's channel-or-frequency argument and its `cbw`.
const fn lowered(width: ChannelWidth) -> (bool, u8) {
    match width {
        ChannelWidth::Mhz20 => (false, 0),
        ChannelWidth::Mhz40Above => (true, 2),
        ChannelWidth::Mhz40Below => (true, 3),
    }
}

/// The MAC's cold preparation: the block's reset, then the preparation that
/// takes its proof.
async fn cold(
    radio: &mut Radio,
    clocked: &mut WifiClocked,
    sources: &mut Sources,
    timer: &CooperativeTimer,
    mac: MacColdStartConfig,
) -> Result<(), ()> {
    let reset = radio.reset_wifi_mac_block(clocked.clocks_on(), probe_cbw());
    let mut hal = clocked.cold_mac_hal();
    initialize_wifi_mac(reset, sources, &mut hal, timer, mac)
        .await
        .map(|_outcome| ())
        .map_err(|_| ())
}

/// Bring Wi-Fi up in `Esp32s31Radio`'s order, then the series.
async fn session(
    parts: SessionParts,
    _lp: &LpClock<RcSlow>,
    journal: &mut Journal,
) -> Result<(), Failure> {
    let SessionParts {
        radio,
        clocks,
        wifi,
        coex,
        mut pilot,
        identity,
        mut mac_irq,
        mut power_irq,
        seed,
        station,
        ap,
    } = parts;
    let mut slot = Some(radio);
    let timer = CooperativeTimer::new(EmbassyClock);
    let mut platform = ();
    // prepare: the PHY's registration.
    apart!(register(
        &mut slot,
        &timer,
        &mut platform,
        &clocks,
        identity
    ))?;
    // `tasks/185` 1: the words between the PHY's registration and the
    // Wi-Fi power entry's pulse.
    #[cfg(feature = "foreign")]
    foreign(journal, held(&mut slot)?, 0, 9);
    // enter: coexistence, common power, the modem clocks, the PHY's client.
    let radio = held(&mut slot)?;
    let (coexisting, _participants) = radio.enter_coex(coex);
    let mut coexisting = Some(coexisting);
    let (powered, power) = WifiCold::from_partition(wifi)
        .power_up(radio, &clocks)
        .map_err(|_| (phase::POWER, 0))?;
    let mut clocked = powered
        .enable_clocks(radio, &clocks)
        .map_err(|_| (phase::CLOCKS, 0))?;
    clocked
        .set_initialized(radio, true)
        .map_err(|_| (phase::INITIALIZED, 0))?;
    let (membership, acquired) = {
        let ready = phy_ready(radio).map_err(|_| (phase::JOIN, 0))?;
        let PhyClientAccess::Available(available) = ready.client::<WifiPhyMembership>() else {
            return Err((phase::JOIN, 1));
        };
        let joined = join_wifi(available, &clocked, &timer).map_err(|_| (phase::JOIN, 2))?;
        (joined.membership, joined.acquired)
    };
    let mut power = Some(power);
    let mut membership = Some(membership);
    // track: the initial tracking, RX enable, the channel, the MAC.
    if acquired == ConcurrentAcquire::TrackingDue {
        apart!(maintain(&mut slot, &timer, &mut platform, &mut pilot))?;
    }
    set_wifi_rx(
        held(&mut slot)?,
        membership.as_ref().ok_or((phase::RX_ENABLE, 1))?,
        true,
    )
    .map_err(|_| (phase::RX_ENABLE, 0))?;
    // A 40 MHz channel for the foreign words, so that the PHY's channel
    // nibble is not zero.
    let channel = Ghz2_4Channel::new(CHANNEL, probe_width()).map_err(|_| (phase::CHANNEL, 0))?;
    apart!(tune(
        &mut slot,
        &mut clocked,
        &timer,
        &mut platform,
        channel
    ))?;
    let radio = held(&mut slot)?;
    let ceiling = Country::new(*b"DE")
        .ok()
        .and_then(RegulatoryDomain::of)
        .and_then(|domain| {
            domain
                .permit(oer_protocols_ieee80211_contracts_port::RegulatedChannel::from(channel))
                .ok()
        })
        .map(|permitted| {
            permitted.tx_ceiling(board::wifi_power_limits(
                i32::from(PhyConfig::production().highest_target_power_quarter_dbm()) * 25,
                Some(1000),
            ))
        })
        .ok_or((phase::CEILING, 0))?;
    let tx_power = radio
        .attachment()
        .phy_state()
        .map_err(|_| (phase::POWER_PROFILE, 0))?
        .tx_target_power_profile()
        .with_maximum_quarter_dbm(ceiling.quarter_dbm());
    let events = [
        MacCoexEvent::Event1,
        MacCoexEvent::Event3,
        MacCoexEvent::Event10,
        MacCoexEvent::Event15,
    ];
    let mut sources = Sources {
        pti: events.map(|event| MacCoexPti::from(radio.coex_pti(event.coex_event()))),
        tx_power,
        delay_random: seed,
    };
    let mac = MacColdStartConfig {
        handshake_deadline: MAC_COLD_HANDSHAKE_DEADLINE,
        station_address: station,
        access_point_address: ap,
    };
    apart!(cold(radio, &mut clocked, &mut sources, &timer, mac))
        .map_err(|()| (phase::MAC_COLD, 0))?;
    #[cfg(feature = "foreign")]
    {
        mark(1);
        journal.record(Kind::Up, &[u32::from(channel.center_frequency_mhz())]);
        for repetition in 0..FOREIGN_REPETITIONS {
            let index = u32::try_from(repetition).unwrap_or(u32::MAX);
            foreign(journal, held(&mut slot)?, index, 0);
            let _pulse = held(&mut slot)?.reset_wifi_mac_block(clocked.clocks_on(), probe_cbw());
            foreign(journal, held(&mut slot)?, index, 1);
            apart!(cold(
                held(&mut slot)?,
                &mut clocked,
                &mut sources,
                &timer,
                mac
            ))
            .map_err(|()| (phase::MAC_COLD, 1))?;
            foreign(journal, held(&mut slot)?, index, 2);
        }
        // `tasks/185` 1: the PHY's two writes again after the preparation,
        // the channel and the registration's I2C TX-rate fields; read;
        // pulse; read; prepare; read.
        for repetition in 0..FOREIGN_REPETITIONS {
            let index = u32::try_from(repetition).unwrap_or(u32::MAX);
            apart!(tune(
                &mut slot,
                &mut clocked,
                &timer,
                &mut platform,
                channel
            ))?;
            let radio = slot.take().ok_or((phase::CHANNEL, 3))?;
            let mut transaction = clocked.channel_transaction(
                &mut platform,
                radio
                    .detach(oer_chips_esp32s31_hal::radio::domain::transaction::Operation::Channel),
            );
            {
                let (mut hal, _) = transaction.parts_mut();
                oer_chips_esp32s31_hal::radio::phy::baseband::configure_i2c_tx_rate(&mut hal);
            }
            slot = Some(transaction.reunite());
            foreign(journal, held(&mut slot)?, index, 3);
            let _pulse = held(&mut slot)?.reset_wifi_mac_block(clocked.clocks_on(), probe_cbw());
            foreign(journal, held(&mut slot)?, index, 4);
            apart!(cold(
                held(&mut slot)?,
                &mut clocked,
                &mut sources,
                &timer,
                mac
            ))
            .map_err(|()| (phase::MAC_COLD, 2))?;
            foreign(journal, held(&mut slot)?, index, 5);
        }
        // Every owner stays held until the leased host reset.
        let _held = (
            slot,
            &mut membership,
            &mut coexisting,
            &mut power,
            clocked,
            &mut mac_irq,
            &mut power_irq,
            &mut pilot,
        );
        return Ok(());
    }
    #[cfg(not(feature = "foreign"))]
    {
        let (access, clocks_on) = clocked.into_running();

        // The series is polled in a frame of its own: inlined into this
        // function's poll, its temporaries would lie on the stack beside the
        // PHY's deepest calls on the radio executor's interrupt stack.
        let result = {
            let mut series = core::pin::pin!(series(
                Owners {
                    slot: &mut slot,
                    mac_irq: &mut mac_irq,
                    power_irq: &mut power_irq,
                    sources: &mut sources,
                    timer: &timer,
                    client: Client {
                        coex: &mut coexisting,
                        power: &mut power,
                        membership: &mut membership,
                        pilot: &mut pilot,
                        clocks: &clocks,
                    },
                    station,
                },
                access,
                clocks_on,
                mac,
                channel,
                seed,
                journal,
            ));
            core::future::poll_fn(|context| poll_apart(series.as_mut(), context)).await
        };
        // Every owner stays held until the leased host reset.
        let _held = (slot, membership, coexisting, power);
        return result;
    }
}

/// `tasks/184` A and `tasks/185` 1: record the three foreign words of
/// `WIFI_MAC` at `point` of `repetition`: 0 before the pulse, 1 after it,
/// 2 after the cold preparation; 3 after the channel and the I2C TX-rate
/// fields written again, 4 after the pulse, 5 after the preparation; 9
/// between the PHY's registration and the Wi-Fi power entry.
#[cfg(feature = "foreign")]
fn foreign(journal: &mut Journal, radio: &Radio, repetition: u32, point: u32) {
    let words = radio.foreign_wifi_mac_words();
    journal.record(
        Kind::Foreign,
        &[
            repetition,
            point,
            words.primary_20_channel_offset.get(),
            (words.minimum_power_high.get() << 4) | words.minimum_power_low.get(),
            words.channel_bits_10_16.get(),
            u32::from(words.channel_bit_17),
            words.channel_bits_18_31.get(),
            words.i2c_bits_0_1.get(),
            u32::from(words.i2c_bits_18_25),
            words.init_low.get(),
            u32::from(words.init_bit_31),
        ],
    );
}

/// What the series borrows from the session.
struct Owners<'a> {
    slot: &'a mut Option<Radio>,
    mac_irq: &'a mut MacInterruptRegisters,
    power_irq: &'a mut MacPowerInterruptRegisters,
    sources: &'a mut Sources,
    timer: &'a CooperativeTimer,
    client: Client<'a>,
    station: [u8; 6],
}

/// The Wi-Fi client's owners a leave and a join take and give back.
struct Client<'a> {
    coex: &'a mut Option<
        oer_chips_esp32s31_hal::radio::domain::Coexisting<
            oer_chips_esp32s31_hal::radio::domain::WifiClient,
        >,
    >,
    power: &'a mut Option<
        oer_chips_esp32s31_hal::radio::domain::CommonPower<
            oer_chips_esp32s31_hal::radio::domain::WifiClient,
        >,
    >,
    membership: &'a mut Option<WifiPhyMembership>,
    pilot: &'a mut phy_pilot::Client,
    clocks: &'a ChipClocks,
}

/// Poll `future` in this function's frame, never inlined into its caller's.
#[inline(never)]
fn poll_apart<F: core::future::Future>(
    future: core::pin::Pin<&mut F>,
    context: &mut core::task::Context<'_>,
) -> core::task::Poll<F::Output> {
    future.poll(context)
}

/// The stops, then the pulses, over the running MAC.
async fn series(
    owners: Owners<'_>,
    mut access: RadioAccess,
    clocks_on: oer_chips_esp32s31_hal::radio::ieee80211::client::WifiClocksOn,
    mac: MacColdStartConfig,
    channel: Ghz2_4Channel,
    seed: u32,
    journal: &mut Journal,
) -> Result<(), Failure> {
    let Owners {
        slot,
        mac_irq,
        power_irq,
        sources,
        timer,
        client,
        station,
    } = owners;
    // The receive ring, live, with the open promiscuous receive.
    let placed =
        RxDmaStorage::place(RX.take().ok_or((phase::RING, 0))?).map_err(|_| (phase::RING, 1))?;
    let storage = placed.storage();
    let prepared = placed
        .prepare_ring(access.rx_dma())
        .map_err(|_| (phase::RING, 2))?;
    let ready = access.wifi_mac_hal().prepare_mac_runtime(prepared);
    let stopped = ready
        .publish(access.rx_dma())
        .map_err(|_| (phase::RING, 3))?;
    // The port's start, in its order: the open receive, the interrupt
    // epoch, the walker, the MAC's runtime.
    access.wifi_mac_hal().configure_open_promiscuous_receive();
    station_policy(&mut access);
    open_epoch(mac_irq, power_irq);
    let mut ring = stopped
        .try_start(access.rx_dma())
        .map_err(|_| (phase::RING, 4))?;
    access.wifi_mac_hal().start_mac_runtime(&ring);
    let units = receive(storage, &mut ring, &mut access, RECEIVE_MS)
        .await
        .map_err(|code| (phase::RECEIVE, code))?;
    journal.record(Kind::Up, &[units]);

    #[cfg(feature = "transmit")]
    let result = {
        let _ = client;
        apart!(transmit::series(
            transmit::TransmitParts {
                storage,
                slot,
                mac_irq,
                power_irq,
                sources,
                timer,
                mac,
                channel,
                station,
                seed,
            },
            access,
            clocks_on,
            ring,
            journal,
        ))
    };
    #[cfg(feature = "receiver")]
    let result = {
        let _ = (
            client, slot, mac_irq, power_irq, sources, timer, mac, channel, seed,
        );
        apart!(receiver::series(
            receiver::ReceiverParts {
                storage,
                station,
                journal,
            },
            access,
            clocks_on,
            ring,
        ))
    };
    #[cfg(not(any(feature = "transmit", feature = "receiver")))]
    let result = {
        let _ = station;
        apart!(part_one(
            PartOne {
                storage,
                slot,
                mac_irq,
                power_irq,
                sources,
                timer,
                client,
                mac,
                channel,
                seed,
            },
            access,
            clocks_on,
            ring,
            journal,
        ))
    };
    result
}

/// What part one borrows from the series.
#[cfg(not(any(feature = "transmit", feature = "receiver")))]
struct PartOne<'a> {
    storage: &'static Storage,
    slot: &'a mut Option<Radio>,
    mac_irq: &'a mut MacInterruptRegisters,
    power_irq: &'a mut MacPowerInterruptRegisters,
    sources: &'a mut Sources,
    timer: &'a CooperativeTimer,
    client: Client<'a>,
    mac: MacColdStartConfig,
    channel: Ghz2_4Channel,
    seed: u32,
}

/// Part one (`tasks/084`): the recovery cases, the stops, the pulses, the
/// quiet pulses and the holds, all under receive.
#[cfg(not(any(feature = "transmit", feature = "receiver")))]
async fn part_one(
    parts: PartOne<'_>,
    mut access: RadioAccess,
    mut clocks_on: oer_chips_esp32s31_hal::radio::ieee80211::client::WifiClocksOn,
    mut ring: Ring,
    journal: &mut Journal,
) -> Result<(), Failure> {
    let PartOne {
        storage,
        slot,
        mac_irq,
        power_irq,
        sources,
        timer,
        client,
        mac,
        channel,
        seed,
    } = parts;
    // `tasks/097`: the recovery cases, from the healthy start on.
    let (cased_access, cased_clocks, cased_ring) = apart!(cases(
        CaseParts {
            storage,
            slot: &mut *slot,
            mac_irq: &mut *mac_irq,
            power_irq: &mut *power_irq,
            sources: &mut *sources,
            timer,
            client,
            mac,
            channel,
        },
        access,
        clocks_on,
        ring,
        journal,
    ))?;
    access = cased_access;
    clocks_on = cased_clocks;
    ring = cased_ring;

    let mut spread = Spread(seed);
    let series_start_cycle = oer_portable_riscv::cycle32();
    let series_start = EmbassyClock.now();

    // Question 4: ordinary stops under receive.
    for index in 0..STOPS {
        let index32 = u32::try_from(index).unwrap_or(u32::MAX);
        let counted = statistics(&access);
        let units = receive(storage, &mut ring, &mut access, 1 + spread.next(5))
            .await
            .map_err(|code| (phase::RECEIVE, code))?;
        let counted_after = statistics(&access);
        let begin = oer_portable_riscv::cycle32();
        access.wifi_mac_hal().request_channel_stop();
        let requested = oer_portable_riscv::cycle32();
        let halted = match ring.try_stop(access.rx_dma()) {
            Ok(halted) => halted,
            Err((_live, _error)) => return Err((phase::WALKER_STOP, index32)),
        };
        let walker_end = oer_portable_riscv::cycle32();
        let bound = EmbassyClock
            .now()
            .checked_add(Duration::from_millis(STOP_BOUND_MS))
            .ok_or((phase::STOP, index32))?;
        let mut active = access.wifi_mac_hal().channel_active_state();
        while active != 0 && EmbassyClock.now() < bound {
            active = access.wifi_mac_hal().channel_active_state();
        }
        let idle = oer_portable_riscv::cycle32();
        let snapshot = access.receive_dma_snapshot();
        journal.record(
            Kind::Stop,
            &[
                index32,
                units,
                requested.wrapping_sub(begin),
                walker_end.wrapping_sub(begin),
                idle.wrapping_sub(begin),
                u32::from(active),
                mac_irq.mac_interrupt_raw().bits(),
                walker(&snapshot)[0],
                walker(&snapshot)[1],
                advance(counted, counted_after, 0, 1),
                advance(counted, counted_after, 2, 3),
            ],
        );
        // An ordinary start again, in the port's order: the ring rebuilt,
        // the open receive, the epoch, the walker, the MAC released.
        let stopped = storage
            .prepare_halted(halted, access.rx_dma())
            .map_err(|_| (phase::RESTART, index32))?;
        access.wifi_mac_hal().reopen_mac_core(&stopped);
        access.wifi_mac_hal().configure_open_promiscuous_receive();
        station_policy(&mut access);
        open_epoch(mac_irq, power_irq);
        ring = stopped
            .try_start(access.rx_dma())
            .map_err(|_| (phase::RESTART, index32 | 0x8000_0000))?;
        access.wifi_mac_hal().start_mac_runtime(&ring);
    }

    // Questions 1 to 3: pulses mid-run, each followed by a second run.
    for index in 0..PULSES {
        let index32 = u32::try_from(index).unwrap_or(u32::MAX);
        let key = install_key(&mut access).map_err(|code| (phase::KEY, code))?;
        let counted = statistics(&access);
        let units_before = receive(storage, &mut ring, &mut access, RECEIVE_MS)
            .await
            .map_err(|code| (phase::RECEIVE, code))?;
        let counted_before = statistics(&access);
        let delay = spread.next(PULSE_SPREAD_US);
        wait_micros(u64::from(delay)).await;
        let before = access.receive_dma_snapshot();
        let pulse_cycle = oer_portable_riscv::cycle32();
        let (pulsed, pulsed_clocks) = {
            let mut clocked = WifiClocked::from_running(access, clocks_on);
            let _pulse = held(slot)?.reset_wifi_mac_block(clocked.clocks_on(), probe_cbw());
            clocked.into_running()
        };
        access = pulsed;
        clocks_on = pulsed_clocks;
        let pulsed_end = oer_portable_riscv::cycle32();
        let active = access.wifi_mac_hal().channel_active_state();
        let raw = mac_irq.mac_interrupt_raw().bits();
        let power = power_causes(&power_irq.power_interrupt_status());
        let after = access.receive_dma_snapshot();
        // The vendor's order (`wifi_reset_mac`): receive disabled at once
        // after the pulse, then the cold preparation, the channel, the
        // start. The reset state's quiet is the hold case's question.
        let halted = match ring.try_stop(access.rx_dma()) {
            Ok(halted) => halted,
            Err((_live, _error)) => return Err((phase::WALKER_STOP, index32 | 0x4000_0000)),
        };
        let key_after = access.wifi_mac_hal().ccmp_entry_is_valid(key);
        let second_cold = {
            let mut clocked = WifiClocked::from_running(access, clocks_on);
            let result = {
                let reset = held(slot)?.reset_wifi_mac_block(clocked.clocks_on(), probe_cbw());
                let mut hal = clocked.cold_mac_hal();
                initialize_wifi_mac(reset, sources, &mut hal, timer, mac).await
            };
            let (running, on) = clocked.into_running();
            access = running;
            clocks_on = on;
            result
        };
        second_cold.map_err(|_| (phase::SECOND_COLD, index32))?;
        // The channel again, with no new calibration: the pulse resets the
        // MAC block's channel and bandwidth words, which the cold
        // preparation does not write.
        let retuned = {
            let mut clocked = WifiClocked::from_running(access, clocks_on);
            let mut platform = ();
            let result = apart!(tune(slot, &mut clocked, timer, &mut platform, channel));
            let (running, on) = clocked.into_running();
            access = running;
            clocks_on = on;
            result
        };
        retuned.map_err(|(phase, code)| (phase, code | 0x0100_0000 | index32 << 8))?;
        ring = restart_after_cold(storage, halted, &mut access, mac_irq, power_irq)?;
        let counted_second = statistics(&access);
        let units_after = receive(storage, &mut ring, &mut access, RECEIVE_MS)
            .await
            .map_err(|code| (phase::RECEIVE, code | 0x4000_0000))?;
        let counted_second_after = statistics(&access);
        let key_second = access.wifi_mac_hal().ccmp_entry_is_valid(key);
        access.wifi_mac_hal().clear_ccmp_entry(KEY_ENTRY);
        journal.record(
            Kind::Pulse,
            &[
                index32,
                units_before,
                delay,
                pulse_cycle,
                pulsed_end.wrapping_sub(pulse_cycle),
                walker(&before)[0],
                walker(&before)[1],
                walker(&after)[0],
                walker(&after)[1],
                walker(&after)[2],
                walker(&after)[3],
                u32::from(active),
                raw,
                power,
                u32::from(key_after),
                units_after,
                u32::from(key_second),
                advance(counted, counted_before, 0, 1),
                advance(counted_second, counted_second_after, 0, 1),
            ],
        );
    }

    // Question 1's walker flag: a pulse with the walker stopped first, so
    // that what the flag reads after it is the block's reset value.
    for index in 0..QUIET {
        let index32 = u32::try_from(index).unwrap_or(u32::MAX);
        let _units = receive(storage, &mut ring, &mut access, RECEIVE_MS)
            .await
            .map_err(|code| (phase::RECEIVE, code | 0x2000_0000))?;
        access.wifi_mac_hal().request_channel_stop();
        let halted = match ring.try_stop(access.rx_dma()) {
            Ok(halted) => halted,
            Err((_live, _error)) => return Err((phase::WALKER_STOP, index32 | 0x2000_0000)),
        };
        let before = access.receive_dma_snapshot();
        let (pulsed, pulsed_clocks) = {
            let mut clocked = WifiClocked::from_running(access, clocks_on);
            let _pulse = held(slot)?.reset_wifi_mac_block(clocked.clocks_on(), probe_cbw());
            clocked.into_running()
        };
        access = pulsed;
        clocks_on = pulsed_clocks;
        let after = access.receive_dma_snapshot();
        let second_cold = {
            let mut clocked = WifiClocked::from_running(access, clocks_on);
            let result = {
                let reset = held(slot)?.reset_wifi_mac_block(clocked.clocks_on(), probe_cbw());
                let mut hal = clocked.cold_mac_hal();
                initialize_wifi_mac(reset, sources, &mut hal, timer, mac).await
            };
            let (running, on) = clocked.into_running();
            access = running;
            clocks_on = on;
            result
        };
        second_cold.map_err(|_| (phase::SECOND_COLD, index32 | 0x2000_0000))?;
        let cold_after = access.receive_dma_snapshot();
        let stopped = storage
            .prepare_halted(halted, access.rx_dma())
            .map_err(|_| (phase::SECOND_RING, index32 | 0x2000_0000))?;
        access.wifi_mac_hal().reopen_mac_core(&stopped);
        access.wifi_mac_hal().configure_open_promiscuous_receive();
        station_policy(&mut access);
        open_epoch(mac_irq, power_irq);
        ring = stopped
            .try_start(access.rx_dma())
            .map_err(|_| (phase::SECOND_RING, index32 | 0x6000_0000))?;
        access.wifi_mac_hal().start_mac_runtime(&ring);
        journal.record(
            Kind::Quiet,
            &[
                index32,
                walker(&before)[0],
                walker(&before)[3],
                walker(&after)[0],
                walker(&after)[1],
                walker(&after)[2],
                walker(&after)[3],
                walker(&cold_after)[0],
                walker(&cold_after)[3],
            ],
        );
    }
    // The reset state under load: the pulse with the walker live, then no
    // cold preparation for HOLD_MS while the injector's frames arrive.
    for index in 0..HOLDS {
        let index32 = u32::try_from(index).unwrap_or(u32::MAX);
        let units = receive(storage, &mut ring, &mut access, RECEIVE_MS)
            .await
            .map_err(|code| (phase::RECEIVE, code | 0x1000_0000))?;
        let (pulsed, pulsed_clocks) = {
            let mut clocked = WifiClocked::from_running(access, clocks_on);
            let _pulse = held(slot)?.reset_wifi_mac_block(clocked.clocks_on(), probe_cbw());
            clocked.into_running()
        };
        access = pulsed;
        clocks_on = pulsed_clocks;
        let at_pulse = access.receive_dma_snapshot();
        let counted = statistics(&access);
        paint(storage, index32 | 0x80).map_err(|code| (phase::RING, code))?;
        wait_micros(u64::from(HOLD_MS) * 1_000).await;
        let kept = access.receive_dma_snapshot();
        let active = access.wifi_mac_hal().channel_active_state();
        let counted_after = statistics(&access);
        let changed = changed_words(storage, index32 | 0x80).map_err(|code| (phase::RING, code))?;
        let halted = match ring.try_stop(access.rx_dma()) {
            Ok(halted) => halted,
            Err((_live, _error)) => return Err((phase::WALKER_STOP, index32 | 0x1000_0000)),
        };
        let second_cold = {
            let mut clocked = WifiClocked::from_running(access, clocks_on);
            let result = {
                let reset = held(slot)?.reset_wifi_mac_block(clocked.clocks_on(), probe_cbw());
                let mut hal = clocked.cold_mac_hal();
                initialize_wifi_mac(reset, sources, &mut hal, timer, mac).await
            };
            let (running, on) = clocked.into_running();
            access = running;
            clocks_on = on;
            result
        };
        second_cold.map_err(|_| (phase::SECOND_COLD, index32 | 0x1000_0000))?;
        let stopped = storage
            .prepare_halted(halted, access.rx_dma())
            .map_err(|_| (phase::SECOND_RING, index32 | 0x1000_0000))?;
        access.wifi_mac_hal().reopen_mac_core(&stopped);
        access.wifi_mac_hal().configure_open_promiscuous_receive();
        station_policy(&mut access);
        open_epoch(mac_irq, power_irq);
        ring = stopped
            .try_start(access.rx_dma())
            .map_err(|_| (phase::SECOND_RING, index32 | 0x5000_0000))?;
        access.wifi_mac_hal().start_mac_runtime(&ring);
        journal.record(
            Kind::Hold,
            &[
                index32,
                units,
                walker(&at_pulse)[0],
                walker(&at_pulse)[1],
                walker(&at_pulse)[3],
                walker(&kept)[0],
                walker(&kept)[1],
                walker(&kept)[2],
                walker(&kept)[3],
                u32::from(active),
                advance(counted, counted_after, 0, 1),
                advance(counted, counted_after, 2, 3),
                changed,
            ],
        );
    }
    let series_cycles = oer_portable_riscv::cycle32().wrapping_sub(series_start_cycle);
    let series_micros = EmbassyClock
        .now()
        .checked_duration_since(series_start)
        .map_or(0, |elapsed| {
            u32::try_from(elapsed.as_micros()).unwrap_or(u32::MAX)
        });
    journal.record(Kind::End, &[series_cycles, series_micros]);
    // Every owner stays held until the leased host reset.
    let _held = (access, clocks_on, ring);
    Ok(())
}

/// What the recovery cases borrow.
struct CaseParts<'a, 'b> {
    storage: &'static Storage,
    slot: &'a mut Option<Radio>,
    mac_irq: &'a mut MacInterruptRegisters,
    power_irq: &'a mut MacPowerInterruptRegisters,
    sources: &'a mut Sources,
    timer: &'a CooperativeTimer,
    client: Client<'b>,
    mac: MacColdStartConfig,
    channel: Ghz2_4Channel,
}

type Ring = RxRingLive<'static, COUNT>;

/// Receive for `millis`: the units and the MAC's counts over it (MPDUs and
/// data successes, FCS errors and buffer-full events, each pair packed).
async fn window(
    storage: &'static Storage,
    ring: &mut Ring,
    access: &mut RadioAccess,
    millis: u32,
) -> Result<[u32; 3], Failure> {
    let counted = statistics(access);
    let units = receive(storage, ring, access, millis)
        .await
        .map_err(|code| (phase::RECEIVE, code | 0x0800_0000))?;
    let after = statistics(access);
    Ok([
        units,
        advance(counted, after, 0, 1),
        advance(counted, after, 2, 3),
    ])
}

/// The ring rebuilt and started in the port's order.
fn restart(
    storage: &'static Storage,
    halted: oer_chips_esp32s31_hal::radio::ieee80211::dma::rx_ring::RxRingHalted<'static, COUNT>,
    access: &mut RadioAccess,
    mac_irq: &mut MacInterruptRegisters,
    power_irq: &mut MacPowerInterruptRegisters,
) -> Result<Ring, Failure> {
    let stopped = storage
        .prepare_halted(halted, access.rx_dma())
        .map_err(|_| (phase::SECOND_RING, 0x0800_0000))?;
    access.wifi_mac_hal().reopen_mac_core(&stopped);
    access.wifi_mac_hal().configure_open_promiscuous_receive();
    station_policy(access);
    open_epoch(mac_irq, power_irq);
    let ring = stopped
        .try_start(access.rx_dma())
        .map_err(|_| (phase::SECOND_RING, 0x0800_0001))?;
    access.wifi_mac_hal().start_mac_runtime(&ring);
    Ok(ring)
}

/// The ring rebuilt and started after a cold preparation.
///
/// The cold preparation holds the MAC's common gates closed
/// (`hold_mac_runtime`), and `prepare_halted` publishes the window and
/// BASE before `reopen_mac_core` can be called (it needs the stopped ring
/// that publication makes). A BASE written into the gated core is not
/// kept: the walker starts on no descriptor (run 7). So the ring is
/// published once more after the gates open, as the first start publishes
/// it after `prepare_mac_runtime`: the stopped ring back to halted
/// (`into_halted`), prepared again, then the port's start.
fn restart_after_cold(
    storage: &'static Storage,
    halted: oer_chips_esp32s31_hal::radio::ieee80211::dma::rx_ring::RxRingHalted<'static, COUNT>,
    access: &mut RadioAccess,
    mac_irq: &mut MacInterruptRegisters,
    power_irq: &mut MacPowerInterruptRegisters,
) -> Result<Ring, Failure> {
    let gated = storage
        .prepare_halted(halted, access.rx_dma())
        .map_err(|_| (phase::SECOND_RING, 0x0800_0003))?;
    access.wifi_mac_hal().reopen_mac_core(&gated);
    restart(storage, gated.into_halted(), access, mac_irq, power_irq)
}

/// The phases a recovery case's failure names.
const CASE_FAILED: u32 = 20;

/// `tasks/097`'s cases in order, each measured by a window of its own: the
/// baseline, case 0 (an ordinary stop and the cold preparation again, no
/// pulse mid-run), case 1 (the pulse mid-run, then what `track` does
/// after the entry: RX enable, channel, cold preparation, start) and case
/// 3 (the client's leave, join and track, the registration kept). Case 2
/// is not callable on its own: the baseband and MAC pulse is the HAL's
/// `pub(crate)` step of a first `power_up`, which case 3 walks.
async fn cases(
    parts: CaseParts<'_, '_>,
    mut access: RadioAccess,
    mut clocks_on: oer_chips_esp32s31_hal::radio::ieee80211::client::WifiClocksOn,
    mut ring: Ring,
    journal: &mut Journal,
) -> Result<
    (
        RadioAccess,
        oer_chips_esp32s31_hal::radio::ieee80211::client::WifiClocksOn,
        Ring,
    ),
    Failure,
> {
    let CaseParts {
        storage,
        slot,
        mac_irq,
        power_irq,
        sources,
        timer,
        client,
        mac,
        channel,
    } = parts;
    let Client {
        coex,
        power,
        membership,
        pilot,
        clocks,
    } = client;
    let mut platform = ();
    let fail = |step: u32| (CASE_FAILED, step);
    let record = |journal: &mut Journal, case: u32, w: [u32; 3], next: u32| {
        let [units, seen, errors] = w;
        journal.record(Kind::Case, &[case, units, seen, errors, next]);
    };

    // The baseline.
    mark(10);
    let w = window(storage, &mut ring, &mut access, CASE_WINDOW_MS).await?;
    record(journal, 0xb, w, walker(&access.receive_dma_snapshot())[1]);

    // Case 0: an ordinary stop, the cold preparation again, the start.
    mark(20);
    access.wifi_mac_hal().request_channel_stop();
    let halted = ring.try_stop(access.rx_dma()).map_err(|_| fail(0x01))?;
    let mut clocked = WifiClocked::from_running(access, clocks_on);
    let cold = {
        let reset = held(slot)?.reset_wifi_mac_block(clocked.clocks_on(), probe_cbw());
        let mut hal = clocked.cold_mac_hal();
        initialize_wifi_mac(reset, &mut *sources, &mut hal, timer, mac).await
    };
    cold.map_err(|_| fail(0x02))?;
    (access, clocks_on) = clocked.into_running();
    ring = restart_after_cold(storage, halted, &mut access, mac_irq, power_irq)?;
    mark(21);
    let w = window(storage, &mut ring, &mut access, CASE_WINDOW_MS).await?;
    record(journal, 0, w, walker(&access.receive_dma_snapshot())[1]);

    // Case 1: the pulse mid-run, then `track` after the entry.
    mark(30);
    let mut clocked = WifiClocked::from_running(access, clocks_on);
    let _pulse = held(slot)?.reset_wifi_mac_block(clocked.clocks_on(), probe_cbw());
    set_wifi_rx(held(slot)?, membership.as_ref().ok_or(fail(0x10))?, true)
        .map_err(|_| fail(0x11))?;
    apart!(tune(slot, &mut clocked, timer, &mut platform, channel))?;
    (access, clocks_on) = clocked.into_running();
    let halted = ring.try_stop(access.rx_dma()).map_err(|_| fail(0x12))?;
    let mut clocked = WifiClocked::from_running(access, clocks_on);
    let cold = {
        let reset = held(slot)?.reset_wifi_mac_block(clocked.clocks_on(), probe_cbw());
        let mut hal = clocked.cold_mac_hal();
        initialize_wifi_mac(reset, &mut *sources, &mut hal, timer, mac).await
    };
    cold.map_err(|_| fail(0x13))?;
    (access, clocks_on) = clocked.into_running();
    ring = restart_after_cold(storage, halted, &mut access, mac_irq, power_irq)?;
    mark(31);
    let w = window(storage, &mut ring, &mut access, CASE_WINDOW_MS).await?;
    record(journal, 1, w, walker(&access.receive_dma_snapshot())[1]);

    // Case 3: leave, join and track by the client's public operations,
    // the PHY's registration kept.
    mark(50);
    access.wifi_mac_hal().request_channel_stop();
    let halted = ring.try_stop(access.rx_dma()).map_err(|_| fail(0x30))?;
    let clocked = WifiClocked::from_running(access, clocks_on);
    let left = membership.take().ok_or(fail(0x31))?;
    set_wifi_rx(held(slot)?, &left, false).map_err(|_| fail(0x32))?;
    let _last = leave_wifi(held(slot)?, &clocked, left).map_err(|_| fail(0x33))?;
    let powered = match clocked.release_clocks(held(slot)?) {
        Ok(released) => released.into_powered(),
        Err(_stopped) => return Err(fail(0x34)),
    };
    let (absent, _participants) = held(slot)?.leave_coex(coex.take().ok_or(fail(0x35))?);
    let cold_client = powered
        .power_down(held(slot)?, power.take().ok_or(fail(0x36))?, &absent)
        .map_err(|_| fail(0x37))?;
    let (entered, _participants) = held(slot)?.enter_coex(absent);
    *coex = Some(entered);
    let (powered, entered_power) = cold_client
        .power_up(held(slot)?, clocks)
        .map_err(|_| fail(0x38))?;
    *power = Some(entered_power);
    let mut clocked = powered
        .enable_clocks(held(slot)?, clocks)
        .map_err(|_| fail(0x39))?;
    clocked
        .set_initialized(held(slot)?, true)
        .map_err(|_| fail(0x3a))?;
    let acquired = {
        let ready = phy_ready(held(slot)?).map_err(|_| fail(0x3b))?;
        let PhyClientAccess::Available(available) = ready.client::<WifiPhyMembership>() else {
            return Err(fail(0x3c));
        };
        let joined = join_wifi(available, &clocked, timer).map_err(|_| fail(0x3d))?;
        *membership = Some(joined.membership);
        joined.acquired
    };
    if acquired == ConcurrentAcquire::TrackingDue {
        apart!(maintain(slot, timer, &mut platform, pilot))?;
    }
    set_wifi_rx(held(slot)?, membership.as_ref().ok_or(fail(0x3e))?, true)
        .map_err(|_| fail(0x3f))?;
    apart!(tune(slot, &mut clocked, timer, &mut platform, channel))?;
    let cold = {
        let reset = held(slot)?.reset_wifi_mac_block(clocked.clocks_on(), probe_cbw());
        let mut hal = clocked.cold_mac_hal();
        initialize_wifi_mac(reset, &mut *sources, &mut hal, timer, mac).await
    };
    cold.map_err(|_| fail(0x40))?;
    (access, clocks_on) = clocked.into_running();
    ring = restart_after_cold(storage, halted, &mut access, mac_irq, power_irq)?;
    mark(51);
    let w = window(storage, &mut ring, &mut access, CASE_WINDOW_MS).await?;
    record(journal, 3, w, walker(&access.receive_dma_snapshot())[1]);
    mark(60);
    Ok((access, clocks_on, ring))
}

/// The interrupt epoch as the port's start opened it (`MacIrqEpoch::activate`
/// with `MAC_STATION_INTERRUPT_MASK`): both banks masked and cleared, the
/// station's mask published, the station TBTT enabled. No line of this
/// image routes the Wi-Fi sources: the banks latch and nothing is taken.
fn open_epoch(mac_irq: &mut MacInterruptRegisters, power_irq: &mut MacPowerInterruptRegisters) {
    mac_irq.mask_all_mac_interrupts();
    power_irq.mask_all_power_interrupts();
    let stale = mac_irq.mac_interrupt_raw();
    mac_irq.acknowledge_mac_interrupt_raw(stale);
    power_irq.clear_all_power_interrupts();
    mac_irq.publish_mac_interrupt_mask(
        oer_chips_esp32s31_hal::radio::ieee80211::init::MAC_STATION_INTERRUPT_MASK,
    );
    power_irq.enable_station_tbtt_interrupt();
}

/// The station's receive policy under which a frame addressed to the
/// station is taken and acknowledged whatever its BSSID: the HAL's scan
/// transition, as the backend applies it (`StaScanRxPolicyHardware`: the
/// normal station transition, which clears queue three's
/// `AUTOACK_DISABLE`, then the scan's policy, with the broadcast BSSID). The
/// station's own address is the cold start's; no key is installed for it.
fn station_policy(access: &mut RadioAccess) {
    let broadcast = [0xff; 6];
    access
        .wifi_mac_hal()
        .configure_station_receive_policy(broadcast);
    access
        .wifi_mac_hal()
        .configure_station_scan_policy(broadcast);
}

/// The MAC's receive statistics this probe reads: MPDUs, data successes,
/// FCS errors and buffer-full events (`dbg_read_rx_count`'s counters).
fn statistics(access: &RadioAccess) -> [u16; 4] {
    let primary = access.receive_statistics_snapshot().primary;
    [
        primary.mpdu_count,
        primary.data_success,
        primary.fcs_error,
        primary.buffer_full,
    ]
}

/// Two counters' advance over a window, packed high and low.
fn advance(before: [u16; 4], after: [u16; 4], high: usize, low: usize) -> u32 {
    let delta = |index: usize| match (before.get(index), after.get(index)) {
        (Some(before), Some(after)) => u32::from(after.wrapping_sub(*before)),
        _ => 0,
    };
    delta(high) << 16 | delta(low)
}

/// The power bank's latched causes, one bit each in their declared order:
/// the four TSF timers, a cause no driver handles, the station's TBTT.
fn power_causes(
    snapshot: &oer_chips_esp32s31_hal::radio::ieee80211::registers::MacPowerInterruptSnapshot,
) -> u32 {
    use oer_chips_esp32s31_hal::radio::ieee80211::registers::MacPowerInterruptCause as Cause;
    let observed = snapshot.observation();
    [
        Cause::TsfTimer0,
        Cause::TsfTimer1,
        Cause::TsfTimer2,
        Cause::TsfTimer3,
        Cause::Unhandled,
        Cause::StaTbtt,
    ]
    .into_iter()
    .enumerate()
    .fold(0, |bits, (place, cause)| {
        bits | u32::from(observed.contains(cause)) << place
    })
}

/// The receive walker's state: its flags (bit 0 enabled, bit 1 a reload
/// pending, bit 2 the upper next field nonzero), its next and its last
/// descriptor, and its base.
fn walker(
    snapshot: &oer_chips_esp32s31_hal::radio::ieee80211::registers::MacRxDmaSnapshot,
) -> [u32; 4] {
    [
        u32::from(snapshot.walker_enabled)
            | u32::from(snapshot.reload_pending) << 1
            | u32::from(snapshot.next_descriptor_upper_unknown) << 2,
        snapshot.next_descriptor_low,
        snapshot.last_descriptor_low,
        snapshot.descriptor_base,
    ]
}

async fn wait_micros(micros: u64) {
    if let Some(at) = EmbassyClock
        .now()
        .checked_add(Duration::from_micros(micros))
    {
        EmbassyClock.wait_until(at).await;
    }
}

/// Install a pairwise key in the probe's entry; its index.
fn install_key(access: &mut RadioAccess) -> Result<MacKeyEntryIndex, u32> {
    let index = MacKeyEntryIndex::new(u32::from(KEY_ENTRY)).ok_or(1_u32)?;
    match access.wifi_mac_hal().install_station_ccmp_entry(
        KEY_ENTRY,
        MacCcmpKeyIdentity::Pairwise {
            peer: [0x02, 0, 0, 0, 0, 0x84],
        },
        &[0x5a; 16],
    ) {
        MacKeyInstallOutcome::Installed => Ok(index),
        MacKeyInstallOutcome::Occupied => Err(2),
        MacKeyInstallOutcome::Rejected => Err(3),
    }
}

/// Receive for `millis`, taking and recycling every completed unit; their
/// count.
async fn receive(
    storage: &Storage,
    ring: &mut RxRingLive<'_, COUNT>,
    access: &mut RadioAccess,
    millis: u32,
) -> Result<u32, u32> {
    let end = EmbassyClock
        .now()
        .checked_add(Duration::from_millis(millis))
        .ok_or(1_u32)?;
    let mut units = 0_u32;
    while EmbassyClock.now() < end {
        storage
            .observe_reserved(ring, access.rx_dma())
            .map_err(|_| 2_u32)?;
        ring.complete_pending_reload(access.rx_dma(), Settle::new(RELOAD_SETTLE, &EmbassyClock))
            .map_err(|_| 3_u32)?;
        let frozen = ring.freeze_cursor(access.rx_dma()).map_err(|_| 4_u32)?;
        if let Some(unit) = storage
            .take_completed_unit(ring, frozen)
            .map_err(|_| 5_u32)?
        {
            let credit = unit.retain_discard().ok_or(6_u32)?;
            let _appended = credit.append(access.rx_dma()).map_err(|_| 7_u32)?;
            units = units.saturating_add(1);
        } else {
            wait_micros(100).await;
        }
        ring.complete_pending_reload(access.rx_dma(), Settle::new(RELOAD_SETTLE, &EmbassyClock))
            .map_err(|_| 8_u32)?;
    }
    Ok(units)
}

fn node(storage: &Storage, index: usize) -> Option<&RxDescriptor> {
    if index == COUNT {
        Some(storage.reserved_descriptor())
    } else {
        storage.descriptors().get(index)
    }
}

/// The guard's word at `word` of buffer `index`, for pulse `pulse`.
const fn pattern(pulse: u32, index: usize, word: usize) -> u32 {
    0x5a00_0000 ^ (pulse << 16) ^ ((index as u32) << 8) ^ (word as u32)
}

/// The checked address of buffer `index` of the probe's own arena.
fn buffer_address(storage: &Storage, index: usize) -> Result<usize, u32> {
    let descriptor = node(storage, index).ok_or(20_u32)?;
    let address = descriptor.buffer_address() as usize;
    let base = core::ptr::from_ref(storage).addr();
    let limit = base
        .checked_add(core::mem::size_of::<Storage>())
        .ok_or(21_u32)?;
    let end = address.checked_add(BUFFER).ok_or(22_u32)?;
    if !address.is_multiple_of(4)
        || address < base
        || end > limit
        || usize::from(descriptor.word0().size()) != BUFFER
    {
        return Err(23);
    }
    Ok(address)
}

// This probe paints the ring's buffers after the pulse, while its
// descriptors stay published: the question is whether the MAC's bus master
// still writes them. Allowed in probe images (`docs/unsafe.md`).
#[allow(
    unsafe_code,
    reason = "probe-only guard paint of the probe's own DMA arena"
)]
fn paint(storage: &Storage, pulse: u32) -> Result<(), u32> {
    for index in 0..NODES {
        let address = buffer_address(storage, index)?;
        for word in 0..WORDS {
            // SAFETY: the address lies in the probe's own static arena, word
            // aligned, inside its buffer's advertised capacity (checked
            // above); only the MAC's DMA may write it besides this probe, and
            // that write is what the guard looks for. No reference is made.
            unsafe {
                (address as *mut u32)
                    .add(word)
                    .write_volatile(pattern(pulse, index, word));
            }
        }
    }
    oer_portable_riscv::fence();
    Ok(())
}

// The guard's read: CPU loads of SRAM, which are no traffic of the MAC's bus.
#[allow(
    unsafe_code,
    reason = "probe-only volatile read of the probe's own DMA arena"
)]
fn changed_words(storage: &Storage, pulse: u32) -> Result<u32, u32> {
    oer_portable_riscv::fence();
    let mut changed = 0_u32;
    for index in 0..NODES {
        let address = buffer_address(storage, index)?;
        for word in 0..WORDS {
            // SAFETY: as in `paint`; this load creates no reference.
            let value = unsafe { (address as *const u32).add(word).read_volatile() };
            changed = changed.saturating_add(u32::from(value != pattern(pulse, index, word)));
        }
    }
    Ok(changed)
}

oer_chips_family_platform_executor::time_driver!(Esp32s31);
