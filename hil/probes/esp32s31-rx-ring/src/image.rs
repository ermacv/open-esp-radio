//! The receive ring's probe: Wi-Fi brought up over the radio domain the
//! probe holds (`radio`), the ring started by the path table, then four
//! cases over case C's frames from the sender (`esp32s31-case-c-sender`):
//!
//! - R1, singles: each lent for a spare, checked by its payload;
//! - R2, chains: copied in place, checked by payload and length, their
//!   continuations' control words recorded by position;
//! - R3, a stall: no serve for 100 ms, the walker's standing read, then the
//!   exit by `serve`;
//! - R4, turns: the stop in its order ending in the release, every handle
//!   back, then a cold start and receive again.
//!
//! Every window counts the sender's units and the ring's anomalies by kind.
use crate::case_c::{CASES, Case, byte};
use crate::radio::{
    Brought, CooperativeTimer, Failure, Radio, SessionParts, apart, begin, bring_up, held, phase,
    prepare, stop,
};
use crate::usb;
use core::mem::MaybeUninit;
use oer_chips_esp32s31_hal::{
    layouts::DmaWindow,
    peripherals::usb_serial_jtag::ConsoleInterrupt,
    peripherals::watchdog::{ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters},
    peripherals::{
        efuse::{FactoryIdentity, Interface},
        software_interrupt::{FromCpu0Irq, FromCpu2Irq},
        system_timer::{SystimerAlarm0, SystimerAlarm1},
    },
    power::ChipClocks,
    radio::ieee80211::{
        client::{WifiClocked, WifiClocksOn},
        dma::{
            receive::{NextAt, RingAnomaly, RxDescriptors, RxRing, RxRingParts, RxTable, Unit},
            space::ChipDma,
        },
        init::MacColdStartConfig,
        registers::WifiRxDmaRegisters,
    },
    system::owner::{MacInterruptRegisters, MacPowerInterruptRegisters, RadioAccess},
    system::root::Root,
};
use oer_chips_esp32s31_platform_board as board;
use oer_chips_esp32s31_platform_runtime::Esp32s31;
use oer_chips_esp32s31_radio_phy::{
    PhyCalibrationIdentity, concurrent::ConcurrentPhy, phy_get_rf_cal_version,
};
use oer_chips_family_platform_executor::{Executor, InterruptExecutor};
use oer_portable_diagnostics::{Name, name};
use oer_portable_embassy_time::EmbassyClock;
use oer_portable_memory::arena::{Arena, Handle};
use oer_portable_time::{Clock, Duration, Instant, Timer};
use oer_protocols_ieee80211_wire::channel::Ghz2_4Channel;

/// The ring: sixteen descriptors and R; the arena's other fifteen buffers
/// are the spares a lent unit is exchanged for. Each buffer is 256 octets
/// with the 64-octet header below its floor: case C's 700- and 1 500-octet
/// frames are chains of three and of seven descriptors.
const RING: usize = 16;
const SIZE: usize = 256;
const FLOOR: usize = 64;
const BUFFERS: usize = 32;
const SPARES: usize = BUFFERS - RING - 1;
const BYTES: usize = BUFFERS * SIZE;
/// The longest frame copied in place, its FCS and padding included.
const COPY: usize = 1_600;

/// R1 and R2's window, R3's stall and its exit window, R4's turns and the
/// windows before and after each stop.
const WINDOW_MS: u32 = 2_000;
const STALL_MS: u32 = 100;
const EXIT_MS: u32 = 1_000;
const TURNS: usize = 50;
const TURN_MS: u32 = 200;
/// How long the probe waits for the sender's first unit, in steps.
const WAIT_MS: u32 = 120_000;
const WAIT_STEP_MS: u32 = 100;

/// The pass thresholds of `tasks/277` and `281`.
const R1_UNITS: u32 = 1_000;
const R2_CHAINS: u32 = 200;
const R3_UNITS: u32 = 100;
const R4_UNITS: u32 = 50;

