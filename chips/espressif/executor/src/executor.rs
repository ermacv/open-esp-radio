use crate::SoftwareInterrupt;
use core::{marker::PhantomData, task::Context};
use oer_espressif_platform_port::ExecutorPlatform;
use oer_memory::signal::SignalList;

use crate::urgent_sources::UrgentSources;
use crate::{Failure, Fault};
use oer_espressif_interrupt_table::{
    Entry,
    signal::{ContextToken, ForeignTask, IrqSignal, SignalExecutor},
};
use oer_espressif_staged_runtime::Hart;
use oer_memory::{ExecutionContext, OnceValue};
use portable_atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

use crate::binding::Binding;
use crate::chip::{
    PortOf, SOFTWARE_INTERRUPT_COUNT, current_core, install_line, interrupt_level, raise_interrupt,
    serve_interrupt, software_interrupt_source,
};
pub(crate) use crate::context::NO_EXECUTOR;
use crate::context::Running;
use crate::poll_stamp::ChipStamp;
use crate::task::{Current, Interrupt as InterruptMode, Ready, Spawner, TaskId, Tasks, Thread};

use oer_espressif_platform_port::HARTS;
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

/// The registered hart in geometry-derived low bits, plus the current task
/// poll timestamp in the remaining bits. All low bits set mean unassigned.
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
static RUNNING: [Running; HARTS] = [const { Running::new() }; HARTS];

