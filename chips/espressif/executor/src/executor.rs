use core::{marker::PhantomData, task::Context};

use oer_interrupt_table::{
    Entry,
    signal::{ContextToken, ForeignTask, IrqSignal, SignalExecutor, SignalList},
};
use portable_atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

use crate::chip::{
    SOFTWARE_INTERRUPT_COUNT, current_core, install_line, interrupt_level, platform,
    raise_interrupt, serve_interrupt, software_interrupt_source,
};
pub(crate) use crate::context::NO_EXECUTOR;
use crate::context::Running;
use crate::task::{Current, Interrupt as InterruptMode, Ready, Spawner, TaskId, Tasks, Thread};

const CORE_COUNT: usize = 2;
const UNASSIGNED_CORE: usize = usize::MAX;

/// Each thread-mode executor's wake flag: work other than a ready task (an
/// expired timer, a published deadline) for its next loop.
#[repr(transparent)]
struct WorkPending([AtomicU8; SOFTWARE_INTERRUPT_COUNT]);

#[allow(unsafe_code, reason = "zero wake flags are zero bytes")]
// SAFETY: `portable_atomic::AtomicU8` has the in-memory representation of
// `u8` (its documented guarantee). Zero means a clear wake flag.
unsafe impl bytemuck::Zeroable for WorkPending {}

oer_memory::zeroed_static! {
    /// Placement: the board linker owns this exported wake-state section.
    #[used]
    static WORK_PENDING: WorkPending = zeroed in ".critical.bss.executor";
}

/// The registered core in the low two bits, plus the current task poll
/// timestamp in the remaining bits. The unassigned sentinel has core bits 3.
#[used]
#[allow(
    unsafe_code,
    reason = "board linker owns this exported executor core-state section"
)]
#[unsafe(link_section = ".critical.data.executor")]
static EXECUTOR_CORE: [AtomicUsize; SOFTWARE_INTERRUPT_COUNT] =
    [const { AtomicUsize::new(UNASSIGNED_CORE) }; SOFTWARE_INTERRUPT_COUNT];

/// Each executor's ready tasks, one bit each.
#[used]
#[allow(
    unsafe_code,
    reason = "board linker owns this exported executor core-state section"
)]
#[unsafe(link_section = ".critical.data.executor")]
static READY: [Ready; SOFTWARE_INTERRUPT_COUNT] =
    [const { Ready::new() }; SOFTWARE_INTERRUPT_COUNT];

/// Which executor's context runs on each core now: the software interrupt of
/// the executor that is polling and the interrupt level it polls at. Each
/// executor sets it for its run and restores the context it preempted; a
/// hardware interrupt that preempts it runs at a higher level, which is no
/// executor's context ([`crate::context`]).
#[used]
#[allow(
    unsafe_code,
    reason = "board linker owns this exported executor core-state section"
)]
#[unsafe(link_section = ".critical.data.executor")]
static RUNNING: [Running; CORE_COUNT] = [const { Running::new() }; CORE_COUNT];

/// The task each executor polls now; only that executor's context reads it,
/// to name a polling task for its signals.
static CURRENT: [Current; SOFTWARE_INTERRUPT_COUNT] =
    [const { Current::new() }; SOFTWARE_INTERRUPT_COUNT];

/// Whether each software interrupt serves a started interrupt-mode executor.
static INTERRUPT_MODE: [AtomicBool; SOFTWARE_INTERRUPT_COUNT] =
    [const { AtomicBool::new(false) }; SOFTWARE_INTERRUPT_COUNT];

/// The executor whose context the caller runs in: [`NO_EXECUTOR`] outside
/// every executor and in a hardware interrupt that preempts one.
pub(crate) fn running() -> u8 {
    RUNNING[current_core()].at(interrupt_level())
}

/// Executor `executor`'s ready mask.
pub(crate) fn ready(executor: u8) -> Option<&'static Ready> {
    READY.get(usize::from(executor))
}