oer_portable_memory::arena! {
    /// The ring's buffers and the spares.
    pub(crate) RingRx: window DmaWindow, count 32, size 256, floor 64,
        placed by oer_chips_esp32s31_hal::dma_static
        as oer_chips_esp32s31_hal::system::memory::dma_static::DmaStatic
        in ".dma.bss.rx_ring_probe";
}
oer_chips_esp32s31_hal::dma_static! {
    static RX_TABLE: RxDescriptors<RING> = zeroed in ".dma.bss.rx_ring_probe_table";
}
type Ring = RxRing<RingRx, BYTES, RING, WifiRxDmaRegisters>;
type Parts = RxRingParts<RingRx, BYTES, RING, WifiRxDmaRegisters>;
static RING_PLACE: static_cell::StaticCell<MaybeUninit<Ring>> = static_cell::StaticCell::new();

use crate::phy_pilot;

// SAFETY: the production runtime script maps .hot.text.* to executable SRAM;
// each generated wrapper retains the interrupt-table ABI and owned part.
oer_chips_family_platform_interrupt_table::interrupt_table! {
    lines: oer_chips_family_platform_runtime::interrupts::LINES,
    handler_attributes: [#[unsafe(link_section = ".hot.text.rx_ring_vectors")]];
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
        session(parts: SessionParts) = session_task;
    }
}
oer_chips_family_platform_executor::tasks! {
    AppTasks: Executor<Esp32s31, 0> {
        report(console: usb::Console, pads: oer_chips_esp32s31_hal::peripherals::usb_serial_jtag::UsbPads) = report;
        watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_watchdog;
    }
}
oer_chips_family_platform_executor::time_driver!(Esp32s31);
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
    if RadioTasks::session(&radio_executor, parts).is_err() {
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
    /// The first start's end.
    Up = 1,
    /// The wait for the sender: its milliseconds, the foreign units in it.
    Waited = 2,
    /// A failure: its phase and its index.
    Failed = 5,
    /// One window's counts: the case, sender units, singles, chains,
    /// mismatched units, length disagreements, foreign units, `Length`
    /// anomalies, `DoorbellPending` anomalies, spares held at the end.
    Window = 20,
    /// R2: one case's chains: its tag, chains, descriptors least and most,
    /// the expected descriptors, mismatches.
    Chains = 21,
    /// R2: one non-terminal word: its position (0 the first, 1 a middle
    /// one), its bits, how many.
    Word = 22,
    /// R3: the standing: the state (0 NEXT zero with the tail completed,
    /// 1 NEXT on the armed tail, 2 neither), LAST, NEXT (`u32::MAX` zero,
    /// `u32::MAX - 1` elsewhere), the tail, its completion, unchanged.
    Standing = 23,
    /// R4: one turn: its index, the units after its start, the spares held,
    /// the stop's and the start's microseconds.
    Turn = 24,
    /// One case's verdict: the case (1 to 4) and 1 for its pass.
    Verdict = 25,
}

const WIDTH: usize = 12;
const ROWS: usize = 8 + 4 * 2 + CASES.len() + 2 * WORDS + TURNS + 4;
/// The distinct non-terminal words kept for each position.
const WORDS: usize = 8;

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
/// The case the session is in, printed as it changes (`MARK <n>`).
static MARK: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

fn mark(phase: u32) {
    MARK.store(phase, core::sync::atomic::Ordering::Relaxed);
}

async fn report(
    mut console: usb::Console,
    _pads: oer_chips_esp32s31_hal::peripherals::usb_serial_jtag::UsbPads,
) {
    use core::fmt::Write as _;
    usb::text(&mut console, b"RXRING boot v=1\n").await;
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
    let mut line = heapless::String::<160>::new();
    for row in journal.rows.iter().take(journal.used) {
        line.clear();
        let _ = line.push_str("R");
        for value in row {
            let _ = write!(line, " {value}");
        }
        let _ = line.push('\n');
        usb::text(&mut console, line.as_bytes()).await;
    }
    let rows = || journal.rows.iter().take(journal.used);
    let failed = rows().any(|row| row.first() == Some(&(Kind::Failed as u32)));
    let passed = rows()
        .filter(|row| row.first() == Some(&(Kind::Verdict as u32)))
        .filter(|row| row.get(2) == Some(&1))
        .count();
    if !failed && passed == 4 {
        usb::text(&mut console, b"OER-RESULT: pass rx-ring\n").await;
    } else {
        usb::text(&mut console, b"OER-RESULT: fail rx-ring\n").await;
    }
}

