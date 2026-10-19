//! What W1 measures besides the receive counters: the run's phase, the
//! cycles of each P2 poll and handler run, the MAC-to-poll and
//! ring-to-doorbell latencies, the platform clock operations, the periodic
//! PHY tracking ticks and the timer windows of both executors.
//!
//! Every value a P3 handler, the radio executor (P2) and the application
//! (P1) share is an atomic in `.critical.data.w1`, with one writer where
//! the type says so ([`CycleHistogram`]); no lock and no critical section
//! is taken. The writers stop recording once the phase is
//! [`Phase::Report`], so the report reads settled values.

use core::{
    future::{Future, poll_fn},
    pin::pin,
    sync::atomic::{AtomicU8, AtomicU32, Ordering},
    task::Poll,
};

use atomic_waker::AtomicWaker;
use oer_adapters_esp_hal_radio::{ClockBackend, EspHal};
use oer_base_memory::AffineSpscSender;
use oer_base_time::{Clock, Duration, Timer};
use oer_esp32s31_hal::power::{PlatformClock, PlatformClockAbandonment};
use oer_esp32s31_radio_client::{Client, Failed, LpClock, LpSource, PhyPreparation};
use oer_esp32s31_radio_system::{
    ClientBackend, CloseFault, Exit, Fault, LastClientLeft, PhyDomain,
};
use oer_exec_executor::{
    poll_timing::{CycleHistogram, CycleSource, Mcycle},
    timer_observation::{Report, Window},
};

use oer_adapters_embassy_time::EmbassyClock;

/// The phases of one run, in order.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum Phase {
    BringUp = 0,
    FirstFrame = 1,
    Window = 2,
    Stress = 3,
    Hold = 4,
    Stop = 5,
    /// The run is over; nothing records any more.
    Report = 6,
}

impl Phase {
    pub const ALL: [Self; 7] = [
        Self::BringUp,
        Self::FirstFrame,
        Self::Window,
        Self::Stress,
        Self::Hold,
        Self::Stop,
        Self::Report,
    ];

    /// The phase's name in the report.
    pub const fn name(self) -> &'static str {
        match self {
            Self::BringUp => "bringup",
            Self::FirstFrame => "first_frame",
            Self::Window => "window",
            Self::Stress => "stress",
            Self::Hold => "hold",
            Self::Stop => "stop",
            Self::Report => "report",
        }
    }

    const fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::BringUp,
            1 => Self::FirstFrame,
            2 => Self::Window,
            3 => Self::Stress,
            4 => Self::Hold,
            5 => Self::Stop,
            _ => Self::Report,
        }
    }

    /// The slot of the phases the P2 timings are kept for.
    pub const fn timed(self) -> Option<usize> {
        match self {
            Self::Window => Some(0),
            Self::Stress => Some(1),
            Self::Hold => Some(2),
            _ => None,
        }
    }
}

/// The phases the P2 timings are kept for, by slot.
pub const TIMED_PHASES: [Phase; 3] = [Phase::Window, Phase::Stress, Phase::Hold];

oer_platform_staged_runtime::critical_static! {
    static PHASE: AtomicU8 = AtomicU8::new(Phase::BringUp as u8), in "w1";
    /// The P2 timer task waits on it for the phase to change.
    static PHASE_WAKER: AtomicWaker = AtomicWaker::new(), in "w1";
}

/// The phase now.
#[inline(always)]
pub fn phase() -> Phase {
    Phase::from_u8(PHASE.load(Ordering::Acquire))
}

/// Enter `phase` (P1 alone) and wake the P2 timer task.
pub fn enter(phase: Phase) {
    PHASE.store(phase as u8, Ordering::Release);
    PHASE_WAKER.wake();
}

/// Wait until the phase is `phase` or later; the phase reached.
async fn reached(phase: Phase) -> Phase {
    poll_fn(|cx| {
        PHASE_WAKER.register(cx.waker());
        let now = self::phase();
        if now >= phase {
            Poll::Ready(now)
        } else {
            Poll::Pending
        }
    })
    .await
}

/// The cycle counter now, never zero: zero marks an empty latch.
#[inline(always)]
pub fn stamp() -> u32 {
    Mcycle.now().max(1)
}