/// Whether the current executor has another ready task. A wake of the
/// running task itself also counts until its next poll, conservatively.
/// Signal/interrupt facts not yet translated into task wakes are additional
/// reasons to yield and must be checked by their owner.
///
/// # Panics
/// Outside a started executor's polling context.
pub fn ready_peers() -> bool {
    READY
        .get(usize::from(running()))
        .expect("ready peers requires a running executor")
        .any()
}

// Keep the clock and timestamp publication out of the poll round.
#[inline(never)]
fn stamp_poll(swi: u8) {
    let state = &EXECUTOR_CORE[usize::from(swi)];
    let now = crate::chip::now();
    state.store(
        crate::poll_stamp::poll_stamp(state.load(Ordering::Relaxed), now),
        Ordering::Release,
    );
}

// Admission is shared by the task wrapper and all PHY leaf boundaries.
#[inline(never)]
pub(crate) fn should_yield() -> bool {
    let swi = usize::from(running());
    let state = EXECUTOR_CORE
        .get(swi)
        .expect("cooperative admission requires a running executor");
    READY[swi].any()
        || crate::chip::is_pending(software_interrupt_source(swi as u8))
        || crate::chip::is_pending(software_interrupt_source(3))
        || signal_pending(swi)
        || {
            let now = crate::chip::now();
            crate::poll_stamp::poll_elapsed(state.load(Ordering::Acquire), now)
                >= crate::poll_stamp::POLL_ADMISSION_MICROS
        }
}
fn signal_pending(swi: usize) -> bool {
    let interrupt_mode = INTERRUPT_MODE[swi].load(Ordering::Acquire);
    let pending = match (interrupt_mode, swi) {
        (true, 0) => oer_interrupt_table::signal::has_pending::<InterruptExecutor<0>>(),
        (true, 1) => oer_interrupt_table::signal::has_pending::<InterruptExecutor<1>>(),
        (true, 2) => oer_interrupt_table::signal::has_pending::<InterruptExecutor<2>>(),
        (true, _) => oer_interrupt_table::signal::has_pending::<InterruptExecutor<3>>(),
        (false, 0) => oer_interrupt_table::signal::has_pending::<Executor<0>>(),
        (false, 1) => oer_interrupt_table::signal::has_pending::<Executor<1>>(),
        (false, 2) => oer_interrupt_table::signal::has_pending::<Executor<2>>(),
        (false, _) => oer_interrupt_table::signal::has_pending::<Executor<3>>(),
    };
    match pending {
        Ok(pending) => pending,
        Err(_) => panic!("signals are queried from their owning executor"),
    }
}

/// Run `poll` as executor `swi`'s context, at the caller's interrupt level,
/// and restore the preempted one.
fn as_running(swi: u8, poll: impl FnOnce()) {
    RUNNING[current_core()].enter(swi, interrupt_level(), poll);
}

/// One poll round of executor `T::EXECUTOR`, in its context: whether a task
/// is ready again.
fn poll_tasks<T: Tasks>() -> bool {
    let swi = T::EXECUTOR;
    crate::task::poll_round::<T>(&READY[usize::from(swi)], &CURRENT[usize::from(swi)], || {
        stamp_poll(swi)
    })
}

