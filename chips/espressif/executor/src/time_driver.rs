//! The Embassy time driver: one timer queue and one SYSTIMER comparator per
//! executor, and our own sole writer of the SYSTIMER comparator registers.
//!
//! - Each executor bound with [`bind`] owns a queue that only its own context
//!   touches: a timer is registered while the executor polls the task, and
//!   expired timers are dispatched before it polls. The queue holds one
//!   deadline per task, by the task's index, which the waker names; an
//!   expired deadline marks its task ready. A timer registered from another
//!   context than its task's executor, or with another executor's waker,
//!   wakes the task at once, so it registers again from its own context. No
//!   queue is shared between contexts, and none is locked.
//! - Each bound executor has its own comparator, whose alarm interrupt
//!   ([`alarm_interrupt`]) only acknowledges its own bit in `INT_CLR` (a
//!   write-one-to-clear, no read-modify-write) and pends that executor.
//! - Programming a comparator takes the vendor sequence (ESP-IDF
//!   `systimer_hal_set_alarm_target`: disable the alarm, write the target,
//!   load it, enable the alarm). The alarm enables share the `CONF` word, so
//!   one executor alone, the writer named at [`init`], writes the SYSTIMER
//!   registers. Another executor publishes its next deadline in a
//!   single-writer cell ([`DeadlineCell`]) and pends the writer, which
//!   programs that executor's comparator. A torn read is skipped: the
//!   publisher pends the writer again after every write.
//!
//! [`init`] owns the running timer's blocks from the reset root without
//! resetting its counter or clock. The binding's platform port supplies the
//! source identities.

use core::{marker::PhantomData, task::Waker};

use crate::{Failure, Fault, SoftwareInterrupt};
use oer_memory::{ContextCell, ContextProof, OnceValue};

use embassy_time_driver::Driver;
use oer_espressif_interrupt_table::Entry;
use portable_atomic::{AtomicBool, AtomicU8, Ordering};

use crate::{
    binding::{Binding, TimeDriver},
    chip::{PortOf, SOFTWARE_INTERRUPT_COUNT, alarm_source},
    deadline_cell::DeadlineCell,
    executor::{NO_EXECUTOR, pend, ready, running},
    systimer::{AlarmPart, Comparator, ComparatorWriter, SystemTimer},
    task::TaskId,
    timer_queue::WakeQueue,
};
use oer_espressif_platform_port::ExecutorPlatform;

const COMPARATOR_COUNT: usize = 3;
const NO_COMPARATOR: u8 = u8::MAX;
/// The comparators' 52-bit range.
const TICK_MASK: u64 = 0xF_FFFF_FFFF_FFFF;

fn alarm_slot<T>(comparator: Comparator, entries: &[T; COMPARATOR_COUNT]) -> &T {
    let [zero, one, two] = entries;
    match comparator {
        Comparator::Zero => zero,
        Comparator::One => one,
        Comparator::Two => two,
    }
}
fn alarm_slot_mut<T>(comparator: Comparator, entries: &mut [T; COMPARATOR_COUNT]) -> &mut T {
    let [zero, one, two] = entries;
    match comparator {
        Comparator::Zero => zero,
        Comparator::One => one,
        Comparator::Two => two,
    }
}

macro_rules! timer_observe {
    ($observer:expr, $executor:expr, $phase:ident, $at:expr, $value:expr) => {
        #[cfg(oer_trace)]
        crate::observation::timer::emit(
            &$observer,
            $executor,
            crate::observation::timer::Phase::$phase,
            $at,
            $value,
        );
    };
}

/// One executor's timers. Only that executor's context touches it.
struct Slot {
    queue: WakeQueue,
    /// The writer's view of each comparator it programs for another
    /// executor: the generation of the deadline it programmed.
    programmed: [Option<u32>; COMPARATOR_COUNT],
}

// Private to with_slot: no caller can apply a proof to a different slot.
struct SlotContext(PhantomData<*mut ()>);

#[allow(
    unsafe_code,
    reason = "the slot access proof replaces its Sync promise"
)]
// SAFETY: only with_slot constructs this proof, after checking running() for
// the selected slot. Each executor runs on one core and never concurrently
// with itself; preempting contexts cannot obtain that slot's proof. Slot is
// const-initialized and retained for the image's lifetime.
unsafe impl ContextProof for SlotContext {
    type Sharing = ();
    fn is_current(&self) -> bool {
        true
    }
}