oer_memory::zeroed_static! {
    /// Composition-owned urgent sources, published before an executor can poll.
    /// Placement: interrupt executors read this list reference from critical SRAM.
    #[used]
    static URGENT: [OnceValue<&'static [u16]>; SOFTWARE_INTERRUPT_COUNT] =
        zeroed in ".critical.bss.executor";
}

oer_espressif_staged_runtime::placed_static! {
    /// The task each executor polls now; only that executor's context reads
    /// it, to name a polling task for its signals. Executors of different
    /// harts write neighbouring entries.
    static CURRENT: [Current; SOFTWARE_INTERRUPT_COUNT] =
        [const { Current::new() }; SOFTWARE_INTERRUPT_COUNT], in critical_data "executor";
}

oer_espressif_staged_runtime::placed_static! {
    /// Whether each software interrupt serves a started interrupt-mode
    /// executor; a pend reads it on any hart.
    static INTERRUPT_MODE: [AtomicBool; SOFTWARE_INTERRUPT_COUNT] =
        [const { AtomicBool::new(false) }; SOFTWARE_INTERRUPT_COUNT], in critical_data "executor";
}

/// The executor whose context the caller runs in: [`NO_EXECUTOR`] outside
/// every executor and in a hardware interrupt that preempts one.
pub(crate) fn running<B: Binding>() -> u8 {
    crate::chip::current_hart::<B>()
        .at(&RUNNING)
        .at(interrupt_level())
}

/// [`running`] for the execution contexts, which name no binding: an
/// architectural ID that names no hart runs no executor's context. Every
/// round enters its context through [`as_running`], whose hart check
/// records that ID and resets first.
fn running_context() -> u8 {
    crate::chip::checked_hart().map_or(NO_EXECUTOR, |hart| hart.at(&RUNNING).at(interrupt_level()))
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
/// Outside a started polling context, records `OutsideExecutor` and resets.
pub fn ready_peers<B: Binding>() -> bool {
    READY
        .get(usize::from(running::<B>()))
        .unwrap_or_else(|| {
            Fault::OutsideExecutor {
                running: running::<B>(),
                level: interrupt_level(),
            }
            .reset::<PortOf<B>>()
        })
        .any()
}

// Keep the clock and timestamp publication out of the poll round.
#[inline(never)]
fn stamp_poll<B: Binding>(swi: u8) {
    let state = &EXECUTOR_CORE[usize::from(swi)];
    let now = crate::chip::now::<B>();
    state.store(
        ChipStamp::poll_stamp(state.load(Ordering::Relaxed), now),
        Ordering::Release,
    );
}

// Admission is shared by the task wrapper and all PHY leaf boundaries.
#[inline(never)]
pub(crate) fn should_yield<B: Binding>() -> bool {
    let identity = SoftwareInterrupt::from_index(running::<B>()).unwrap_or_else(|| {
        Fault::OutsideExecutor {
            running: running::<B>(),
            level: interrupt_level(),
        }
        .reset::<PortOf<B>>()
    });
    let state = identity.at(&EXECUTOR_CORE);
    identity.at(&READY).any()
        || crate::chip::is_pending::<B>(software_interrupt_source::<PortOf<B>>(identity))
        || identity.at(&URGENT).get().is_some_and(|sources| {
            crate::urgent_sources::pending(sources, crate::chip::is_pending::<B>)
        })
        || signal_pending::<B>(identity)
        || {
            let now = crate::chip::now::<B>();
            ChipStamp::poll_elapsed(state.load(Ordering::Acquire), now)
                >= crate::poll_stamp::POLL_ADMISSION_MICROS
        }
}
fn signal_pending<B: Binding>(swi: SoftwareInterrupt) -> bool {
    use oer_espressif_interrupt_table::signal::has_pending;
    let interrupt_mode = swi.at(&INTERRUPT_MODE).load(Ordering::Acquire);
    let pending = match (interrupt_mode, swi) {
        (true, SoftwareInterrupt::Zero) => has_pending::<InterruptExecutor<B, 0>>(),
        (true, SoftwareInterrupt::One) => has_pending::<InterruptExecutor<B, 1>>(),
        (true, SoftwareInterrupt::Two) => has_pending::<InterruptExecutor<B, 2>>(),
        (true, SoftwareInterrupt::Three) => has_pending::<InterruptExecutor<B, 3>>(),
        (false, SoftwareInterrupt::Zero) => has_pending::<Executor<B, 0>>(),
        (false, SoftwareInterrupt::One) => has_pending::<Executor<B, 1>>(),
        (false, SoftwareInterrupt::Two) => has_pending::<Executor<B, 2>>(),
        (false, SoftwareInterrupt::Three) => has_pending::<Executor<B, 3>>(),
    };
    match pending {
        Ok(pending) => pending,
        Err(_) => Fault::SignalContext {
            executor: swi as u8,
            running: running::<B>(),
        }
        .reset::<PortOf<B>>(),
    }
}

/// Run `poll` as executor `swi`'s context, at the caller's interrupt level,
/// and restore the preempted one.
fn as_running<B: Binding, T: Tasks<Binding = B>>(swi: u8, poll: impl FnOnce()) {
    let _: PhantomData<T> = PhantomData;
    let hart = crate::chip::current_hart::<B>();
    #[cfg(oer_trace)]
    let observer = T::observer();
    #[cfg(oer_trace)]
    oer_trace::observe!(
        observer,
        crate::observation::Activity::new(
            crate::chip::now::<B>() as u32,
            swi,
            hart.get() as u8,
            interrupt_level(),
            crate::observation::Phase::WorkStart
        )
    );
    hart.at(&RUNNING).enter(swi, interrupt_level(), poll);
    #[cfg(oer_trace)]
    oer_trace::observe!(
        observer,
        crate::observation::Activity::new(
            crate::chip::now::<B>() as u32,
            swi,
            hart.get() as u8,
            interrupt_level(),
            crate::observation::Phase::WorkEnd
        )
    );
}

/// One poll round of executor `T::EXECUTOR`, in its context: whether a task
/// is ready again.
fn poll_tasks<B: Binding, T: Tasks<Binding = B>, R: crate::task::Roots>(roots: &mut R) -> bool {
    let swi = T::EXECUTOR;
    crate::task::poll_round::<T, R>(
        &READY[usize::from(swi)],
        &CURRENT[usize::from(swi)],
        roots,
        || stamp_poll::<B>(swi),
    )
}

/// Claim executor `swi` for the current core.
///
/// A duplicate claim records its original poll stamp and resets.
fn claim_core<B: Binding>(swi: u8) {
    EXECUTOR_CORE[usize::from(swi)]
        .compare_exchange(
            UNASSIGNED_CORE,
            current_core::<B>(),
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .unwrap_or_else(|stamp| {
            Fault::ExecutorClaimed {
                executor: swi,
                stamp,
            }
            .reset::<PortOf<B>>()
        });
}

/// Scheduler-free thread-mode executor of the chip binding `B`.
///
/// One software interrupt is reserved per executor; its task wakers remain
/// on the hart that owns it.
pub struct Executor<B, const SWI: u8> {
    not_send: PhantomData<*mut ()>,
    binding: PhantomData<fn() -> B>,
}

impl<B: Binding, const SWI: u8> Executor<B, SWI> {
    /// Run the tasks `T` and `roots` on the calling core, woken through the
    /// executor's software interrupt: `wake` is that source's token of the
    /// image's interrupt table, whose entry names
    /// [`wake_handler::<B, SWI, _>`](wake_handler) on this core. `init`
    /// starts the first tasks; every root is ready for the first round
    /// ([`Roots`](crate::task::Roots), `()` for none).
    /// `urgent` comes from the composition's doorbell constructor, or `()`
    /// when it has no urgent sources.
    ///
    /// # Errors
    /// A refused route returns its token and part in quarantine. A foreign
    /// hart's entry fails at compile time; duplicate startup records a fault.
    #[allow(
        clippy::too_many_arguments,
        reason = "each is one capability the run consumes"
    )]
    pub fn run<T, R: crate::task::Roots, W, C, const H: u8, U: UrgentSources>(
        self,
        _hart: &Hart<H>,
        router: &mut oer_espressif_interrupt_table::Router<C>,
        wake: W,
        part: W::Part,
        _urgent: U,
        mut roots: R,
        init: impl FnOnce(&Spawner<T>),
    ) -> Result<
        core::convert::Infallible,
        Failure<PortOf<B>, oer_espressif_interrupt_table::BindError<W>>,
    >
    where
        Self: SignalExecutor,
        T: Tasks<Binding = B, Mode = Thread>,
        W: Entry,
        W::Part: oer_espressif_interrupt_table::InterruptSource,
        C: oer_espressif_interrupt_table::Controller,
    {
        const {
            if <W::Part as oer_espressif_interrupt_table::InterruptSource>::SOURCE
                != software_interrupt_source::<PortOf<B>>(SoftwareInterrupt::constant::<SWI>())
            {
                panic!("the token is not this executor's software interrupt");
            }
            if T::EXECUTOR != SWI {
                panic!("the tasks are another executor's");
            }
        };
        crate::hart_binding::check::<H, W>();
        let _ = (self.not_send, self.binding);
        let _bound = router
            .bind(wake, part)
            .map_err(|refused| Failure::new(Fault::binding(SWI, &refused), refused))?;
        claim_core::<B>(SWI);
        URGENT[usize::from(SWI)]
            .set(U::SOURCES)
            .unwrap_or_else(|_| Fault::UrgentPublished { executor: SWI }.reset::<PortOf<B>>());
        init(&Spawner::new());
        let ready = const { SoftwareInterrupt::constant::<SWI>() }.at(&READY);
        for root in T::COUNT..crate::task::root_end::<T, R>() {
            ready.mark(root);
        }
        loop {
            WORK_PENDING.0[usize::from(SWI)].store(0, Ordering::Release);
            as_running::<B, T>(SWI, || {
                crate::time_driver::service::<B, T>(const { SoftwareInterrupt::constant::<SWI>() });
                // Raised signals wake their tasks before the round; a signal
                // raised meanwhile marks the work flag again.
                oer_espressif_interrupt_table::signal::service::<Self>();
                let _ = poll_tasks::<B, T, R>(&mut roots);
            });
            wait_for_work::<B, T, SWI>();
        }
    }
}

/// Whether the caller runs in the interrupt-mode executor `SWI`'s context:
/// its handler's poll on this core, not a hardware interrupt preempting it.
fn signal_context<const SWI: u8>() -> bool {
    running_context() == SWI
        && INTERRUPT_MODE
            .get(usize::from(SWI))
            .is_some_and(|interrupt_mode| interrupt_mode.load(Ordering::Acquire))
}

/// Executor of the chip binding `B` polled in its software interrupt's
/// handler, at that interrupt's level: the radio executor (P2) on
/// `FROM_CPU` line 2.
///
/// Its handler preempts lower levels; a higher level preempts it. Its tasks
/// may start from another context than the one that polls them, so their
/// starts are `Send`; it makes each task's future in its own context.
pub struct InterruptExecutor<B, const SWI: u8> {
    _line: PhantomData<fn() -> B>,
}

/// The memory context of the interrupt-mode executor on software interrupt
/// `SWI`, whatever its binding: an image links one binding, so its signal
/// list is one static.
pub struct InterruptContext<const SWI: u8>(());

/// The memory context of the thread-mode executor on software interrupt
/// `SWI`, whatever its binding.
pub struct ThreadContext<const SWI: u8>(());

#[allow(unsafe_code, reason = "the executor's signal context contract")]
// SAFETY: signal_context is true only during this executor's polling
// handler on its claimed hart. running_context() excludes preempting
// higher-level handlers and other harts; the handler cannot reenter itself.
// Each concrete SWI type therefore admits one non-concurrent memory context.
unsafe impl<const SWI: u8> ExecutionContext for InterruptContext<SWI> {
    type Sharing = ();
    fn running() -> bool {
        signal_context::<SWI>()
    }
}

/// Whether the caller runs in the thread-mode executor `SWI`'s context: its
/// run loop on its core, at thread level, not an interrupt preempting it.
fn thread_context<const SWI: u8>() -> bool {
    running_context() == SWI
        && INTERRUPT_MODE
            .get(usize::from(SWI))
            .is_some_and(|interrupt_mode| !interrupt_mode.load(Ordering::Acquire))
}

#[allow(unsafe_code, reason = "the executor's signal context contract")]
// SAFETY: thread_context is true only in this executor's run loop on
// its claimed hart. running_context() excludes preempting handlers, other
// executors and other harts. One owned software line starts this one
// non-concurrent polling context; no scheduling assumption supplies its
// memory exclusion.
unsafe impl<const SWI: u8> ExecutionContext for ThreadContext<SWI> {
    type Sharing = ();
    fn running() -> bool {
        thread_context::<SWI>()
    }
}

impl<B: Binding, const SWI: u8> InterruptExecutor<B, SWI> {
    /// Start polling the tasks `T` in the handler of `wake`'s source, whose
    /// table entry names [`Round<T>`](Round) on this core: an entry with
    /// another handler, or another set's round, fails to compile. Signal declarations and request bindings check their
    /// direction from the table entries. The spawner starts `T`'s tasks.
    /// `urgent` comes from the composition's doorbell constructor, or `()`
    /// when it has no urgent sources.
    ///
    /// # Errors
    /// A refused route returns its token and part in quarantine. A foreign
    /// hart's entry fails at compile time; duplicate startup records a fault.
    pub fn start<T, W, C, const H: u8, U: UrgentSources>(
        self,
        _hart: &Hart<H>,
        router: &mut oer_espressif_interrupt_table::Router<C>,
        wake: W,
        part: W::Part,
        _urgent: U,
    ) -> Result<Spawner<T>, Failure<PortOf<B>, oer_espressif_interrupt_table::BindError<W>>>
    where
        T: Tasks<Binding = B, Mode = InterruptMode>,
        W: oer_espressif_interrupt_table::ExecutorEntry<Handler = Round<T>>,
        W::Part: oer_espressif_interrupt_table::InterruptSource,
        C: oer_espressif_interrupt_table::Controller,
    {
        const {
            if <W::Part as oer_espressif_interrupt_table::InterruptSource>::SOURCE
                != software_interrupt_source::<PortOf<B>>(SoftwareInterrupt::constant::<SWI>())
            {
                panic!("the token is not this executor's software interrupt");
            }
            if T::EXECUTOR != SWI {
                panic!("the tasks are another executor's");
            }
        };
        crate::hart_binding::check::<H, W>();
        let _ = self._line;
        claim_core::<B>(SWI);
        URGENT[usize::from(SWI)]
            .set(U::SOURCES)
            .unwrap_or_else(|_| Fault::UrgentPublished { executor: SWI }.reset::<PortOf<B>>());
        // Published before the source is enabled, so the first handler polls
        // and a pend raises its interrupt.
        INTERRUPT_MODE[usize::from(SWI)].store(true, Ordering::Release);
        let _bound = router
            .bind(wake, part)
            .map_err(|refused| Failure::new(Fault::binding(SWI, &refused), refused))?;
        Ok(Spawner::new())
    }
}

macro_rules! impl_executor_constructors {
    ($number:literal, $block:ident, $take:ident) => {
        impl<B: Binding> SignalExecutor for InterruptExecutor<B, $number> {
            type Task = TaskId;
            type Context = InterruptContext<$number>;
            const SOURCE: u16 =
                software_interrupt_source::<PortOf<B>>(SoftwareInterrupt::constant::<$number>());
            const INTERRUPT_MODE: bool = true;
            fn pend(_: &'static IrqSignal<Self>) {
                raise_interrupt::<B>(const { SoftwareInterrupt::constant::<$number>() });
            }
            fn list() -> &'static SignalList<TaskId, Self::Context> {
                static LIST: SignalList<TaskId, InterruptContext<$number>> = SignalList::new();
                &LIST
            }
            fn current(
                cx: &Context<'_>,
                _: &ContextToken<Self::Context>,
            ) -> Result<TaskId, ForeignTask> {
                crate::task::signal_current::<B>(&CURRENT[$number], $number, cx)
            }
            fn wake(task: TaskId, _: &ContextToken<Self::Context>) {
                crate::task::signal_wake(task);
            }
        }
        impl<B: Binding> SignalExecutor for Executor<B, $number> {
            type Task = TaskId;
            type Context = ThreadContext<$number>;
            const SOURCE: u16 =
                software_interrupt_source::<PortOf<B>>(SoftwareInterrupt::constant::<$number>());
            const INTERRUPT_MODE: bool = false;
            fn pend(_: &'static IrqSignal<Self>) {
                pend::<B>(const { SoftwareInterrupt::constant::<$number>() });
            }
            fn list() -> &'static SignalList<TaskId, Self::Context> {
                static LIST: SignalList<TaskId, ThreadContext<$number>> = SignalList::new();
                &LIST
            }
            fn current(
                cx: &Context<'_>,
                _: &ContextToken<Self::Context>,
            ) -> Result<TaskId, ForeignTask> {
                crate::task::signal_current::<B>(&CURRENT[$number], $number, cx)
            }
            fn wake(task: TaskId, _: &ContextToken<Self::Context>) {
                crate::task::signal_wake(task);
            }
        }
        impl<B: Binding> Executor<B, $number> {
            /// The executor on software interrupt line `SWI`: it consumes
            /// the line's typed block from the reset root.
            pub fn new(interrupt: <PortOf<B> as ExecutorPlatform>::$block) -> Self {
                install_line::<B>(
                    const { SoftwareInterrupt::constant::<$number>() },
                    crate::chip::$take::<B>(interrupt),
                );
                Self {
                    not_send: PhantomData,
                    binding: PhantomData,
                }
            }
        }

        impl<B: Binding> InterruptExecutor<B, $number> {
            /// The executor on software interrupt line `SWI`: it consumes
            /// the line's typed block from the reset root.
            pub fn new(interrupt: <PortOf<B> as ExecutorPlatform>::$block) -> Self {
                install_line::<B>(
                    const { SoftwareInterrupt::constant::<$number>() },
                    crate::chip::$take::<B>(interrupt),
                );
                Self { _line: PhantomData }
            }
        }
    };
}