/// Claim executor `swi` for the current core.
///
/// # Panics
/// When another executor uses the software interrupt.
fn claim_core(swi: u8) {
    EXECUTOR_CORE[usize::from(swi)]
        .compare_exchange(
            UNASSIGNED_CORE,
            current_core(),
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .unwrap_or_else(|_| panic!("software interrupt {swi} is already used by an executor"));
}

/// The level of the image's three execution contexts, checked when the image
/// is compiled: the radio interrupts (`High`, P3) preempt the radio executor
/// (`Mid`, P2), which preempts the application (`Low`), all on one core.
///
/// ```ignore
/// const ORDER: LevelOrder<MacToken, RadioWakeToken, AppWakeToken> = LevelOrder::new();
/// ```
pub struct LevelOrder<High, Mid, Low> {
    _levels: PhantomData<Levels<High, Mid, Low>>,
}

type Levels<High, Mid, Low> = fn() -> (High, Mid, Low);

impl<High, Mid, Low> LevelOrder<High, Mid, Low>
where
    High: Entry,
    Mid: Entry,
    Low: Entry,
{
    /// The order of the three entries' levels and cores, or a compile error.
    pub const fn new() -> Self {
        const {
            assert!(
                High::LEVEL as u8 > Mid::LEVEL as u8,
                "the radio interrupts must preempt the radio executor"
            );
            assert!(
                Mid::LEVEL as u8 > Low::LEVEL as u8,
                "the radio executor must preempt the application"
            );
            assert!(
                High::HART as u8 == Mid::HART as u8 && Mid::HART as u8 == Low::HART as u8,
                "the three contexts share one core"
            );
        }
        Self {
            _levels: PhantomData,
        }
    }
}

impl<High, Mid, Low> Default for LevelOrder<High, Mid, Low>
where
    High: Entry,
    Mid: Entry,
    Low: Entry,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<High, Mid, Low> Clone for LevelOrder<High, Mid, Low> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<High, Mid, Low> Copy for LevelOrder<High, Mid, Low> {}

/// Two entries at one level on one core, checked when the image is compiled:
/// a radio interrupt (`Radio`, P3) and the [`Doorbell`](crate::Doorbell)
/// (`Bell`) whose handler applies requests to that interrupt's registers.
/// Equal levels on one hart cannot preempt each other. The host tests of
/// `level` cover the refusals.
pub struct SameLevel<Radio, Bell> {
    _levels: PhantomData<Pair<Radio, Bell>>,
}

type Pair<Radio, Bell> = fn() -> (Radio, Bell);

impl<Radio, Bell> SameLevel<Radio, Bell>
where
    Radio: Entry,
    Bell: Entry,
{
    /// The two entries' common level and core, or a compile error.
    pub const fn new() -> Self {
        const {
            crate::level::same_level(
                (Radio::LEVEL as u8, Radio::HART as u8),
                (Bell::LEVEL as u8, Bell::HART as u8),
            )
        }
        Self {
            _levels: PhantomData,
        }
    }
}

impl<Radio, Bell> Default for SameLevel<Radio, Bell>
where
    Radio: Entry,
    Bell: Entry,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<Radio, Bell> Clone for SameLevel<Radio, Bell> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<Radio, Bell> Copy for SameLevel<Radio, Bell> {}

/// Scheduler-free thread-mode executor.
///
/// One software interrupt is reserved per executor so a waker on another CPU
/// can wake the sleeping owner without introducing a scheduler or RTOS.
pub struct Executor<const SWI: u8> {
    not_send: PhantomData<*mut ()>,
}

impl<const SWI: u8> Executor<SWI> {
    /// Run the tasks `T` on the calling core, woken through the executor's
    /// software interrupt: `wake` is that source's token of the image's
    /// interrupt table, whose entry names [`wake_handler::<SWI>`](wake_handler)
    /// on this core. `init` starts the first tasks.
    ///
    /// # Panics
    ///
    /// When the table routes the source to another core.
    pub fn run<T, W, C>(
        self,
        router: &mut oer_interrupt_table::Router<C>,
        wake: W,
        part: W::Part,
        init: impl FnOnce(&Spawner<T>),
    ) -> !
    where
        T: Tasks<Mode = Thread>,
        W: Entry,
        C: oer_interrupt_table::Controller,
    {
        const {
            assert!(
                <W::Part as oer_interrupt_table::InterruptSource>::SOURCE
                    == software_interrupt_source(SWI) as u16,
                "the token is not this executor's software interrupt"
            );
            assert!(T::EXECUTOR == SWI, "the tasks are another executor's");
        };
        let _ = self.not_send;
        assert_eq!(
            W::HART as usize,
            current_core(),
            "executor route is on this hart"
        );
        if let Err(error) = router.bind(wake, part) {
            panic!("executor software interrupt {SWI}: {error:?}");
        }
        claim_core(SWI);
        init(&Spawner::new());
        loop {
            WORK_PENDING.0[usize::from(SWI)].store(0, Ordering::Release);
            as_running(SWI, || {
                crate::time_driver::service(SWI);
                // Raised signals wake their tasks before the round; a signal
                // raised meanwhile marks the work flag again.
                oer_interrupt_table::signal::service::<Self>();
                let _ = poll_tasks::<T>();
            });
            wait_for_work::<SWI>();
        }
    }
}

/// The signals each executor services before its poll round; only that
/// executor's context touches its list.
static SIGNAL_LISTS: [SignalList<TaskId>; SOFTWARE_INTERRUPT_COUNT] =
    [const { SignalList::new() }; SOFTWARE_INTERRUPT_COUNT];

/// Pend the interrupt-mode executor `SWI` for a raised signal: raise its
/// software interrupt, whose handler services the signal and polls. It
/// wakes no task itself.
fn pend_signal<const SWI: u8>(_: &'static IrqSignal<InterruptExecutor<SWI>>) {
    raise_interrupt(SWI);
}

/// Whether the caller runs in the interrupt-mode executor `SWI`'s context:
/// its handler's poll on this core, not a hardware interrupt preempting it.
fn signal_context<const SWI: u8>() -> bool {
    running() == SWI
        && INTERRUPT_MODE
            .get(usize::from(SWI))
            .is_some_and(|interrupt_mode| interrupt_mode.load(Ordering::Acquire))
}

/// Executor polled in its software interrupt's handler, at that interrupt's
/// level: the radio executor (P2) on `FROM_CPU` line 2.
///
/// Its handler preempts lower levels; a higher level preempts it. Its tasks
/// may start from another context than the one that polls them, so their
/// starts are `Send`; it makes each task's future in its own context.
pub struct InterruptExecutor<const SWI: u8> {
    _line: (),
}

#[allow(unsafe_code, reason = "the executor's signal context contract")]
// SAFETY: `signal_context::<SWI>` is true only while this executor's
// handler polls (`as_running`), at the level it entered at: a hardware
// interrupt that preempts it runs at a higher level and is refused
// (`running`), and the handler never reenters itself on its core;
// `pend_signal::<SWI>` raises that handler's interrupt, which services
// `SIGNAL_LISTS[SWI]` before it polls (`interrupt_handler`). `current`
// names a task only for the waker of the task this executor polls now
// (`CURRENT[SWI]`, set around each poll); `wake` marks the task ready in
// this executor's mask, which the poll that follows takes, never polling it
// in place.
unsafe impl<const SWI: u8> SignalExecutor for InterruptExecutor<SWI> {
    type Task = TaskId;
    fn pend(signal: &'static IrqSignal<Self>) {
        pend_signal::<SWI>(signal);
    }
    fn running() -> bool {
        signal_context::<SWI>()
    }
    fn list() -> &'static SignalList<TaskId> {
        &SIGNAL_LISTS[usize::from(SWI)]
    }
    fn current(cx: &Context<'_>, _: &ContextToken) -> Result<TaskId, ForeignTask> {
        crate::task::signal_current(&CURRENT[usize::from(SWI)], SWI, cx)
    }
    fn wake(task: TaskId, _: &ContextToken) {
        crate::task::signal_wake(task);
    }
}