// ---------------------------------------------------------------- counts

/// One window's counts.
#[derive(Clone, Copy, Default)]
struct Counts {
    sender: u32,
    singles: u32,
    chains: u32,
    mismatched: u32,
    lengths: u32,
    foreign: u32,
    length_anomalies: u32,
    doorbell_anomalies: u32,
}

impl Counts {
    fn anomalies(&self) -> u32 {
        self.length_anomalies
            .saturating_add(self.doorbell_anomalies)
    }

    fn record(&self, journal: &mut Journal, case: u32, spares: usize) {
        journal.record(
            Kind::Window,
            &[
                case,
                self.sender,
                self.singles,
                self.chains,
                self.mismatched,
                self.lengths,
                self.foreign,
                self.length_anomalies,
                self.doorbell_anomalies,
                spares as u32,
            ],
        );
    }
}

/// R2's chains, by case.
#[derive(Clone, Copy, Default)]
struct ChainCase {
    chains: u32,
    least: u32,
    most: u32,
    mismatched: u32,
}

/// The non-terminal words seen, by position, with how many.
struct Words {
    seen: [heapless::Vec<(u32, u32), WORDS>; 2],
    /// Words past [`WORDS`] distinct ones.
    overflow: u32,
}

impl Words {
    fn add(&mut self, position: usize, word: u32) {
        let Some(list) = self.seen.get_mut(position) else {
            return;
        };
        if let Some(entry) = list.iter_mut().find(|(bits, _)| *bits == word) {
            entry.1 = entry.1.saturating_add(1);
        } else if list.push((word, 1)).is_err() {
            self.overflow = self.overflow.saturating_add(1);
        }
    }
}

/// What a window records of the chains: the cases' and the words.
struct ChainRecord {
    cases: [ChainCase; CASES.len()],
    words: Words,
}

/// What one frame is to the probe.
enum Seen {
    /// Not from the sender to the station.
    Foreign,
    /// The sender's, of this case: whether every byte and the length
    /// agreed.
    Sender {
        case: Case,
        bytes: bool,
        length: bool,
    },
}

const RETRY: u8 = 0x08;

/// A frame of `written` octets (FCS and padding included) the walker wrote:
/// the sender's case C frame to `station`, checked by every byte the sender
/// sets and by its length, or a foreign one. The sender is learned from its
/// first frame.
fn check(frame: &[u8], written: usize, station: [u8; 6], sender: &mut Option<[u8; 6]>) -> Seen {
    let (Some(kind), Some(flags), Some(to), Some(from)) = (
        frame.first().copied(),
        frame.get(1).copied(),
        frame.get(4..10),
        frame.get(10..16),
    ) else {
        return Seen::Foreign;
    };
    let (Ok(to), Ok(from)) = (<[u8; 6]>::try_from(to), <[u8; 6]>::try_from(from)) else {
        return Seen::Foreign;
    };
    if to != station || flags & !RETRY != 0 || !matches!(kind, 0x88 | 0x08) {
        return Seen::Foreign;
    }
    let learned = *sender.get_or_insert(from);
    if from != learned {
        return Seen::Foreign;
    }
    let probe = if kind == 0x88 {
        Case::Amsdu2
    } else {
        Case::Provenance700
    };
    let counter = frame
        .get(probe.counter_at()..probe.counter_at() + 4)
        .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
        .map(u32::from_le_bytes);
    let Some((case, counter)) =
        counter.and_then(|counter| Some((Case::from_counter(counter)?, counter)))
    else {
        return Seen::Foreign;
    };
    let length = case.length();
    // The bytes after the header are the MPDU without its FCS, rounded up to
    // a word. SOURCE: `HW_RX_CHAIN_LAPS_20261117` in
    // chips/esp32s31/registers/evidence/hw-rx-radio.toml.
    let length_agrees = (FLOOR + length).next_multiple_of(4) == FLOOR + written;
    let bytes_agree = (0..length).all(|offset| {
        // Duration and Sequence Control are the MAC's; so is the Retry flag.
        if matches!(offset, 2 | 3 | 22 | 23) {
            return true;
        }
        let expected = byte(case, learned, station, counter, offset);
        let seen = frame.get(offset).copied().unwrap_or(!expected);
        if offset == 1 {
            seen & !RETRY == expected & !RETRY
        } else {
            seen == expected
        }
    });
    Seen::Sender {
        case,
        bytes: bytes_agree,
        length: length_agrees,
    }
}