impl_executor_constructors!(0, Line0, own_line_0);
impl_executor_constructors!(1, Line1, own_line_1);
impl_executor_constructors!(2, Line2, own_line_2);
impl_executor_constructors!(3, Line3, own_line_3);

/// The interrupt-table handler of a thread-mode executor's software interrupt
/// (`FROM_CPU_INTR<SWI>`), which holds the line's interrupt part: it lowers
/// the line, which wakes the executor's core from its wait.
pub fn wake_handler<
    B: Binding,
    const SWI: u8,
    P: oer_espressif_interrupt_table::InterruptSource,
>(
    _: &mut P,
) {
    const {
        if P::SOURCE != software_interrupt_source::<PortOf<B>>(SoftwareInterrupt::constant::<SWI>())
        {
            panic!("the part is another line's");
        }
    };
    serve_interrupt::<B, _>(const { SoftwareInterrupt::constant::<SWI>() }, || ());
}

/// The handler type the interrupt-table entry of the interrupt-mode executor
/// of the tasks `T` names (its software interrupt, `FROM_CPU_INTR<N>`, whose
/// line's interrupt part the entry holds): each run serves the executor's
/// timers and signals and polls one round of its ready tasks. The entry's
/// token states it ([`oer_espressif_interrupt_table::ExecutorEntry`]), and
/// [`InterruptExecutor::start`] binds only the entry whose handler is
/// `Round<T>` for the tasks `T` it starts, checked at compile time. It is a
/// type, never constructed.
pub struct Round<T>(PhantomData<fn() -> T>);