oer_platform_staged_runtime::critical_static! {
    /// Cycles of each run of the radio executor's handler (SWI2), per timed
    /// phase; written by that handler alone.
    pub static SWI2: [CycleHistogram; 3] = [const { CycleHistogram::new() }; 3], in "w1";
    /// Cycles of each poll of the radio system's actor, per timed phase.
    pub static ACTOR: [CycleHistogram; 3] = [const { CycleHistogram::new() }; 3], in "w1";
    /// Cycles of each poll of the receive runner, per timed phase.
    pub static RECEIVE: [CycleHistogram; 3] = [const { CycleHistogram::new() }; 3], in "w1";
    /// The longest MAC handler entry to the next receive poll, in cycles, per
    /// timed phase; written by the receive runner's polls.
    pub static MAC_TO_POLL_MAX: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3], in "w1";
    /// The longest doorbell ring to its handler, in cycles, per timed phase;
    /// written by the doorbell handler.
    pub static RING_TO_DOORBELL_MAX: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3], in "w1";
    /// The MAC handler's entry stamp, until the next receive poll takes it.
    pub static MAC_LATCH: AtomicU32 = AtomicU32::new(0), in "w1";
    /// The doorbell's ring stamp, until its handler takes it.
    pub static RING_AT: AtomicU32 = AtomicU32::new(0), in "w1";
}

/// Record `cycles` into the slot of the phase now, if it is timed.
#[inline(always)]
pub fn record(histograms: &'static [CycleHistogram; 3], cycles: u32) {
    if let Some(histogram) = phase().timed().and_then(|slot| histograms.get(slot)) {
        histogram.record(cycles);
    }
}

/// Raise the slot of the phase now to `cycles`, if it is timed.
#[inline(always)]
pub fn raise(maxima: &'static [AtomicU32; 3], cycles: u32) {
    if let Some(maximum) = phase().timed().and_then(|slot| maxima.get(slot)) {
        maximum.fetch_max(cycles, Ordering::Relaxed);
    }
}

/// `future`, each poll's cycles recorded into `histograms` by phase; it is
/// their one writer. `before` runs at the start of each poll.
pub async fn timed<F: Future>(
    future: F,
    histograms: &'static [CycleHistogram; 3],
    before: impl Fn(),
) -> F::Output {
    let mut future = pin!(future);
    poll_fn(|cx| {
        before();
        let start = Mcycle.now();
        let poll = future.as_mut().poll(cx);
        record(histograms, Mcycle.now().wrapping_sub(start));
        poll
    })
    .await
}

/// Take the MAC latch at a receive poll: the time since the MAC handler
/// that set it.
#[inline(always)]
pub fn take_mac_latch() {
    let latched = MAC_LATCH.swap(0, Ordering::AcqRel);
    if latched != 0 {
        raise(&MAC_TO_POLL_MAX, Mcycle.now().wrapping_sub(latched));
    }
}

/// The platform clocks, as the report names them.
pub const CLOCKS: [PlatformClock; 5] = [
    PlatformClock::Pll160m,
    PlatformClock::AnalogI2cMaster,
    PlatformClock::Mpll,
    PlatformClock::ModemCoexistence,
    PlatformClock::ModemLowPowerTimer,
];

/// A clock's name in the report.
pub const fn clock_name(clock: PlatformClock) -> &'static str {
    match clock {
        PlatformClock::Pll160m => "pll160m",
        PlatformClock::AnalogI2cMaster => "analog_i2c_master",
        PlatformClock::Mpll => "mpll",
        PlatformClock::ModemCoexistence => "modem_coexistence",
        PlatformClock::ModemLowPowerTimer => "modem_low_power_timer",
    }
}

const fn clock_slot(clock: PlatformClock) -> usize {
    match clock {
        PlatformClock::Pll160m => 0,
        PlatformClock::AnalogI2cMaster => 1,
        PlatformClock::Mpll => 2,
        PlatformClock::ModemCoexistence => 3,
        PlatformClock::ModemLowPowerTimer => 4,
    }
}

/// A platform clock operation.
#[derive(Clone, Copy)]
pub enum ClockOp {
    Acquire = 0,
    Release = 1,
    Abandon = 2,
}

impl ClockOp {
    pub const ALL: [Self; 3] = [Self::Acquire, Self::Release, Self::Abandon];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Acquire => "acquire",
            Self::Release => "release",
            Self::Abandon => "abandon",
        }
    }
}

const RECORDED_PHASES: usize = Phase::Report as usize;

oer_platform_staged_runtime::critical_static! {
    /// Per clock, operation and phase (before the report): the operations.
    static CLOCK_COUNT: [[[AtomicU32; RECORDED_PHASES]; 3]; 5] =
        [const { [const { [const { AtomicU32::new(0) }; RECORDED_PHASES] }; 3] }; 5], in "w1";
    /// Per clock, operation and phase: the longest operation, in cycles.
    static CLOCK_MAX: [[[AtomicU32; RECORDED_PHASES]; 3]; 5] =
        [const { [const { [const { AtomicU32::new(0) }; RECORDED_PHASES] }; 3] }; 5], in "w1";
}

fn clock_cell(
    table: &[[[AtomicU32; RECORDED_PHASES]; 3]; 5],
    clock: PlatformClock,
    op: ClockOp,
    phase: Phase,
) -> Option<&AtomicU32> {
    table
        .get(clock_slot(clock))?
        .get(op as usize)?
        .get(phase as usize)
}