/// The descriptors a frame of `case` takes: its header and the MPDU without
/// its FCS, padded to a word, in buffers of [`SIZE`]
/// (`HW_RX_CHAIN_LAPS_20261117`).
const fn expected_descriptors(case: Case) -> usize {
    (FLOOR + case.length()).next_multiple_of(4).div_ceil(SIZE)
}

/// What a window serves with.
struct Service<'a> {
    ring: &'a mut oer_portable_memory::Occupied<'static, Ring>,
    arena: Arena<RingRx, BYTES>,
    spares: &'a mut heapless::Vec<Handle<RingRx>, SPARES>,
    station: [u8; 6],
    sender: &'a mut Option<[u8; 6]>,
}

/// Serve the ring for `millis`: every single lent for a spare (copied in
/// place when none is held), every chain copied in place, each checked.
async fn window(
    service: &mut Service<'_>,
    millis: u32,
    mut chains: Option<&mut ChainRecord>,
) -> Result<Counts, Failure> {
    let end = EmbassyClock
        .now()
        .checked_add(Duration::from_millis(millis))
        .ok_or((phase::RECEIVE, 0))?;
    let mut counts = Counts::default();
    while EmbassyClock.now() < end {
        let Service {
            ring,
            arena,
            spares,
            station,
            sender,
        } = service;
        let arena = *arena;
        let station = *station;
        let served = ring.serve(&EmbassyClock, |view| {
            let tally = |seen: Seen, counts: &mut Counts| match seen {
                Seen::Foreign => counts.foreign = counts.foreign.saturating_add(1),
                Seen::Sender { bytes, length, .. } => {
                    counts.sender = counts.sender.saturating_add(1);
                    counts.mismatched = counts.mismatched.saturating_add(u32::from(!bytes));
                    counts.lengths = counts.lengths.saturating_add(u32::from(!length));
                }
            };
            while let Some(unit) = view.next_unit() {
                match unit {
                    Unit::Single(single) => {
                        counts.singles = counts.singles.saturating_add(1);
                        let written = single.written().saturating_sub(FLOOR);
                        match spares.pop() {
                            Some(spare) => {
                                let handle = single.lend(spare);
                                let seen = check(arena.bytes(&handle), written, station, sender);
                                tally(seen, &mut counts);
                                // Its spare's place: every buffer goes back.
                                let _ = spares.push(handle);
                            }
                            None => {
                                let mut copy = [0_u8; COPY];
                                let length = single.copy_in_place(|frame| frame.copy_to(&mut copy));
                                let seen = check(
                                    copy.get(..length).unwrap_or(&[]),
                                    written,
                                    station,
                                    sender,
                                );
                                tally(seen, &mut counts);
                            }
                        }
                    }
                    Unit::Chain(chain) => {
                        counts.chains = counts.chains.saturating_add(1);
                        let count = chain.count() as u32;
                        if let Some(record) = chains.as_deref_mut() {
                            for (position, word) in chain.continuation_words().enumerate() {
                                record.words.add(usize::from(position != 0), word.bits());
                            }
                        }
                        let mut copy = [0_u8; COPY];
                        let (copied, written) =
                            chain.copy_in_place(|frame| (frame.copy_to(&mut copy), frame.len()));
                        let seen =
                            check(copy.get(..copied).unwrap_or(&[]), written, station, sender);
                        if let (
                            Seen::Sender {
                                case,
                                bytes,
                                length,
                            },
                            Some(record),
                        ) = (&seen, chains.as_deref_mut())
                            && let Some(at) = CASES.iter().position(|each| each == case)
                            && let Some(per) = record.cases.get_mut(at)
                        {
                            per.chains = per.chains.saturating_add(1);
                            per.least = if per.chains == 1 {
                                count
                            } else {
                                per.least.min(count)
                            };
                            per.most = per.most.max(count);
                            let descriptors = expected_descriptors(*case) as u32;
                            per.mismatched = per.mismatched.saturating_add(u32::from(
                                !bytes || !length || count != descriptors,
                            ));
                        }
                        tally(seen, &mut counts);
                    }
                }
            }
        });
        if let Err((anomaly, ())) = served {
            match anomaly {
                RingAnomaly::Length { .. } => {
                    counts.length_anomalies = counts.length_anomalies.saturating_add(1);
                }
                RingAnomaly::DoorbellPending => {
                    counts.doorbell_anomalies = counts.doorbell_anomalies.saturating_add(1);
                }
            }
        }
        wait_micros(200).await;
    }
    Ok(counts)
}