/// Whether the caller runs in the thread-mode executor `SWI`'s context: its
/// run loop on its core, at thread level, not an interrupt preempting it.
fn thread_context<const SWI: u8>() -> bool {
    running() == SWI
        && INTERRUPT_MODE
            .get(usize::from(SWI))
            .is_some_and(|interrupt_mode| !interrupt_mode.load(Ordering::Acquire))
}

#[allow(unsafe_code, reason = "the executor's signal context contract")]
// SAFETY: `thread_context::<SWI>` is true only while this executor's run
// loop runs as its context (`as_running`), at the level it entered at: one
// context on one core. An interrupt that preempts it runs at a higher level
// and is refused (`running`); a preempting executor enters its own context. `pend` marks the executor's work and wakes its core,
// and the loop services `SIGNAL_LISTS[SWI]` before each poll round. A
// software interrupt serves one executor, so this list is no interrupt
// executor's. `current` names a task only for the waker of the task this
// executor polls now; `wake` marks the task ready in this executor's mask,
// which the round that follows takes, never polling it in place.
unsafe impl<const SWI: u8> SignalExecutor for Executor<SWI> {
    type Task = TaskId;
    fn pend(_: &'static IrqSignal<Self>) {
        pend(SWI);
    }
    fn running() -> bool {
        thread_context::<SWI>()
    }
    fn list() -> &'static SignalList<TaskId> {
        &SIGNAL_LISTS[usize::from(SWI)]
    }
    fn current(cx: &Context<'_>, _: &ContextToken) -> Result<TaskId, ForeignTask> {
        crate::task::signal_current(&CURRENT[usize::from(SWI)], SWI, cx)
    }
    fn wake(task: TaskId, _: &ContextToken) {
        crate::task::signal_wake(task);
    }
}