fn clock_op(clock: PlatformClock, op: ClockOp, run: impl FnOnce()) {
    let start = Mcycle.now();
    run();
    let cycles = Mcycle.now().wrapping_sub(start);
    let phase = phase();
    if let (Some(count), Some(max)) = (
        clock_cell(&CLOCK_COUNT, clock, op, phase),
        clock_cell(&CLOCK_MAX, clock, op, phase),
    ) {
        count.fetch_add(1, Ordering::Relaxed);
        max.fetch_max(cycles, Ordering::Relaxed);
    }
}

/// The operations of `clock` by `op` in `phase` and the longest, in cycles.
pub fn clock_stats(clock: PlatformClock, op: ClockOp, phase: Phase) -> (u32, u32) {
    let load =
        |table| clock_cell(table, clock, op, phase).map_or(0, |cell| cell.load(Ordering::Relaxed));
    (load(&CLOCK_COUNT), load(&CLOCK_MAX))
}

/// esp-hal's clock backend, each operation timed by clock, operation and
/// phase.
pub struct TimedEspHal;

impl ClockBackend for TimedEspHal {
    fn acquire(clock: PlatformClock) {
        clock_op(clock, ClockOp::Acquire, || EspHal::acquire(clock));
    }

    fn release(clock: PlatformClock) {
        clock_op(clock, ClockOp::Release, || EspHal::release(clock));
    }

    fn abandon(clock: PlatformClock, how: PlatformClockAbandonment) {
        clock_op(clock, ClockOp::Abandon, || EspHal::abandon(clock, how));
    }
}

/// The periodic tracking ticks the log keeps, the latest ones.
pub const TICKS: usize = 32;

oer_platform_staged_runtime::critical_static! {
    static TICK_START: [AtomicU32; TICKS] = [const { AtomicU32::new(0) }; TICKS], in "w1";
    static TICK_END: [AtomicU32; TICKS] = [const { AtomicU32::new(0) }; TICKS], in "w1";
    static TICK_OK: [AtomicU8; TICKS] = [const { AtomicU8::new(0) }; TICKS], in "w1";
    /// Ticks logged; the entry of tick `n` is `n % TICKS`.
    static TICK_COUNT: AtomicU32 = AtomicU32::new(0), in "w1";
}

/// One logged tracking tick: SYSTIMER µs, low 32 bits.
#[derive(Clone, Copy)]
pub struct Tick {
    pub start: u32,
    pub end: u32,
    pub ok: bool,
}

/// The logged ticks, oldest first.
pub fn ticks() -> heapless::Vec<Tick, TICKS> {
    let count = TICK_COUNT.load(Ordering::Acquire) as usize;
    let first = count.saturating_sub(TICKS);
    let mut ticks = heapless::Vec::new();
    for tick in first..count {
        let slot = tick % TICKS;
        if let (Some(start), Some(end), Some(ok)) =
            (TICK_START.get(slot), TICK_END.get(slot), TICK_OK.get(slot))
        {
            let _full = ticks.push(Tick {
                start: start.load(Ordering::Relaxed),
                end: end.load(Ordering::Relaxed),
                ok: ok.load(Ordering::Relaxed) != 0,
            });
        }
    }
    ticks
}

fn log_tick(start: u32, end: u32, ok: bool) {
    if phase() == Phase::Report {
        return;
    }
    let count = TICK_COUNT.load(Ordering::Relaxed);
    let slot = count as usize % TICKS;
    if let (Some(start_cell), Some(end_cell), Some(ok_cell)) =
        (TICK_START.get(slot), TICK_END.get(slot), TICK_OK.get(slot))
    {
        start_cell.store(start, Ordering::Relaxed);
        end_cell.store(end, Ordering::Relaxed);
        ok_cell.store(u8::from(ok), Ordering::Relaxed);
        TICK_COUNT.store(count.wrapping_add(1), Ordering::Release);
    }
}

/// The SYSTIMER µs now, low 32 bits.
fn micros_low() -> u32 {
    EmbassyClock.now().as_micros() as u32
}

/// The radio system's backend `B`, its periodic tracking ticks logged.
pub struct TickStamped<B>(pub B);

impl<B: PhyDomain> PhyDomain for TickStamped<B> {
    async fn prepare(&mut self) -> Result<PhyPreparation, Fault> {
        self.0.prepare().await
    }

    async fn close_rf(&mut self, last: LastClientLeft) -> Result<(), CloseFault> {
        self.0.close_rf(last).await
    }

    fn tracking_deadline(&self) -> Option<oer_base_time::Instant> {
        self.0.tracking_deadline()
    }