/// Wait `micros` on the executor's clock.
async fn wait_micros(micros: u64) {
    if let Some(at) = EmbassyClock
        .now()
        .checked_add(Duration::from_micros(micros))
    {
        EmbassyClock.wait_until(at).await;
    }
}

/// The microseconds since `since`.
fn micros_since(since: Instant) -> u32 {
    u32::try_from(
        EmbassyClock
            .now()
            .saturating_duration_since(since)
            .as_micros(),
    )
    .unwrap_or(u32::MAX)
}

// ---------------------------------------------------------------- session

type SessionRun = impl core::future::Future<Output = ()>;
static SESSION: static_cell::ConstStaticCell<oer_portable_memory::StaticPlace<SessionRun>> =
    static_cell::ConstStaticCell::new(oer_portable_memory::StaticPlace::new());
#[define_opaque(SessionRun)]
fn session_future(parts: SessionParts) -> SessionRun {
    async move {
        let Some(journal) = JOURNAL.try_take() else {
            return;
        };
        journal.used = 0;
        if let Err((phase, index)) = apart!(session(parts, journal)) {
            journal.record(Kind::Failed, &[phase, index]);
        }
        let _ = DONE.install(journal);
    }
}

async fn session_task(parts: SessionParts) {
    let Some(place) = SESSION.try_take() else {
        return;
    };
    place.future(|| session_future(parts)).await;
}

/// The first `N` items of `items`, or `None` when it has fewer.
fn first_of<T, const N: usize>(items: impl Iterator<Item = T>) -> Option<[T; N]> {
    let mut collected = heapless::Vec::<T, N>::new();
    for item in items.take(N) {
        collected.push(item).ok()?;
    }
    collected.into_array().ok()
}

/// The owners a turn passes through: the radio, the running MAC with its
/// clocks' proof, the walker's interrupt banks and the cold start's inputs.
struct Turns<'a> {
    slot: &'a mut Option<Radio>,
    access: Option<(RadioAccess, WifiClocksOn)>,
    mac_irq: &'a mut MacInterruptRegisters,
    power_irq: &'a mut MacPowerInterruptRegisters,
    sources: &'a mut crate::radio::Sources,
    mac: MacColdStartConfig,
    channel: Ghz2_4Channel,
}