static SLOTS: [ContextCell<Slot, SlotContext>; SOFTWARE_INTERRUPT_COUNT] = [const {
    ContextCell::new(Slot {
        queue: WakeQueue::new(),
        programmed: [None; COMPARATOR_COUNT],
    })
};
    SOFTWARE_INTERRUPT_COUNT];

/// A successful assignment always carries both bounded identities.
#[derive(Clone, Copy)]
struct Assignment {
    comparator: Comparator,
    writer: SoftwareInterrupt,
}
/// No assignment exists before the executor's alarm is bound.
static COMPARATOR_OF: [OnceValue<Assignment>; SOFTWARE_INTERRUPT_COUNT] =
    [const { OnceValue::new() }; SOFTWARE_INTERRUPT_COUNT];
/// An alarm has no receiving executor until its assignment is published.
static EXECUTOR_OF: [OnceValue<SoftwareInterrupt>; COMPARATOR_COUNT] =
    [const { OnceValue::new() }; COMPARATOR_COUNT];
/// The one executor that writes the SYSTIMER registers.
static WRITER: AtomicU8 = AtomicU8::new(NO_EXECUTOR);
/// Each executor's next deadline, published to the writer.
static PUBLISHED: [DeadlineCell; SOFTWARE_INTERRUPT_COUNT] =
    [const { DeadlineCell::new(u64::MAX) }; SOFTWARE_INTERRUPT_COUNT];

static ALARM_FIRED: [AtomicBool; COMPARATOR_COUNT] =
    [const { AtomicBool::new(false) }; COMPARATOR_COUNT];

/// Run `f` on executor `swi`'s slot in its context; refuse a nested borrow.
fn with_slot<B: Binding, R>(swi: SoftwareInterrupt, f: impl FnOnce(&mut Slot) -> R) -> Option<R> {
    if running::<B>() != swi as u8 {
        return None;
    }
    let cell = swi.at(&SLOTS);
    cell.with(&SlotContext(PhantomData), f)
}

use crate::chip::now;

/// The comparator whose alarm interrupt is `source`.
const fn comparator_of_source<P: ExecutorPlatform>(source: u16) -> Option<Comparator> {
    if source == alarm_source::<P>(Comparator::Zero) {
        Some(Comparator::Zero)
    } else if source == alarm_source::<P>(Comparator::One) {
        Some(Comparator::One)
    } else if source == alarm_source::<P>(Comparator::Two) {
        Some(Comparator::Two)
    } else {
        None
    }
}

/// Inputs held by a failed time-driver setup. Only `Failure` owns this
/// payload; no recovery authority is exposed after partial installation.
pub struct TimeSetup<P: ExecutorPlatform, A: Entry> {
    _owners: TimeSetupOwners<P, A>,
}
enum TimeSetupOwners<P: ExecutorPlatform, A: Entry> {
    Counter {
        _timer: SystemTimer<P>,
        _alarm: A,
        _part: A::Part,
    },
    Binding {
        _error: oer_espressif_interrupt_table::BindError<A>,
    },
}

/// A refused time-driver setup of binding `B`.
type SetupFailure<B, A> = Failure<PortOf<B>, TimeSetup<PortOf<B>, A>>;

/// Affine startup authority for binding more alarms after successful init.
/// Its private constructor runs only after counter publication and the writer's
/// first alarm binding succeed. It remains on the initialization hart.
#[must_use = "retain the startup authority while binding additional alarms"]
pub struct TimeDriverReady<B: Binding> {
    writer: SoftwareInterrupt,
    _binding: PhantomData<fn() -> B>,
    _local: PhantomData<*mut ()>,
}

