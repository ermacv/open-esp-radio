//! The receive ring's probe: Wi-Fi brought up over the radio domain the
//! probe holds (`radio`), the ring started by the path table, then each
//! step the host's program (`hil/scenarios/radio-pair`) asks for over the
//! HIL protocol, over case C's frames from the sender
//! (`esp32s31-case-c-sender`), from which the host builds its four cases:
//!
//! - R1, singles: each lent for a spare, checked by its payload;
//! - R2, chains: copied in place, checked by payload and length, their
//!   continuations' control words recorded by position;
//! - R3, a stall: no serve for 100 ms, the walker's standing read, then the
//!   exit by `serve`;
//! - R4, turns: the stop in its order ending in the release, every handle
//!   back, then a cold start and receive again.
//!
//! Every window counts the sender's units and the ring's anomalies by kind;
//! the thresholds and the verdict are the host's.
use crate::case_c::{CASES, Case, byte, word_padded};
use crate::pair;
use crate::radio::{
    Brought, Cooperative, CooperativeTimer, Failure, Radio, SessionParts, apart, begin, bring_up,
    held as held_radio, phase, prepare, stop,
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
            receive::{RingAnomaly, RxDescriptors, RxRing, RxRingParts, RxTable, Unit},
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
use oer_hil_agent::answer;
use oer_hil_services_radio_pair::{self as service, ReceiverRequest, Refusal};
use oer_portable_diagnostics::{Name, name};
use oer_portable_embassy_time::EmbassyClock;
use oer_portable_memory::arena::{Arena, Handle};
use oer_portable_time::{Clock, Duration, Instant, Timer};
use oer_protocols_hil_protocol::RequestIdentity;
use oer_protocols_ieee80211_wire::channel::Ghz2_4Channel;

/// The ring: sixteen descriptors; the arena's other sixteen buffers
/// are the spares a lent unit is exchanged for. Each buffer is 256 octets
/// with the 64-octet header below its floor: case C's 700- and 1 500-octet
/// frames are chains of three and of seven descriptors.
const RING: usize = 16;
const SIZE: usize = 256;
const FLOOR: usize = 64;
const BUFFERS: usize = 32;
const SPARES: usize = BUFFERS - RING;
const BYTES: usize = BUFFERS * SIZE;
/// The longest frame copied in place, its FCS and padding included.
const COPY: usize = 1_600;

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
        session(parts: SessionParts, inbox: oer_hil_agent::Inbox<'static, ReceiverRequest>, reset: u32) = session_task;
    }
}
oer_chips_family_platform_executor::tasks! {
    AppTasks: Executor<Esp32s31, 0> {
        watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_watchdog;
        host(host: Host) = host_task;
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
            reset_cause,
            runtime_efuse,
            runtime_radio,
            runtime_timg0_alarm,
            runtime_clocks,
            runtime_system_timer,
            runtime_wifi_interrupts,
            runtime_software_interrupts,
            runtime_entropy,
            runtime_watchdogs,
            runtime_cycles,
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
            let reset_causes =
                oer_chips_esp32s31_platform_runtime::reset_causes::read(hardware.reset_cause);
            crate::diagnostic::report_previous();
            (
                reset_causes.of(oer_chips_esp32s31_hal::system::platform::BOOT_HART),
                hardware.efuse,
                hardware.radio,
                hardware.timg0_alarm,
                hardware.clocks,
                hardware.system_timer,
                hardware.wifi_interrupts,
                hardware.software_interrupts,
                hardware.entropy,
                hardware.watchdogs,
                hardware.cycles,
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
        oer_chips_esp32s31_hal::peripherals::entropy::Entropy::new(runtime_entropy, runtime_cycles);
    let (seed, boot) = match (rng.random(), rng.random(), rng.random()) {
        (Ok(seed), Ok(high), Ok(low)) => (seed, (u64::from(high) << 32) | u64::from(low)),
        _ => fail(name!("entropy"; 32)),
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
        cycles: runtime_cycles,
    };
    let (console, pads) = match usb::open(adopted.router(), interrupts.console_usb) {
        Ok(value) => value,
        Err(error) => fail(error.reason()),
    };
    // The probe reads no previous boot's panic or exception record: its
    // diagnostic hook keeps its own (`diagnostic::report_previous`).
    // The pads stay held by `main`, which never returns, so that nothing
    // switches them off.
    let _pads = pads;
    let Some(mailbox) = MAILBOX.try_take() else {
        fail(name!("mailbox"; 32));
    };
    let (outbox, inbox) = mailbox.split();
    let host = Host {
        console,
        boot,
        records: oer_hil_agent::Records {
            reset_cause,
            previous_panic: None,
            previous_exception: None,
        },
        outbox,
    };
    if RadioTasks::session(&radio_executor, parts, inbox, u32::from(reset_cause.code())).is_err() {
        fail(name!("session-task"; 32));
    }
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
                if AppTasks::watchdog(spawner, runtime_watchdogs.rtc).is_err()
                    || AppTasks::host(spawner, host).is_err()
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
    patterns: u32,
    pattern_written: u32,
}

impl Counts {
    /// The counts in the service's words.
    const fn to_service(self) -> service::Counts {
        service::Counts {
            sender: self.sender,
            singles: self.singles,
            chains: self.chains,
            mismatched: self.mismatched,
            lengths: self.lengths,
            foreign: self.foreign,
            length_anomalies: self.length_anomalies,
            doorbell_anomalies: self.doorbell_anomalies,
            patterns: self.patterns,
            pattern_written: self.pattern_written,
        }
    }
}

/// The distinct non-terminal words kept for each position.
const WORDS: usize = service::WORDS;

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
    /// The sender's `single` frame: every body byte its offset's; its bytes
    /// after the header as written.
    Pattern { written: usize },
}

const RETRY: u8 = 0x08;

/// A frame of `written` octets (FCS and padding included) the walker wrote:
/// the sender's case C frame to `station`, checked by every byte the sender
/// sets and by its length, or a foreign one. The sender is the one the host
/// named ([`service::Expect`]).
fn check(frame: &[u8], written: usize, station: [u8; 6], sender: [u8; 6]) -> Seen {
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
    if from != sender {
        return Seen::Foreign;
    }
    // The sender's `single` frame before case C's: its body bytes may read
    // as a counter of some case.
    if let Seen::Pattern { written } = pattern(frame, written, kind) {
        return Seen::Pattern { written };
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
    let length_agrees = word_padded(FLOOR + length) == FLOOR + written;
    let bytes_agree = (0..length).all(|offset| {
        // Duration and Sequence Control are the MAC's; so is the Retry flag.
        if matches!(offset, 2 | 3 | 22 | 23) {
            return true;
        }
        let expected = byte(case, sender, station, counter, offset);
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

/// The sender's `single` frame, or a foreign one: a non-QoS Data frame whose
/// body bytes are each their offset's, checked up to the written bytes'
/// last word, which may be padding.
fn pattern(frame: &[u8], written: usize, kind: u8) -> Seen {
    const BODY: usize = 24;
    let checked = written.saturating_sub(3);
    let body = (BODY..checked).all(|offset| frame.get(offset) == Some(&(offset as u8)));
    if kind == 0x08 && checked > BODY && body {
        Seen::Pattern { written }
    } else {
        Seen::Foreign
    }
}

/// The descriptors a frame of `case` takes: its header and the MPDU without
/// its FCS, padded to a word, in buffers of [`SIZE`]
/// (`HW_RX_CHAIN_LAPS_20261117`).
const fn expected_descriptors(case: Case) -> usize {
    // `SIZE` is a constant above zero: the division cannot fail.
    (word_padded(FLOOR + case.length()) + (SIZE - 1)) / SIZE
}

/// What a window serves with.
struct Service<'a> {
    ring: &'a mut oer_portable_memory::Occupied<'static, Ring>,
    arena: Arena<RingRx, BYTES>,
    spares: &'a mut heapless::Vec<Handle<RingRx>, SPARES>,
    station: [u8; 6],
    sender: [u8; 6],
}

/// Serve the ring for `millis`: every single lent for a spare (copied in
/// place when none is held), every chain copied in place, each checked.
async fn window(
    service: &mut Service<'_>,
    cycles: oer_chips_esp32s31_hal::system::cycles::CycleCounter,
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
        let sender = *sender;
        let served = ring.serve(&cycles, |view| {
            let tally = |seen: Seen, counts: &mut Counts| match seen {
                Seen::Foreign => counts.foreign = counts.foreign.saturating_add(1),
                Seen::Pattern { written } => {
                    counts.patterns = counts.patterns.saturating_add(1);
                    counts.pattern_written = u32::try_from(written).unwrap_or(u32::MAX);
                }
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

// ---------------------------------------------------------------- host

oer_protocols_hil_agent::image_keys! {
    /// The receiver: the base module and the receiver's requests.
    KEYS = [ReceiverRequest]
}

/// What the receiver announces in its Hello.
const IMAGE: oer_hil_agent::Image = oer_hil_agent::Image {
    keys: KEYS,
    maximum_payload_bytes: service::MAXIMUM_PAYLOAD_BYTES,
};

static MAILBOX: static_cell::ConstStaticCell<oer_hil_agent::Mailbox<ReceiverRequest>> =
    static_cell::ConstStaticCell::new(oer_hil_agent::Mailbox::new());

/// What the host's link takes at boot: the console, this boot's identity,
/// what the adoption read and the mailbox's console end.
pub(crate) struct Host {
    pub(crate) console: usb::Console,
    pub(crate) boot: u64,
    pub(crate) records: oer_hil_agent::Records,
    pub(crate) outbox: oer_hil_agent::Outbox<'static, ReceiverRequest>,
}

/// How the receiver's serving ends: the console's transport broke.
enum Ended {
    Console,
}

/// The application level's task: the HIL console, which hands each request
/// to the radio level's worker through the mailbox and writes the worker's
/// answers. Its own worker never ends: the receiver's work is the radio
/// level's.
async fn host_task(host: Host) {
    let Host {
        console,
        boot,
        records,
        outbox,
    } = host;
    match oer_hil_agent::serve(
        console,
        boot,
        records,
        IMAGE,
        outbox,
        Ended::Console,
        core::future::pending::<Result<core::convert::Infallible, Ended>>,
    )
    .await
    {
        Err(Ended::Console) => fail(name!("console"; 32)),
        Ok(never) => match never {},
    }
}

type SessionRun = impl core::future::Future<Output = ()>;
static SESSION: static_cell::ConstStaticCell<oer_portable_memory::StaticPlace<SessionRun>> =
    static_cell::ConstStaticCell::new(oer_portable_memory::StaticPlace::new());

/// The radio level's one task: the worker that drives the radio, each step
/// as the host asks for it.
#[define_opaque(SessionRun)]
fn session_future(
    parts: SessionParts,
    inbox: oer_hil_agent::Inbox<'static, ReceiverRequest>,
    reset: u32,
) -> SessionRun {
    async move {
        match worker(parts, inbox, reset).await {
            Ok(never) => match never {},
            Err(Ended::Console) => fail(name!("console"; 32)),
        }
    }
}

async fn session_task(
    parts: SessionParts,
    inbox: oer_hil_agent::Inbox<'static, ReceiverRequest>,
    reset: u32,
) {
    let Some(place) = SESSION.try_take() else {
        fail(name!("session-place"; 32));
    };
    place.future(|| session_future(parts, inbox, reset)).await;
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

/// Answer `request` with its endpoint's refusal.
fn refuse(identity: RequestIdentity, request: ReceiverRequest, refusal: Refusal) {
    match request {
        ReceiverRequest::SnapshotCold(_) | ReceiverRequest::SnapshotUp(_) => {
            answer(identity, &service::SnapshotPage::Refused(refusal));
        }
        ReceiverRequest::BringUp(_) => answer(identity, &service::BroughtUp::Refused(refusal)),
        ReceiverRequest::ReceiveCounters(_) => {
            answer(identity, &service::Counters::Refused(refusal));
        }
        ReceiverRequest::Stop(_) => answer(identity, &service::Stopped::Refused(refusal)),
        ReceiverRequest::Expect(_) => answer(identity, &service::Expecting::Refused(refusal)),
        ReceiverRequest::Window(_) => answer(identity, &service::Windowed::Refused(refusal)),
        ReceiverRequest::Stall(_) => answer(identity, &service::Stood::Refused(refusal)),
        ReceiverRequest::Turn(_) => answer(identity, &service::Turned::Refused(refusal)),
    }
}

/// After the bring-up's failure or the stop: the cold snapshot still answers, every
/// other step is refused, until the next boot.
async fn down(
    mut inbox: oer_hil_agent::Inbox<'static, ReceiverRequest>,
    cold: &pair::Readings,
) -> Result<core::convert::Infallible, Ended> {
    loop {
        let (identity, request) = inbox.next().await;
        match request {
            ReceiverRequest::SnapshotCold(request) => {
                answer(identity, &pair::page(cold, request.page));
            }
            other => refuse(identity, other, Refusal::Down),
        }
    }
}

/// The receiver's worker: each step the host asks for, in its order.
async fn worker(
    parts: SessionParts,
    mut inbox: oer_hil_agent::Inbox<'static, ReceiverRequest>,
    reset: u32,
) -> Result<core::convert::Infallible, Ended> {
    let cold = pair::cold(reset, &parts.radio, &parts.clocks);
    let identity = parts.identity;
    let cycles = parts.cycles;
    let ap = parts.ap;
    let bring = loop {
        let (id, request) = inbox.next().await;
        match request {
            ReceiverRequest::SnapshotCold(request) => {
                answer(id, &pair::page(&cold, request.page));
            }
            ReceiverRequest::BringUp(_) => break id,
            other => refuse(id, other, Refusal::NotUp),
        }
    };
    let cooperative = Cooperative::new(EmbassyClock);
    let timer = CooperativeTimer::new(&cooperative, cycles);
    let mut platform = ();
    let up = async {
        let brought = apart!(bring_up(parts, &timer, &mut platform))?;
        let table = RX_TABLE.take().ok_or((phase::RING, 0))?;
        let table = RxTable::place::<ChipDma>(table).map_err(|_| (phase::RING, 1))?;
        let (arena, mut fresh) = Arena::<RingRx, BYTES>::claim().ok_or((phase::RING, 2))?;
        let ring: [Handle<RingRx>; RING] = first_of(&mut fresh).ok_or((phase::RING, 3))?;
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
            held,
        } = brought;
        let reset = held_radio(&mut slot)?.reset_wifi_mac_block(
            clocked.clocks_on(),
            first,
            crate::radio::CBW,
        );
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
        };
        let (ring, enabled) = begin(
            &mut access,
            &mut mac_irq,
            &mut power_irq,
            prepared,
            place,
            parts,
        );
        Ok((
            slot, access, clocks_on, ring, enabled, arena, spares, sources, mac, channel, mac_irq,
            power_irq, station, held,
        ))
    };
    let (
        mut slot,
        access,
        clocks_on,
        ring,
        enabled,
        arena,
        mut spares,
        mut sources,
        mac,
        channel,
        mut mac_irq,
        mut power_irq,
        station,
        held,
    ) = match up.await {
        Ok(up) => up,
        Err((phase, index)) => {
            answer(
                bring,
                &service::BroughtUp::Failed(service::Failure { phase, index }),
            );
            return down(inbox, &cold).await;
        }
    };
    answer(bring, &service::BroughtUp::Up { station, ap });
    let up = match pair::up(&mut slot, held.clocks(), reset, identity) {
        Ok(readings) => readings,
        Err(_) => cold.clone(),
    };
    let mut turns = Turns {
        slot: &mut slot,
        access: Some((access, clocks_on)),
        mac_irq: &mut mac_irq,
        power_irq: &mut power_irq,
        sources: &mut sources,
        mac,
        channel,
    };
    let mut sender = None;
    let mut receiving = Receiving::Up {
        ring,
        enabled,
        steps: Steps::Open,
    };
    loop {
        let (id, request) = inbox.next().await;
        match request {
            ReceiverRequest::SnapshotCold(request) => answer(id, &pair::page(&cold, request.page)),
            ReceiverRequest::SnapshotUp(request) => answer(id, &pair::page(&up, request.page)),
            ReceiverRequest::BringUp(_) => {
                answer(id, &service::BroughtUp::Refused(Refusal::AlreadyUp));
            }
            ReceiverRequest::ReceiveCounters(_) => match (&mut receiving, turns.access.as_ref()) {
                (Receiving::Up { ring, .. }, Some((access, _))) => answer(
                    id,
                    &pair::counters(
                        access,
                        Some(pair::ring_state(
                            ring.standing(),
                            ring.descriptor_controls(),
                        )),
                    ),
                ),
                (Receiving::Stopped, Some((access, _))) => {
                    answer(id, &pair::counters(access, None));
                }
                (_, None) => answer(id, &service::Counters::Refused(Refusal::Down)),
            },
            ReceiverRequest::Stop(_) => {
                let (Receiving::Up { ring, enabled, .. }, Some((mut access, clocks_on))) = (
                    core::mem::replace(&mut receiving, Receiving::Stopped),
                    turns.access.take(),
                ) else {
                    answer(id, &service::Stopped::Refused(Refusal::Down));
                    return down(inbox, &cold).await;
                };
                match held_radio(turns.slot) {
                    Ok(radio) => {
                        let began = EmbassyClock.now();
                        let _stopped = stop(radio, &mut access, &clocks_on, ring, enabled, cycles);
                        answer(
                            id,
                            &service::Stopped::Stopped {
                                micros: micros_since(began),
                            },
                        );
                    }
                    Err((phase, index)) => {
                        answer(
                            id,
                            &service::Stopped::Failed(service::Failure { phase, index }),
                        );
                    }
                }
                return down(inbox, &cold).await;
            }
            ReceiverRequest::Turn(request) => {
                let Some(sender) = sender else {
                    answer(id, &service::Turned::Refused(Refusal::NoSender));
                    continue;
                };
                let (mut ring, enabled) =
                    match core::mem::replace(&mut receiving, Receiving::Stopped) {
                        Receiving::Up {
                            ring,
                            enabled,
                            steps: Steps::Open,
                        } => (ring, enabled),
                        held => {
                            let refusal = match held {
                                Receiving::Up { .. } => Refusal::StepFailed,
                                Receiving::Stopped => Refusal::Down,
                            };
                            receiving = held;
                            answer(id, &service::Turned::Refused(refusal));
                            continue;
                        }
                    };
                // A step that fails with the ring up keeps it, failed; one
                // that fails after the stop leaves the radio stopped.
                macro_rules! failed {
                    ($state:expr, $phase:expr, $index:expr) => {{
                        receiving = $state;
                        answer(
                            id,
                            &service::Turned::Failed(service::Failure {
                                phase: $phase,
                                index: $index,
                            }),
                        );
                        continue;
                    }};
                }
                let before = {
                    let mut service = Service {
                        ring: &mut ring,
                        arena,
                        spares: &mut spares,
                        station,
                        sender,
                    };
                    window(&mut service, cycles, request.millis, None).await
                };
                let before = match before {
                    Ok(before) => before,
                    Err((phase, index)) => failed!(
                        Receiving::Up {
                            ring,
                            enabled,
                            steps: Steps::Failed,
                        },
                        phase,
                        index
                    ),
                };
                // T1 to T4.
                let Some((mut access, clocks_on)) = turns.access.take() else {
                    failed!(
                        Receiving::Up {
                            ring,
                            enabled,
                            steps: Steps::Failed,
                        },
                        phase::RING,
                        0x40
                    )
                };
                let radio = match held_radio(turns.slot) {
                    Ok(radio) => radio,
                    Err((phase, index)) => {
                        turns.access = Some((access, clocks_on));
                        failed!(
                            Receiving::Up {
                                ring,
                                enabled,
                                steps: Steps::Failed,
                            },
                            phase,
                            index
                        )
                    }
                };
                let began = EmbassyClock.now();
                let (reset, parts, place) =
                    stop(radio, &mut access, &clocks_on, ring, enabled, cycles);
                let stopped = micros_since(began);
                let handles = parts.ring.len() + spares.len();
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
                ));
                let prepared = match prepared {
                    Ok(prepared) => prepared,
                    Err((phase, index)) => {
                        turns.access = Some(clocked.into_running());
                        failed!(Receiving::Stopped, phase, index)
                    }
                };
                let (mut access, clocks_on) = clocked.into_running();
                let parts = Parts { walker, ..parts };
                let (mut ring, enabled) = begin(
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
                        sender,
                    };
                    window(&mut service, cycles, request.millis, None).await
                };
                let after = match after {
                    Ok(after) => after,
                    Err((phase, index)) => failed!(
                        Receiving::Up {
                            ring,
                            enabled,
                            steps: Steps::Failed,
                        },
                        phase,
                        index
                    ),
                };
                receiving = Receiving::Up {
                    ring,
                    enabled,
                    steps: Steps::Open,
                };
                answer(
                    id,
                    &service::Turned::Turned {
                        before: before.to_service(),
                        after: after.to_service(),
                        handles: u8::try_from(handles).unwrap_or(u8::MAX),
                        stop_micros: stopped,
                        start_micros: started,
                    },
                );
            }
            other => {
                // The other steps run only while the ring is up and no step
                // failed.
                let ring = match &mut receiving {
                    Receiving::Up {
                        ring,
                        steps: Steps::Open,
                        ..
                    } => ring,
                    Receiving::Up { .. } => {
                        refuse(id, other, Refusal::StepFailed);
                        continue;
                    }
                    Receiving::Stopped => {
                        refuse(id, other, Refusal::Down);
                        continue;
                    }
                };
                match other {
                    ReceiverRequest::Expect(expect) => {
                        sender = Some(expect.sender);
                        answer(id, &service::Expecting::Expecting);
                    }
                    ReceiverRequest::Window(request) => {
                        let Some(sender) = sender else {
                            answer(id, &service::Windowed::Refused(Refusal::NoSender));
                            continue;
                        };
                        let mut record = ChainRecord {
                            cases: [ChainCase::default(); CASES.len()],
                            words: Words {
                                seen: [heapless::Vec::new(), heapless::Vec::new()],
                                overflow: 0,
                            },
                        };
                        let mut service = Service {
                            ring,
                            arena,
                            spares: &mut spares,
                            station,
                            sender,
                        };
                        let chains = match request.record {
                            service::Record::Chains => Some(&mut record),
                            service::Record::Counts => None,
                        };
                        match window(&mut service, cycles, request.millis, chains).await {
                            Ok(counts) => answer(
                                id,
                                &service::Windowed::Counted {
                                    counts: counts.to_service(),
                                    spares: u8::try_from(spares.len()).unwrap_or(u8::MAX),
                                    chains: CASES
                                        .iter()
                                        .zip(&record.cases)
                                        .map(|(case, per)| service::ChainCase {
                                            tag: case.tag(),
                                            chains: per.chains,
                                            least: per.least,
                                            most: per.most,
                                            expected: expected_descriptors(*case) as u32,
                                            mismatched: per.mismatched,
                                        })
                                        .collect(),
                                    words: record
                                        .words
                                        .seen
                                        .iter()
                                        .enumerate()
                                        .flat_map(|(position, list)| {
                                            list.iter().map(move |(word, seen)| service::WordSeen {
                                                position: position as u8,
                                                word: *word,
                                                seen: *seen,
                                            })
                                        })
                                        .take(service::WORDS)
                                        .collect(),
                                    words_overflow: record.words.overflow,
                                },
                            ),
                            Err((phase, index)) => {
                                answer(
                                    id,
                                    &service::Windowed::Failed(service::Failure { phase, index }),
                                );
                                if let Receiving::Up { steps, .. } = &mut receiving {
                                    *steps = Steps::Failed;
                                }
                            }
                        }
                    }
                    ReceiverRequest::Stall(request) => {
                        wait_micros(u64::from(request.millis) * 1_000).await;
                        answer(
                            id,
                            &service::Stood::Standing(pair::standing_of(ring.standing())),
                        );
                    }
                    // Answered above.
                    ReceiverRequest::SnapshotCold(_)
                    | ReceiverRequest::SnapshotUp(_)
                    | ReceiverRequest::BringUp(_)
                    | ReceiverRequest::ReceiveCounters(_)
                    | ReceiverRequest::Stop(_)
                    | ReceiverRequest::Turn(_) => {}
                }
            }
        }
    }
}

/// The receiver's ring, while the bring-up's radio runs.
enum Receiving {
    /// The ring is up.
    Up {
        ring: oer_portable_memory::Occupied<'static, Ring>,
        enabled: oer_chips_esp32s31_hal::radio::ieee80211::dma::receive::WalkerEnabled,
        steps: Steps,
    },
    /// A failed turn's stop took the ring down: the radio is stopped.
    Stopped,
}

/// Whether a step failed while the ring was up.
#[derive(Clone, Copy)]
enum Steps {
    Open,
    /// A step failed: only the counters, the snapshots and the stop answer
    /// until the next boot.
    Failed,
}
