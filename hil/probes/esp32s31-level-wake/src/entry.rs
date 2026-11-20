use core::sync::atomic::{AtomicU32, Ordering};

use oer_chips_esp32s31_hal::peripherals::software_interrupt::{
    FromCpu0Irq, FromCpu2Irq, FromCpu3, FromCpu3Irq,
};
use oer_chips_esp32s31_hal::peripherals::system_timer::SystimerAlarm0;
use oer_chips_esp32s31_hal::peripherals::usb_serial_jtag;
use oer_chips_esp32s31_hal::peripherals::watchdog::{
    ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters,
};
use oer_chips_esp32s31_hal::system::platform::{LinePend, Port};
use oer_chips_esp32s31_hal::system::root::Root;
use oer_chips_esp32s31_platform_board as board;
use oer_chips_esp32s31_platform_runtime::Esp32s31;
use oer_chips_family_platform_executor::{Executor, InterruptExecutor};
use oer_chips_family_platform_interrupt_table::{Here, InterruptSource, InterruptSources, Pend};
use oer_portable_embassy_time::EmbassyClock;
use oer_portable_riscv::OnHart;
use oer_portable_time::{Duration, Timer as _};
use static_cell::StaticCell;

use crate::calc::{Line, Outcome, Tally};

oer_chips_family_platform_runtime::interrupt_table! {
    contexts { app_wake: 1; probe_executor: 2; keeper: 3; raiser: 4; }
    /// The boot hart's thread-mode executor's wake: the watchdog's feed and
    /// the report.
    wake: WakeToken = FromCpu0Irq => oer_chips_family_platform_executor::wake_handler::<Esp32s31, 0, FromCpu0Irq>, line 16, context app_wake, hart 0, Hardware;
    /// The thread-mode executor's timer alarm; it only acknowledges and pends.
    alarm: AlarmToken = SystimerAlarm0 => oer_chips_family_platform_executor::alarm_interrupt::<Tasks, 0, SystimerAlarm0>, line 17, context app_wake, hart 0, Hardware;
    /// The interrupt-mode executor below the task, whose task runs the
    /// trials and pends the task from below.
    probe: ProbeToken = FromCpu2Irq => oer_chips_family_platform_executor::Round<ProbeTasks>, line 18, context probe_executor, hart 0, Executor;
    /// The handler above the task, on `FROM_CPU3`: it pends the task with
    /// its witness.
    raiser: RaiserToken = RaiserPart => on_raiser, with here, line 19, context raiser, hart 0, Hardware;
    /// The level's one task, on a software line of its own.
    keeper: KeeperToken = KeeperPart => on_keeper, line 34, context keeper, hart 0, LevelTask;
}

oer_chips_family_platform_executor::tasks! {
    /// The boot hart's thread-mode tasks.
    Tasks: Executor<Esp32s31, 0> {
        rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_rtc_watchdog;
        report() = report;
    }
}