/// Install the time driver over the SoC's system timer: `systimer` is
/// consumed for the rest of the program. Executor `WRITER` serves the
/// comparator of `alarm`'s source and writes the SYSTIMER registers for
/// every executor; bind the others with [`bind`].
///
/// Call it before interrupts are enabled.
///
/// # Errors
/// A stuck counter latch or refused route returns the supplied inputs in
/// quarantine. Duplicate installation records a fault and resets.
pub fn init<B: Binding, const WRITER_SWI: u8, A, C>(
    systimer: SystemTimer<PortOf<B>>,
    router: &mut oer_espressif_interrupt_table::Router<C>,
    alarm: A,
    part: A::Part,
) -> Result<TimeDriverReady<B>, SetupFailure<B, A>>
where
    A: Entry,
    A::Part: AlarmPart,
    C: oer_espressif_interrupt_table::Controller,
{
    let identity = const { SoftwareInterrupt::constant::<WRITER_SWI>() };
    let writer = WRITER.load(Ordering::Acquire);
    if writer != NO_EXECUTOR {
        Fault::TimerInstalled { writer }.reset::<PortOf<B>>();
    }
    let bank = match ComparatorWriter::adopt::<B>(systimer) {
        Ok(bank) => bank,
        Err(failure) => {
            return Err(failure.map_owners(|timer| TimeSetup {
                _owners: TimeSetupOwners::Counter {
                    _timer: timer,
                    _alarm: alarm,
                    _part: part,
                },
            }));
        }
    };
    let storage = B::storage();
    let cell = match storage.bank_storage.try_uninit() {
        Some(cell) => cell,
        None => Fault::TimerInstalled { writer }.reset::<PortOf<B>>(),
    };
    if storage.bank.install(cell.write(bank)).is_err() {
        Fault::TimerInstalled { writer }.reset::<PortOf<B>>();
    }
    if let Err(writer) =
        WRITER.compare_exchange(NO_EXECUTOR, WRITER_SWI, Ordering::AcqRel, Ordering::Acquire)
    {
        Fault::TimerInstalled { writer }.reset::<PortOf<B>>();
    }
    bind_inner::<B, WRITER_SWI, A, C>(identity, router, alarm, part)
        .map(|()| TimeDriverReady {
            writer: identity,
            _binding: PhantomData,
            _local: PhantomData,
        })
        .map_err(|failure| {
            failure.map_owners(|refused| TimeSetup {
                _owners: TimeSetupOwners::Binding { _error: refused },
            })
        })
}

/// Give executor `SWI` the comparator of `alarm`'s source, whose table entry
/// names [`alarm_interrupt`] for that comparator on this core.
///
/// Borrow the authority returned by [`init`], before interrupts are enabled.
///
/// # Errors
/// A refused route returns its token and part in quarantine. Duplicate
/// assignment or an unavailable timer bank records a fault and resets.
pub fn bind<B: Binding, const SWI: u8, A, C>(
    ready: &mut TimeDriverReady<B>,
    router: &mut oer_espressif_interrupt_table::Router<C>,
    alarm: A,
    part: A::Part,
) -> Result<(), Failure<PortOf<B>, oer_espressif_interrupt_table::BindError<A>>>
where
    A: Entry,
    A::Part: AlarmPart,
    C: oer_espressif_interrupt_table::Controller,
{
    bind_inner::<B, SWI, A, C>(ready.writer, router, alarm, part)
}