impl<T, P> oer_espressif_interrupt_table::ExecutorHandler<P> for Round<T>
where
    T: Tasks<Mode = InterruptMode>,
    T::Binding: Binding,
    P: oer_espressif_interrupt_table::InterruptSource,
{
    fn handle(part: &mut P) {
        interrupt_handler::<T, P>(part);
    }
}

/// One run of the handler of the interrupt-mode executor of the tasks `T`.
fn interrupt_handler<
    T: Tasks<Mode = InterruptMode>,
    P: oer_espressif_interrupt_table::InterruptSource,
>(
    _: &mut P,
) where
    T::Binding: Binding,
{
    let swi = const {
        match SoftwareInterrupt::from_index(T::EXECUTOR) {
            Some(swi) => swi,
            None => panic!("software interrupt is out of range"),
        }
    };
    // The line is lowered before the round takes its work: a pend during the
    // round raises it again and runs another round.
    serve_interrupt::<T::Binding, _>(swi, || round::<T::Binding, T>(swi));
}

/// One round of the interrupt-mode executor `swi`.
fn round<B: Binding, T: Tasks<Binding = B, Mode = InterruptMode>>(swi: SoftwareInterrupt) {
    use oer_espressif_interrupt_table::signal::service;
    if !swi.at(&INTERRUPT_MODE).load(Ordering::Acquire) {
        return;
    }
    as_running::<B, T>(swi as u8, || {
        crate::time_driver::service::<B, T>(swi);
        // Raised signals wake their tasks here, in this context, before
        // the poll; a signal raised meanwhile raises this interrupt again.
        match swi {
            SoftwareInterrupt::Zero => service::<InterruptExecutor<B, 0>>(),
            SoftwareInterrupt::One => service::<InterruptExecutor<B, 1>>(),
            SoftwareInterrupt::Two => service::<InterruptExecutor<B, 2>>(),
            SoftwareInterrupt::Three => service::<InterruptExecutor<B, 3>>(),
        }
        // A task woken during the round is polled in the next run of this
        // handler, after the timers and signals.
        if poll_tasks::<B, T, ()>(&mut ()) {
            raise_interrupt::<B>(swi);
        }
    });
}