/// Bring Wi-Fi up, start the ring, then the four cases. Every owner stays
/// held until the leased host reset.
async fn session(parts: SessionParts, journal: &mut Journal) -> Result<(), Failure> {
    let timer = CooperativeTimer::new(EmbassyClock);
    let mut platform = ();
    let brought = apart!(bring_up(parts, &timer, &mut platform))?;
    let table = RX_TABLE.take().ok_or((phase::RING, 0))?;
    let table = RxTable::place::<ChipDma>(table).map_err(|_| (phase::RING, 1))?;
    let (arena, mut fresh) = Arena::<RingRx, BYTES>::claim().ok_or((phase::RING, 2))?;
    let ring: [Handle<RingRx>; RING] = first_of(&mut fresh).ok_or((phase::RING, 3))?;
    let reserved = fresh.next().ok_or((phase::RING, 4))?;
    let mut spares = heapless::Vec::<Handle<RingRx>, SPARES>::new();
    for spare in fresh {
        spares.push(spare).map_err(|_| (phase::RING, 5))?;
    }
    if spares.len() != SPARES {
        return Err((phase::RING, 5));
    }
    let place = RING_PLACE
        .try_init(MaybeUninit::uninit())
        .ok_or((phase::RING, 6))?;
    let Brought {
        mut slot,
        mut clocked,
        first,
        mut walker,
        mut sources,
        mac,
        channel,
        mut mac_irq,
        mut power_irq,
        station,
        held: _held,
    } = brought;
    let reset =
        held(&mut slot)?.reset_wifi_mac_block(clocked.clocks_on(), first, crate::radio::CBW);
    let prepared = apart!(prepare(
        &mut slot,
        &mut clocked,
        &mut walker,
        &mut sources,
        reset,
        mac,
        channel,
        &timer,
        &mut platform,
    ))?;
    let (mut access, clocks_on) = clocked.into_running();
    let parts = RxRingParts {
        table,
        walker,
        arena,
        ring,
        reserved,
    };
    let (mut ring, mut enabled) = begin(
        &mut access,
        &mut mac_irq,
        &mut power_irq,
        prepared,
        place,
        parts,
    );
    journal.record(Kind::Up, &[]);
    let mut sender = None;

    // The sender starts once this probe receives (`MARK 6`); the cases
    // begin with its first unit.
    mark(6);
    let mut waited = 0_u32;
    let mut before = Counts::default();
    while before.sender == 0 {
        if waited >= WAIT_MS {
            journal.record(Kind::Waited, &[waited, before.foreign]);
            return Err((phase::RECEIVE, 1));
        }
        let mut service = Service {
            ring: &mut ring,
            arena,
            spares: &mut spares,
            station,
            sender: &mut sender,
        };
        let counts = window(&mut service, WAIT_STEP_MS, None).await?;
        before.sender = counts.sender;
        before.foreign = before.foreign.saturating_add(counts.foreign);
        waited = waited.saturating_add(WAIT_STEP_MS);
    }
    journal.record(Kind::Waited, &[waited, before.foreign]);

    // R1 and R2: one window, singles lent and chains copied.
    mark(1);
    let mut record = ChainRecord {
        cases: [ChainCase::default(); CASES.len()],
        words: Words {
            seen: [heapless::Vec::new(), heapless::Vec::new()],
            overflow: 0,
        },
    };
    let counts = {
        let mut service = Service {
            ring: &mut ring,
            arena,
            spares: &mut spares,
            station,
            sender: &mut sender,
        };
        window(&mut service, WINDOW_MS, Some(&mut record)).await?
    };
    counts.record(journal, 1, spares.len());
    for (case, per) in CASES.iter().zip(&record.cases) {
        journal.record(
            Kind::Chains,
            &[
                case.tag(),
                per.chains,
                per.least,
                per.most,
                expected_descriptors(*case) as u32,
                per.mismatched,
            ],
        );
    }
    for (position, list) in record.words.seen.iter().enumerate() {
        for (bits, seen) in list {
            journal.record(Kind::Word, &[position as u32, *bits, *seen]);
        }
    }
    let r1 = counts.singles > 0
        && counts.sender >= R1_UNITS
        && counts.mismatched == 0
        && counts.lengths == 0
        && counts.anomalies() == 0
        && spares.len() == SPARES;
    journal.record(Kind::Verdict, &[1, u32::from(r1)]);
    let long = [Case::Provenance700, Case::Provenance1500];
    let r2 = long.iter().all(|case| {
        CASES
            .iter()
            .position(|each| each == case)
            .and_then(|at| record.cases.get(at))
            .is_some_and(|per| per.chains >= R2_CHAINS && per.mismatched == 0)
    }) && counts.anomalies() == 0;
    journal.record(Kind::Verdict, &[2, u32::from(r2)]);

    // R3: the stall, the walker's standing, then the exit by serve.
    mark(3);
    wait_micros(u64::from(STALL_MS) * 1_000).await;
    let standing = ring.standing();
    let state = match (standing.next, standing.tail_done) {
        (NextAt::Zero, true) => 0,
        (NextAt::Ring(at), false) if at == standing.tail => 1,
        _ => 2,
    };
    journal.record(
        Kind::Standing,
        &[
            state,
            standing.last.map_or(u32::MAX, |last| last as u32),
            match standing.next {
                NextAt::Zero => u32::MAX,
                NextAt::Elsewhere => u32::MAX - 1,
                NextAt::Ring(at) => at as u32,
            },
            standing.tail as u32,
            u32::from(standing.tail_done),
            u32::from(standing.unchanged),
        ],
    );
    let counts = {
        let mut service = Service {
            ring: &mut ring,
            arena,
            spares: &mut spares,
            station,
            sender: &mut sender,
        };
        window(&mut service, EXIT_MS, None).await?
    };
    counts.record(journal, 3, spares.len());
    let r3 = standing.unchanged
        && state != 2
        && counts.sender >= R3_UNITS
        && counts.mismatched == 0
        && counts.anomalies() == 0;
    journal.record(Kind::Verdict, &[3, u32::from(r3)]);

    // R4: the turns.
    mark(4);
    let mut turns = Turns {
        slot: &mut slot,
        access: Some((access, clocks_on)),
        mac_irq: &mut mac_irq,
        power_irq: &mut power_irq,
        sources: &mut sources,
        mac,
        channel,
    };
    let mut r4 = true;
    let mut totals = Counts::default();
    for turn in 0..TURNS {
        let before = {
            let mut service = Service {
                ring: &mut ring,
                arena,
                spares: &mut spares,
                station,
                sender: &mut sender,
            };
            window(&mut service, TURN_MS, None).await?
        };
        // T1 to T4.
        let (mut access, clocks_on) = turns.access.take().ok_or((phase::RING, 0x40))?;
        let began = EmbassyClock.now();
        let (reset, parts, place) = stop(held(turns.slot)?, &mut access, &clocks_on, ring, enabled);
        let stopped = micros_since(began);
        let handles = parts.ring.len() + 1 + spares.len();
        // T5: the next start from the cold preparation.
        let began = EmbassyClock.now();
        let mut clocked = WifiClocked::from_running(access, clocks_on);
        let mut walker = parts.walker;
        let prepared = apart!(prepare(
            turns.slot,
            &mut clocked,
            &mut walker,
            turns.sources,
            reset,
            turns.mac,
            turns.channel,
            &timer,
            &mut platform,
        ))?;
        let (mut access, clocks_on) = clocked.into_running();
        let parts = Parts { walker, ..parts };
        (ring, enabled) = begin(
            &mut access,
            turns.mac_irq,
            turns.power_irq,
            prepared,
            place,
            parts,
        );
        let started = micros_since(began);
        turns.access = Some((access, clocks_on));
        let after = {
            let mut service = Service {
                ring: &mut ring,
                arena,
                spares: &mut spares,
                station,
                sender: &mut sender,
            };
            window(&mut service, TURN_MS, None).await?
        };
        journal.record(
            Kind::Turn,
            &[
                turn as u32,
                after.sender,
                handles as u32,
                stopped,
                started,
                before.sender,
                after.mismatched,
                after
                    .length_anomalies
                    .saturating_add(before.length_anomalies),
                after
                    .doorbell_anomalies
                    .saturating_add(before.doorbell_anomalies),
            ],
        );
        r4 &= after.sender >= R4_UNITS
            && before.sender > 0
            && handles == BUFFERS
            && after.mismatched == 0
            && before.anomalies() == 0
            && after.anomalies() == 0;
        totals.sender = totals.sender.saturating_add(after.sender);
    }
    journal.record(Kind::Verdict, &[4, u32::from(r4)]);
    mark(5);
    let _held = (ring, turns.access, _held);
    Ok(())
}