fn bind_inner<B: Binding, const SWI: u8, A, C>(
    writer: SoftwareInterrupt,
    router: &mut oer_espressif_interrupt_table::Router<C>,
    alarm: A,
    mut part: A::Part,
) -> Result<(), Failure<PortOf<B>, oer_espressif_interrupt_table::BindError<A>>>
where
    A: Entry,
    A::Part: AlarmPart,
    C: oer_espressif_interrupt_table::Controller,
{
    let comparator = const {
        let comparator = comparator_of_source::<PortOf<B>>(
            <A::Part as oer_espressif_interrupt_table::InterruptSource>::SOURCE,
        );
        let Some(comparator) = comparator else {
            panic!("the token is not a SYSTIMER alarm");
        };
        if SWI as usize >= SOFTWARE_INTERRUPT_COUNT {
            panic!("no such executor");
        }
        comparator
    };
    let executor = const { SoftwareInterrupt::constant::<SWI>() };
    if executor
        .at(&COMPARATOR_OF)
        .set(Assignment { comparator, writer })
        .is_err()
    {
        // init creates the only authority; bind borrows it exclusively and
        // neither call yields. A duplicate therefore sees a completed earlier
        // publication. The sentinel also keeps diagnostics nonblocking if a
        // fault interrupts publication before its READY marker.
        let previous = executor
            .at(&COMPARATOR_OF)
            .get()
            .map_or(NO_COMPARATOR, |assignment| assignment.comparator as u8);
        Fault::TimerBinding {
            executor: SWI,
            comparator: comparator as u8,
            previous,
        }
        .reset::<PortOf<B>>();
    }
    if alarm_slot(comparator, &EXECUTOR_OF)
        .set(const { SoftwareInterrupt::constant::<SWI>() })
        .is_err()
    {
        let previous = alarm_slot(comparator, &EXECUTOR_OF)
            .get()
            .map_or(NO_EXECUTOR, |executor| executor as u8);
        Fault::TimerBinding {
            executor: SWI,
            comparator: comparator as u8,
            previous,
        }
        .reset::<PortOf<B>>();
    }
    let actual = crate::chip::current_core::<B>();
    if A::HART as usize != actual {
        return Err(Failure::new(
            Fault::AlarmHart {
                expected: A::HART,
                actual,
            },
            oer_espressif_interrupt_table::BindError::Refused {
                error: oer_espressif_interrupt_table::Error::WrongHart {
                    source: <A::Part as oer_espressif_interrupt_table::InterruptSource>::SOURCE,
                    hart: A::HART,
                    current: actual as u8,
                },
                token: alarm,
                part,
            },
        ));
    }
    if B::storage()
        .bank
        .with(|bank| bank.bind(&mut part))
        .is_none()
    {
        Fault::TimerBankUnavailable {
            comparator: comparator as u8,
        }
        .reset::<PortOf<B>>();
    }
    router
        .bind(alarm, part)
        .map(|_bound| ())
        .map_err(|refused| Failure::new(Fault::binding(SWI, &refused), refused))
}

/// Program `comparator` for `deadline` (microseconds), in the writer's
/// context only. A deadline already past fires at once: the S31 comparator
/// raises its interrupt when enabled with a target behind the counter
/// (ESP-IDF `SYSTIMER_LL_ALARM_MISS_COMPENSATE`).
fn program<B: Binding>(comparator: Comparator, deadline: u64) {
    let ticks = (deadline != u64::MAX).then(|| {
        deadline
            .saturating_mul(PortOf::<B>::TICKS_PER_MICROSECOND)
            .min(TICK_MASK)
    });
    if B::storage()
        .bank
        .with(|bank| bank.program(comparator, ticks))
        .is_none()
    {
        Fault::TimerBankUnavailable {
            comparator: comparator as u8,
        }
        .reset::<PortOf<B>>();
    }
}

/// Mark executor `swi`'s task ready, in its context before it polls.
fn mark(swi: SoftwareInterrupt) -> impl FnMut(u8) {
    let ready = ready(swi as u8);
    move |index| {
        if let Some(ready) = ready {
            ready.mark(index);
        }
    }
}

/// After `slot`'s queue changed: program its comparator (the writer) or
/// publish its next deadline to the writer.
fn rearm<B: Binding, O: oer_trace::Observer>(
    swi: SoftwareInterrupt,
    assignment: Assignment,
    slot: &mut Slot,
    now: u64,
    fired: bool,
    observer: &O,
) {
    let _ = observer;
    let comparator = assignment.comparator;
    let Some(next) = slot.queue.alarm_deadline(fired) else {
        return;
    };
    let writer = assignment.writer;
    #[cfg(oer_trace)]
    crate::observation::timer::emit(
        observer,
        swi,
        if next == u64::MAX {
            crate::observation::timer::Phase::Stop
        } else {
            crate::observation::timer::Phase::Arm
        },
        now,
        next,
    );
    if swi == writer {
        program::<B>(comparator, next);
    } else {
        swi.at(&PUBLISHED).publish(next);
        pend::<B>(writer);
    }
    #[cfg(oer_trace)]
    if next != u64::MAX {
        timer_observe!(observer, swi, Armed, self::now::<B>(), 0);
    }
    let _ = now;
}