impl<const SWI: u8> InterruptExecutor<SWI> {
    /// Start polling the tasks `T` in the handler of `wake`'s source, whose
    /// table entry names [`interrupt_handler::<T>`](interrupt_handler) on
    /// this core. `order` proves that this executor's level lies between the
    /// radio interrupts' and the application's. The spawner starts `T`'s
    /// tasks.
    ///
    /// # Panics
    ///
    /// When the table routes the source to another core, or the software
    /// interrupt already serves an executor.
    pub fn start<T, W, High, Low, C>(
        self,
        router: &mut oer_interrupt_table::Router<C>,
        wake: W,
        part: W::Part,
        order: LevelOrder<High, W, Low>,
    ) -> Spawner<T>
    where
        T: Tasks<Mode = InterruptMode>,
        W: Entry,
        C: oer_interrupt_table::Controller,
        High: Entry,
        Low: Entry,
    {
        const {
            assert!(
                <W::Part as oer_interrupt_table::InterruptSource>::SOURCE
                    == software_interrupt_source(SWI) as u16,
                "the token is not this executor's software interrupt"
            );
            assert!(T::EXECUTOR == SWI, "the tasks are another executor's");
        };
        let _ = (order, self._line);
        claim_core(SWI);
        // Published before the source is enabled, so the first handler polls
        // and a pend raises its interrupt.
        INTERRUPT_MODE[usize::from(SWI)].store(true, Ordering::Release);
        assert_eq!(
            W::HART as usize,
            current_core(),
            "executor route is on this hart"
        );
        if let Err(error) = router.bind(wake, part) {
            panic!("executor software interrupt {SWI}: {error:?}");
        }
        Spawner::new()
    }
}

macro_rules! impl_executor_constructors {
    ($number:literal, $block:ident, $take:ident) => {
        impl Executor<$number> {
            /// The executor on software interrupt line `SWI`: it consumes
            /// the line's typed block from the reset root.
            pub fn new(interrupt: platform::$block) -> Self {
                install_line($number, crate::chip::$take(interrupt));
                Self {
                    not_send: PhantomData,
                }
            }
        }

        impl InterruptExecutor<$number> {
            /// The executor on software interrupt line `SWI`: it consumes
            /// the line's typed block from the reset root.
            pub fn new(interrupt: platform::$block) -> Self {
                install_line($number, crate::chip::$take(interrupt));
                Self { _line: () }
            }
        }
    };
}