oer_chips_family_platform_executor::tasks! {
    /// The level-2 executor's task: the trials, from below the task.
    pub ProbeTasks: InterruptExecutor<Esp32s31, 2> {
        trials(pend: KeeperPend, raiser: &'static FromCpu3) = trials;
    }
}

/// The task's pend: from the handler above (`RaiserToken`) and from a task
/// of the level-2 executor (`ProbeToken`), through the line's handle.
pub type KeeperPend = Pend<KeeperToken, RaiserToken, ProbeToken, LinePend<'static, 0, 34>, 0>;

/// The level task's part: no source; it holds the line of the handler
/// above, which selection 6 raises during the task's run.
pub struct KeeperPart {
    raiser: &'static FromCpu3,
}

impl InterruptSources for KeeperPart {
    const SOURCES: &'static [u16] = &[];
}

/// The handler above's part: `FROM_CPU3`'s source, the line it lowers and
/// the task's pend.
pub struct RaiserPart {
    line: &'static FromCpu3,
    pend: KeeperPend,
}

impl InterruptSource for RaiserPart {
    const SOURCE: u16 = <FromCpu3Irq as InterruptSource>::SOURCE;
}

/// What the handler above and the task do in a trial: nothing.
const MODE_NONE: u32 = 0;
/// The handler pends the task once.
const MODE_ONCE: u32 = 1;
/// The handler pends the task twice.
const MODE_TWICE: u32 = 2;
/// The task's first entry raises the handler above, which pends it once.
const MODE_RUNNING: u32 = 3;

/// Trials of each selection in one boot.
const TRIALS: u32 = 100;
/// Cycles a trial waits for its entries: far above a pend's delay (218
/// cycles in `HW_S31_CLIC_LEVELS_SOFTWARE_PEND_20261118`).
const SETTLE_CYCLES: u64 = 20_000;
/// Cycles the boot hart waits for the trials after the enable: far above
/// their sum (`TRIALS` of four selections, each `SETTLE_CYCLES`).
const TRIALS_CYCLES: u64 = 400_000_000;
/// The wait after boot before the first line that matters: the host's
/// console attaches to the re-enumerated USB device.
const ATTACH: Duration = Duration::from_secs(5);
/// The report's poll of the trials' end.
const POLL: Duration = Duration::from_millis(10);

/// Selections 3 to 6, in order, each `(trials, held, least, most)` in the
/// record.
const SELECTIONS: [u32; 4] = [3, 4, 5, 6];
/// The record's word count.
const WORDS: usize = 4 * SELECTIONS.len();

oer_chips_family_platform_runtime::placed_static! {
    /// The trial's mode, written by the trials before each trigger.
    static MODE: AtomicU32 = AtomicU32::new(MODE_NONE), in critical_data "level_wake";
    /// The task's entries in the trial.
    static ENTRIES: AtomicU32 = AtomicU32::new(0), in critical_data "level_wake";
    /// The handler above's entries in the trial.
    static RAISES: AtomicU32 = AtomicU32::new(0), in critical_data "level_wake";
    /// The order of the trial's events: each event takes the next number.
    static SEQUENCE: AtomicU32 = AtomicU32::new(0), in critical_data "level_wake";
    /// The handler above's return, by its number.
    static RAISER_EXIT: AtomicU32 = AtomicU32::new(0), in critical_data "level_wake";
    /// The task's first entry, its return, and its second entry.
    static FIRST_ENTRY: AtomicU32 = AtomicU32::new(0), in critical_data "level_wake";
    static FIRST_EXIT: AtomicU32 = AtomicU32::new(0), in critical_data "level_wake";
    static SECOND_ENTRY: AtomicU32 = AtomicU32::new(0), in critical_data "level_wake";
    /// The cycle counter's low word just before the pend's store, and at
    /// the task's first statement.
    static STORED_AT: AtomicU32 = AtomicU32::new(0), in critical_data "level_wake";
    static ENTERED_AT: AtomicU32 = AtomicU32::new(0), in critical_data "level_wake";
    /// The task's entries between the enable and the first trial.
    static BOOT_ENTRIES: AtomicU32 = AtomicU32::new(u32::MAX), in critical_data "level_wake";
    /// The trials' record, then `READY`.
    static RECORD: [AtomicU32; WORDS] = [const { AtomicU32::new(0) }; WORDS], in critical_data "level_wake";
    static READY: AtomicU32 = AtomicU32::new(0), in critical_data "level_wake";
    /// The bind's verify (1 passed, 2 refused), and the second verify after
    /// the trials (the same; 0 when the trials did not end).
    static FIRST_VERIFY: AtomicU32 = AtomicU32::new(0), in critical_data "level_wake";
    static SECOND_VERIFY: AtomicU32 = AtomicU32::new(0), in critical_data "level_wake";
    /// The fresh buffers of the arena claimed at boot.
    static ARENA_BUFFERS: AtomicU32 = AtomicU32::new(0), in critical_data "level_wake";
}

oer_portable_memory::arena! {
    /// One arena placed in the DMA window by the HAL's placed static, claimed
    /// once at boot: the chip's placement of 5.1's arenas has its consumer
    /// here until the receive ring's probe claims its own.
    ArenaCheck: window oer_chips_esp32s31_hal::layouts::DmaWindow, count 2, size 64, floor 0,
        placed by oer_chips_esp32s31_hal::dma_static
        as oer_chips_esp32s31_hal::system::memory::dma_static::DmaStatic
        in ".dma.bss.level_wake_arena";
}

/// `FROM_CPU3`, raised by the trials and by the task, lowered by the handler
/// above.
static RAISER_LINE: StaticCell<FromCpu3> = StaticCell::new();

/// Whether this build stores the task's pending byte pended before its bind
/// (selection 2b) or binds it plainly (selection 1).
const PENDED_BIND: bool = cfg!(feature = "pended-bind");

oer_chips_family_platform_runtime::entry!(main, Port);

fn main(handoff: oer_chips_family_platform_runtime::Handoff<Port>) -> ! {
    let Some(root) = Root::take() else {
        fail(oer_portable_diagnostics::name!("root-taken"; 32));
    };
    let ((timer, software, watchdogs, pend_lines), _psram, mut adopted) =
        match oer_chips_family_platform_runtime::adoption::run::<Esp32s31, _, _, _>(
            handoff,
            root.into_runtime(),
            InterruptTable,
            board::BootstrapPsram,
            oer_chips_esp32s31_platform_runtime::phy_fault::report_previous,
            |hardware| {
                (
                    hardware.system_timer,
                    hardware.software_interrupts,
                    hardware.watchdogs,
                    hardware.pend_lines,
                )
            },
        ) {
            Ok(adopted) => adopted.into_parts(),
            Err(error) => fail(error.name()),
        };
    let Some(interrupts) = Interrupts::take() else {
        fail(oer_portable_diagnostics::name!("interrupt-table"; 32));
    };
    let Ok(on_hart) = OnHart::<0>::check() else {
        fail(oer_portable_diagnostics::name!("boot-hart"; 32));
    };
    let _timers = oer_chips_family_platform_executor::init::<Esp32s31, 0, _, _, _, _>(
        oer_chips_family_platform_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        adopted.router(),
        interrupts.alarm,
        timer.alarm0,
    )
    .unwrap_or_else(|failure| failure.reset());
    // The arena's place, claimed once: its buffers lie in the DMA window.
    let Some((_arena, fresh)) =
        oer_portable_memory::arena::Arena::<ArenaCheck, { 2 * 64 }>::claim()
    else {
        fail(oer_portable_diagnostics::name!("arena-claim"; 32));
    };
    ARENA_BUFFERS.store(fresh.count() as u32, Ordering::Relaxed);
    let raiser_line: &'static FromCpu3 = RAISER_LINE.init(software.line3);
    // Selection 2b: the pending byte stored pended before the bind, through
    // the line's handle with the hart's proof; the bind clears it.
    if PENDED_BIND {
        let line: LinePend<'static, 0, 34> = pend_lines.line();
        oer_chips_family_platform_interrupt_table::PendLine::pend(&line, &on_hart);
    }
    // Selections 1 and 2b: the bind returns its proof, and `verify` checks
    // the line as a task's.
    let keeper = adopted
        .router()
        .bind(interrupts.keeper, KeeperPart { raiser: raiser_line })
        .unwrap_or_else(|held| oer_chips_family_platform_runtime::interrupts::reset_binding::<Port, _>(held));
    FIRST_VERIFY.store(verdict(&adopted.router().verify()), Ordering::Relaxed);
    let raiser = RaiserPart {
        line: raiser_line,
        pend: Pend::new(&keeper, pend_lines.line()),
    };
    let _raiser = adopted
        .router()
        .bind(interrupts.raiser, raiser)
        .unwrap_or_else(|held| oer_chips_family_platform_runtime::interrupts::reset_binding::<Port, _>(held));
    let below = Pend::new(&keeper, pend_lines.line());
    let (hart, router) = adopted.parts();
    let spawner = InterruptExecutor::<Esp32s31, 2>::new(software.line2)
        .start::<ProbeTasks, _, _, 0, _, _>(hart, router, interrupts.probe, software.irq2, ())
        .unwrap_or_else(|failure| failure.reset());
    if ProbeTasks::trials(&spawner, below, raiser_line).is_err() {
        fail(oer_portable_diagnostics::name!("trials-start"; 32));
    }
    let executor = Executor::<Esp32s31, 0>::new(software.line0);
    let watchdog = watchdogs.rtc;
    text(b"OER-PROBE level-wake ready\r\n");
    // The enable: a task line left pending enters the task here, before the
    // level-2 executor runs the trials, which count it first.
    let (mut router, hart) = match adopted.enable_interrupts() {
        Ok(router) => router,
        Err(error) => fail(error.name()),
    };
    // Selection 8: the second verify, once the trials ended.
    if poll(TRIALS_CYCLES, || READY.load(Ordering::Acquire) != 0) == Outcome::Held {
        SECOND_VERIFY.store(verdict(&router.verify()), Ordering::Relaxed);
    }
    match executor
        .run(&hart, &mut router, interrupts.wake, software.irq0, (), (), move |spawner| {
            if Tasks::rtc_watchdog(spawner, watchdog).is_err() || Tasks::report(spawner).is_err() {
                fail(oer_portable_diagnostics::name!("task-start"; 32));
            }
        })
        .unwrap_or_else(|failure| failure.reset())
    {}
}

/// A verify's word in the record: 1 passed, 2 refused.
const fn verdict<E>(verified: &Result<(), E>) -> u32 {
    match verified {
        Ok(()) => 1,
        Err(_) => 2,
    }
}

/// Poll `done` until it holds (`Held`) or `limit` cycles pass (`Broke`).
#[inline(always)]
fn poll(limit: u64, done: impl Fn() -> bool) -> Outcome {
    let start = oer_portable_riscv::cycle64();
    loop {
        if done() {
            return Outcome::Held;
        }
        if oer_portable_riscv::cycle64().wrapping_sub(start) >= limit {
            return Outcome::Broke;
        }
    }
}

/// The trial's outcome: `Held` when what the selection expects happened.
#[inline(always)]
fn outcome(expected: impl FnOnce() -> bool) -> Outcome {
    if expected() {
        Outcome::Held
    } else {
        Outcome::Broke
    }
}

/// The low word of the cycle counter: the spans measured are far below
/// 2^32 cycles.
#[inline(always)]
fn cycles() -> u32 {
    oer_portable_riscv::cycle64() as u32
}

/// The next number of the trial's order.
#[inline(always)]
fn next() -> u32 {
    SEQUENCE.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
}

/// The level task: count the entry; in selection 6, raise the handler above
/// on the first entry, which pends this task during its run.
fn on_keeper(part: &mut KeeperPart) {
    let at = cycles();
    let entry = ENTRIES.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    if entry == 1 {
        ENTERED_AT.store(at, Ordering::Relaxed);
        FIRST_ENTRY.store(next(), Ordering::Relaxed);
        if MODE.load(Ordering::Relaxed) == MODE_RUNNING {
            part.raiser.raise();
            // The handler above preempts this task at once; the bound only
            // keeps a missing entry from holding the task.
            let _raised = poll(SETTLE_CYCLES, || RAISES.load(Ordering::Relaxed) != 0);
        }
        FIRST_EXIT.store(next(), Ordering::Relaxed);
    } else {
        SECOND_ENTRY.store(next(), Ordering::Relaxed);
    }
}

/// The handler above: lower its line, then pend the task by its mode with
/// the witness of the call.
fn on_raiser(part: &mut RaiserPart, here: Here<'_, RaiserToken>) {
    part.line.lower();
    RAISES.fetch_add(1, Ordering::Relaxed);
    match MODE.load(Ordering::Relaxed) {
        MODE_ONCE | MODE_RUNNING => {
            STORED_AT.store(cycles(), Ordering::Relaxed);
            part.pend.from_handler(&here);
        }
        MODE_TWICE => {
            STORED_AT.store(cycles(), Ordering::Relaxed);
            part.pend.from_handler(&here);
            part.pend.from_handler(&here);
        }
        _ => {}
    }
    RAISER_EXIT.store(next(), Ordering::Relaxed);
}

/// One trial's events, read after its settle.
struct Trial {
    entries: u32,
    raises: u32,
    raiser_exit: u32,
    first_entry: u32,
    first_exit: u32,
    second_entry: u32,
    delay: u32,
}

/// Run one trial: reset the counters, set the mode, trigger it, wait
/// `SETTLE_CYCLES`, read what happened.
fn trial(mode: u32, trigger: impl FnOnce()) -> Trial {
    for word in [
        &ENTRIES,
        &RAISES,
        &SEQUENCE,
        &RAISER_EXIT,
        &FIRST_ENTRY,
        &FIRST_EXIT,
        &SECOND_ENTRY,
        &STORED_AT,
        &ENTERED_AT,
    ] {
        word.store(0, Ordering::Relaxed);
    }
    MODE.store(mode, Ordering::Relaxed);
    trigger();
    // The settle: nothing ends it early.
    let _settled = poll(SETTLE_CYCLES, || false);
    MODE.store(MODE_NONE, Ordering::Relaxed);
    Trial {
        entries: ENTRIES.load(Ordering::Relaxed),
        raises: RAISES.load(Ordering::Relaxed),
        raiser_exit: RAISER_EXIT.load(Ordering::Relaxed),
        first_entry: FIRST_ENTRY.load(Ordering::Relaxed),
        first_exit: FIRST_EXIT.load(Ordering::Relaxed),
        second_entry: SECOND_ENTRY.load(Ordering::Relaxed),
        delay: ENTERED_AT
            .load(Ordering::Relaxed)
            .wrapping_sub(STORED_AT.load(Ordering::Relaxed)),
    }
}

/// The level-2 executor's task: selections 3 to 6, a hundred trials each,
/// then the record. Selection 7's spans are 3's and 4's.
async fn trials(pend: KeeperPend, raiser: &'static FromCpu3) {
    BOOT_ENTRIES.store(ENTRIES.load(Ordering::Relaxed), Ordering::Relaxed);
    let Ok(on_hart) = OnHart::<0>::check() else {
        fail(oer_portable_diagnostics::name!("trials-hart"; 32));
    };
    let mut tallies = [Tally::new(); SELECTIONS.len()];
    let [from_handler, from_task, twice, running] = &mut tallies;
    for _ in 0..TRIALS {
        // 3: from the handler above: one entry, after the handler returned.
        let t = trial(MODE_ONCE, || raiser.raise());
        from_handler.trial(outcome(|| {
            t.entries == 1 && t.raises == 1 && t.first_entry > t.raiser_exit
        }));
        from_handler.span(t.delay);
        // 4: from below, with the proof from `OnHart::check`: one entry.
        let t = trial(MODE_NONE, || {
            STORED_AT.store(cycles(), Ordering::Relaxed);
            pend.from_task(&on_hart);
        });
        from_task.trial(outcome(|| t.entries == 1 && t.raises == 0));
        from_task.span(t.delay);
        // 5: two pends before the entry: one entry.
        let t = trial(MODE_TWICE, || raiser.raise());
        twice.trial(outcome(|| {
            t.entries == 1 && t.raises == 1 && t.first_entry > t.raiser_exit
        }));
        // 6: a pend during the task's run: one more entry after it returns.
        let t = trial(MODE_RUNNING, || pend.from_task(&on_hart));
        running.trial(outcome(|| {
            t.entries == 2
                && t.raises == 1
                && t.raiser_exit > t.first_entry
                && t.first_exit > t.raiser_exit
                && t.second_entry > t.first_exit
        }));
    }
    for (index, tally) in tallies.iter().enumerate() {
        let (least, most) = tally.span.unwrap_or((0, 0));
        for (offset, word) in [tally.trials, tally.held, least, most].into_iter().enumerate() {
            if let Some(slot) = RECORD.get(index.saturating_mul(4).saturating_add(offset)) {
                slot.store(word, Ordering::Relaxed);
            }
        }
    }
    READY.store(1, Ordering::Release);
}

/// After the console attaches: the bind's line, a line a selection and the
/// verdict, every selection held.
async fn report() {
    if EmbassyClock.wait_for(ATTACH).await.is_err() {
        fail(oer_portable_diagnostics::name!("attach-time"; 32));
    }
    while READY.load(Ordering::Acquire) == 0 {
        if EmbassyClock.wait_for(POLL).await.is_err() {
            fail(oer_portable_diagnostics::name!("poll-time"; 32));
        }
    }
    let boot_entries = BOOT_ENTRIES.load(Ordering::Relaxed);
    let first = FIRST_VERIFY.load(Ordering::Relaxed);
    let second = SECOND_VERIFY.load(Ordering::Relaxed);
    let mut pass = boot_entries == 0 && first == 1 && second == 1;
    text(
        Line::new()
            .text("OER-PROBE level-wake selection=")
            .text(if PENDED_BIND { "2b bind=pended" } else { "1 bind=plain" })
            .text(" bound=1 verify=")
            .number(u64::from(first))
            .text(" entries-after-enable=")
            .number(u64::from(boot_entries))
            .text(" arena-buffers=")
            .number(u64::from(ARENA_BUFFERS.load(Ordering::Relaxed)))
            .end(),
    );
    for (index, selection) in SELECTIONS.iter().enumerate() {
        let word = |offset: usize| {
            RECORD
                .get(index.saturating_mul(4).saturating_add(offset))
                .map_or(0, |word| word.load(Ordering::Relaxed))
        };
        let tally = Tally {
            trials: word(0),
            held: word(1),
            span: None,
        };
        pass &= tally.holds() && tally.trials == TRIALS;
        let mut line = Line::new();
        line.text("OER-PROBE level-wake selection=")
            .number(u64::from(*selection))
            .text(" trials=")
            .number(u64::from(tally.trials))
            .text(" held=")
            .number(u64::from(tally.held));
        if *selection == 3 || *selection == 4 {
            line.text(" delay-least=")
                .number(u64::from(word(2)))
                .text(" delay-most=")
                .number(u64::from(word(3)));
        }
        text(line.end());
    }
    text(
        Line::new()
            .text("OER-PROBE level-wake selection=8 verify=")
            .number(u64::from(second))
            .end(),
    );
    text(if pass {
        b"OER-RESULT: pass level-wake\r\n"
    } else {
        b"OER-RESULT: fail level-wake\r\n"
    });
}

async fn feed_rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) {
    let period = Duration::from_millis(RTC_WATCHDOG_FEED_PERIOD_MS);
    match watchdog.feed_every(&EmbassyClock, period).await {}
}

/// Write `bytes` through the early writer, within its bounded budget.
fn text(bytes: &[u8]) {
    let _written = usb_serial_jtag::early_text(bytes);
}

/// Retain and report a failed boot step before explicitly resetting the image.
fn fail<const B: usize>(reason: oer_portable_diagnostics::Name<B>) -> ! {
    oer_chips_family_platform_runtime::fatal::boot_failure::<Port>(reason.as_str(), || {
        let _note = usb_serial_jtag::fatal_note(&oer_portable_diagnostics::Note::runtime_fail(reason));
    })
}

oer_chips_family_platform_executor::time_driver!(Esp32s31);