/// Serve executor `swi`'s timers, in its context before it polls: dispatch
/// its expired timers and rearm; the writer also programs the comparators
/// whose executors published a new deadline.
pub(crate) fn service<B: Binding, T: crate::Tasks<Binding = B>>(swi: SoftwareInterrupt) {
    let _: PhantomData<T> = PhantomData;
    let Some(assignment) = swi.at(&COMPARATOR_OF).get() else {
        return;
    };
    let comparator = assignment.comparator;
    #[cfg(oer_trace)]
    let observer = T::observer();
    #[cfg(not(oer_trace))]
    let observer = oer_trace::Noop;
    let _ = with_slot::<B, _>(swi, |slot| {
        let fired = alarm_slot(comparator, &ALARM_FIRED).swap(false, Ordering::AcqRel);
        let start = now::<B>();
        timer_observe!(observer, swi, Dispatch, start, u64::from(fired));
        slot.queue.dispatch_expired(start, mark(swi));
        timer_observe!(observer, swi, Dispatched, now::<B>(), 0);
        // Only an actual alarm leaves the comparator idle. A task's self-wake
        // must not disable and program an unchanged comparator on every poll.
        rearm::<B, _>(swi, assignment, slot, start, fired, &observer);
        if swi == assignment.writer {
            program_published::<B>(slot);
        }
    });
}

/// The writer programs every other executor's comparator whose published
/// deadline it has not programmed yet. A torn read is left for the next
/// pass, which the publisher's pend brings.
fn program_published<B: Binding>(writer: &mut Slot) {
    for (swi, (cell, assignment)) in PUBLISHED.iter().zip(COMPARATOR_OF.iter()).enumerate() {
        let Some(assignment) = assignment.get() else {
            continue;
        };
        let comparator = assignment.comparator;
        if swi == assignment.writer.index() {
            continue;
        }
        let Some(published) = cell.read() else {
            continue;
        };
        let programmed = alarm_slot_mut(comparator, &mut writer.programmed);
        if *programmed != Some(published.generation) {
            program::<B>(comparator, published.value);
            *programmed = Some(published.generation);
        }
    }
}

/// The interrupt-table handler of comparator `COMPARATOR`'s alarm
/// (`SYSTIMER_TARGET<COMPARATOR>`), which holds the comparator's alarm part:
/// it acknowledges the alarm and pends the comparator's executor. It writes
/// no other register and touches no queue.
pub fn alarm_interrupt<T: crate::Tasks, const COMPARATOR: u8, P: AlarmPart>(part: &mut P)
where
    T::Binding: Binding,
{
    const {
        if P::COMPARATOR as u8 != COMPARATOR {
            panic!("the part is another comparator's");
        }
    };
    #[cfg(oer_trace)]
    let entered = now::<T::Binding>();
    part.acknowledge();
    #[cfg(oer_trace)]
    if let Some(executor) = alarm_slot(P::COMPARATOR, &EXECUTOR_OF).get() {
        timer_observe!(
            T::observer(),
            executor,
            Interrupt,
            entered,
            now::<T::Binding>()
        );
    }
    alarm_slot(P::COMPARATOR, &ALARM_FIRED).store(true, Ordering::Release);
    if let Some(executor) = alarm_slot(P::COMPARATOR, &EXECUTOR_OF).get() {
        pend::<T::Binding>(executor);
    }
}

impl<B: Binding, O: oer_trace::Observer + Send + Sync + 'static> Driver for TimeDriver<B, O> {
    #[inline]
    fn now(&self) -> u64 {
        now::<B>()
    }

    fn schedule_wake(&self, at: u64, waker: &Waker) {
        let Some(task) = TaskId::of::<B>(waker) else {
            waker.wake_by_ref();
            return;
        };
        let swi = task.executor();
        let Some(assignment) = swi.at(&COMPARATOR_OF).get() else {
            Fault::MissingComparator {
                executor: swi as u8,
                comparator: NO_COMPARATOR,
            }
            .reset::<PortOf<B>>();
        };
        let registered = with_slot::<B, _>(swi, |slot| {
            timer_observe!(self.observer, swi, Registration, now::<B>(), at);
            if let Some(now) = slot
                .queue
                .schedule_wake(at, task.index(), now::<B>, mark(swi))
            {
                #[cfg(oer_trace)]
                let observer = &self.observer;
                #[cfg(not(oer_trace))]
                let observer = &oer_trace::Noop;
                rearm::<B, _>(swi, assignment, slot, now, false, observer);
            }
        });
        if registered.is_none() {
            // A foreign context or enclosing slot borrow refused registration.
            // The task polls again after that access ends and registers there.
            waker.wake_by_ref();
        }
    }
}