impl_executor_constructors!(0, FromCpu0, own_line_0);
impl_executor_constructors!(1, FromCpu1, own_line_1);
impl_executor_constructors!(2, FromCpu2, own_line_2);
impl_executor_constructors!(3, FromCpu3, own_line_3);

/// The interrupt-table handler of a thread-mode executor's software interrupt
/// (`FROM_CPU_INTR<SWI>`): it lowers the line, which wakes the executor's
/// core from its wait.
pub fn wake_handler<const SWI: u8>() {
    serve_interrupt(SWI, || ());
}

/// The interrupt-table handler of the interrupt-mode executor of the tasks
/// `T` (its software interrupt, `FROM_CPU_INTR<N>`): it serves the
/// executor's timers and signals and polls one round of its ready tasks.
pub fn interrupt_handler<T: Tasks<Mode = InterruptMode>>() {
    let swi = T::EXECUTOR;
    // The line is lowered before the round takes its work: a pend during the
    // round raises it again and runs another round.
    serve_interrupt(swi, || round::<T>(swi));
}

/// One round of the interrupt-mode executor `swi`.
fn round<T: Tasks<Mode = InterruptMode>>(swi: u8) {
    if !INTERRUPT_MODE[usize::from(swi)].load(Ordering::Acquire) {
        return;
    }
    as_running(swi, || {
        crate::time_driver::service(swi);
        // Raised signals wake their tasks here, in this context, before
        // the poll; a signal raised meanwhile raises this interrupt again.
        match swi {
            0 => oer_interrupt_table::signal::service::<InterruptExecutor<0>>(),
            1 => oer_interrupt_table::signal::service::<InterruptExecutor<1>>(),
            2 => oer_interrupt_table::signal::service::<InterruptExecutor<2>>(),
            _ => oer_interrupt_table::signal::service::<InterruptExecutor<3>>(),
        }
        // A task woken during the round is polled in the next run of this
        // handler, after the timers and signals.
        if poll_tasks::<T>() {
            raise_interrupt(swi);
        }
    });
}

#[inline(always)]
pub(crate) fn mark_work(swi: u8) {
    WORK_PENDING.0[usize::from(swi)].store(1, Ordering::Release);
}

/// Make executor `swi` run again: a thread-mode executor sees its work flag
/// at its next wake, an interrupt-mode executor's handler runs.
pub(crate) fn pend(swi: u8) {
    if INTERRUPT_MODE[usize::from(swi)].load(Ordering::Acquire) {
        raise_interrupt(swi);
        return;
    }
    mark_work(swi);
    let target_core =
        crate::poll_stamp::assigned_core(EXECUTOR_CORE[usize::from(swi)].load(Ordering::Acquire));
    if target_core.is_some_and(|core| core != current_core()) {
        raise_interrupt(swi);
    }
}

/// Pend executor `swi` for a task its ready mask holds. In the executor's
/// own context the round's end sees the mask and needs no pend.
pub(crate) fn pend_task(swi: u8) {
    if running() != swi {
        pend(swi);
    }
}

fn wait_for_work<const SWI: u8>() {
    oer_riscv::interrupt_free(|| {
        if WORK_PENDING.0[usize::from(SWI)].load(Ordering::Acquire) == 0
            && !READY[usize::from(SWI)].any()
        {
            #[cfg(oer_profile)]
            crate::idle_accounting::add_wait(usize::from(SWI));
            #[cfg(not(oer_profile))]
            crate::chip::wait_for_interrupt();
        }
    });
}

/// The core executor `executor` runs on, once it runs.
#[cfg(oer_profile)]
pub(crate) fn executor_core(executor: usize) -> Option<usize> {
    let core = EXECUTOR_CORE[executor].load(Ordering::Acquire);
    crate::poll_stamp::assigned_core(core)
}

/// Raise the software interrupt of the running executor `executor`, waking
/// its core from its wait.
#[cfg(oer_profile)]
pub(crate) fn raise(executor: usize) {
    if executor_core(executor).is_some() {
        raise_interrupt(executor as u8);
    }
}