/// Make executor `swi` run again: a thread-mode executor sees its work flag
/// at its next wake, an interrupt-mode executor's handler runs.
pub(crate) fn pend<B: Binding>(swi: SoftwareInterrupt) {
    let mode = swi.at(&INTERRUPT_MODE);
    let work = swi.at(&WORK_PENDING.0);
    let core = swi.at(&EXECUTOR_CORE);
    if mode.load(Ordering::Acquire) {
        raise_interrupt::<B>(swi);
        return;
    }
    work.store(1, Ordering::Release);
    let target_core = ChipStamp::assigned_core(core.load(Ordering::Acquire));
    if target_core.is_some_and(|core| core != current_core::<B>()) {
        raise_interrupt::<B>(swi);
    }
}

/// Pend executor `swi` for a task its ready mask holds. In the executor's
/// own context the round's end sees the mask and needs no pend.
pub(crate) fn pend_task<B: Binding>(swi: SoftwareInterrupt) {
    if running::<B>() != swi as u8 {
        pend::<B>(swi);
    }
}

fn wait_for_work<B: Binding, T: Tasks<Binding = B>, const SWI: u8>() {
    oer_riscv::interrupt_free(|| {
        if WORK_PENDING.0[usize::from(SWI)].load(Ordering::Acquire) == 0
            && !READY[usize::from(SWI)].any()
        {
            crate::chip::wait_for_interrupt::<B, T>();
        }
    });
}