    async fn track_tick(&mut self) -> Result<(), Fault> {
        let start = micros_low();
        let result = self.0.track_tick().await;
        log_tick(start, micros_low(), result.is_ok());
        result
    }
}

impl<K: Client, S: LpSource, B: ClientBackend<K, S>> ClientBackend<K, S> for TickStamped<B> {
    async fn enter(
        &mut self,
        join: K::Join,
        phy: PhyPreparation,
        lp: &LpClock<S>,
    ) -> Result<K::Joined, Failed<K::JoinReturned>> {
        self.0.enter(join, phy, lp).await
    }

    async fn track(&mut self, track: K::Track) -> Result<K::Tracked, Failed<K::TrackReturned>> {
        self.0.track(track).await
    }

    async fn exit(&mut self, leave: K::Leave) -> (K::Left, Exit) {
        self.0.exit(leave).await
    }
}

/// One timer observation window of one comparator.
#[derive(Clone, Copy)]
pub struct TimerLine {
    /// The comparator: 0 is the radio executor's, 1 the application's.
    pub comparator: u8,
    pub phase: Phase,
    pub report: Report,
}

/// The radio executor's timer comparator.
pub const P2_COMPARATOR: u8 = 0;
/// The application executor's timer comparator.
pub const P1_COMPARATOR: u8 = 1;

/// The P2 timer stress period.
const P2_STRESS_PERIOD: Duration = Duration::from_micros(37);
/// The P1 timer stress period.
pub const P1_STRESS_PERIOD: Duration = Duration::from_micros(113);

/// The radio executor's timer windows: the window phase as the baseline,
/// then the stress phase with a timer every 37 µs. Each window's report is
/// sent to the application.
#[embassy_executor::task]
pub async fn p2_timers(lines: AffineSpscSender<'static, TimerLine, 2>) {
    if reached(Phase::Window).await == Phase::Window {
        let window = Window::begin();
        reached(Phase::Stress).await;
        if let Some(window) = window {
            let _full = lines.try_send(TimerLine {
                comparator: P2_COMPARATOR,
                phase: Phase::Window,
                report: window.finish(),
            });
        }
    }
    if phase() == Phase::Stress {
        let window = Window::begin();
        while phase() == Phase::Stress {
            wait_until(
                P2_COMPARATOR,
                EmbassyClock.now().saturating_add(P2_STRESS_PERIOD),
            )
            .await;
        }
        if let Some(window) = window {
            let _full = lines.try_send(TimerLine {
                comparator: P2_COMPARATOR,
                phase: Phase::Stress,
                report: window.finish(),
            });
        }
    }
}

/// The application's timer stress: a timer every 113 µs until `deadline`.
pub async fn p1_stress(deadline: oer_base_time::Instant) {
    while EmbassyClock.now() < deadline {
        wait_until(
            P1_COMPARATOR,
            EmbassyClock.now().saturating_add(P1_STRESS_PERIOD),
        )
        .await;
    }
}

oer_platform_staged_runtime::critical_static! {
    /// Per executor (by its comparator) and phase: the image's timer waits.
    static WAITS: [[AtomicU32; RECORDED_PHASES]; 2] =
        [const { [const { AtomicU32::new(0) }; RECORDED_PHASES] }; 2], in "w1";
    /// Per executor and phase: the waits that returned before their deadline.
    static WOKE_EARLY: [[AtomicU32; RECORDED_PHASES]; 2] =
        [const { [const { AtomicU32::new(0) }; RECORDED_PHASES] }; 2], in "w1";
}

/// Wait until `deadline` on the executor of `comparator`, which is the
/// caller's; count the wait, and count it early when it returned before
/// the deadline.
pub async fn wait_until(comparator: u8, deadline: oer_base_time::Instant) {
    EmbassyClock.wait_until(deadline).await;
    let early = EmbassyClock.now() < deadline;
    let phase = phase();
    let cell = |table: &'static [[AtomicU32; RECORDED_PHASES]; 2]| {
        table
            .get(usize::from(comparator))
            .and_then(|phases| phases.get(phase as usize))
    };
    if let Some(waits) = cell(&WAITS) {
        waits.fetch_add(1, Ordering::Relaxed);
    }
    if early && let Some(woke_early) = cell(&WOKE_EARLY) {
        woke_early.fetch_add(1, Ordering::Relaxed);
    }
}

/// The waits of the executor of `comparator` in `phase`, and how many woke
/// early.
pub fn waits(comparator: u8, phase: Phase) -> (u32, u32) {
    let load = |table: &[[AtomicU32; RECORDED_PHASES]; 2]| {
        table
            .get(usize::from(comparator))
            .and_then(|phases| phases.get(phase as usize))
            .map_or(0, |cell| cell.load(Ordering::Relaxed))
    };
    (load(&WAITS), load(&WOKE_EARLY))
}
