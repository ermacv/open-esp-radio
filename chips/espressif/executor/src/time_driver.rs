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
//! resetting its counter or clock. The HAL supplies the source identities.

use core::{cell::UnsafeCell, task::Waker};

use embassy_time_driver::Driver;
use oer_interrupt_table::Entry;
use portable_atomic::{AtomicBool, AtomicU8, Ordering};

use crate::{
    chip::{SOFTWARE_INTERRUPT_COUNT, alarm_source},
    deadline_cell::DeadlineCell,
    executor::{NO_EXECUTOR, pend, ready, running},
    systimer::{AlarmPart, Comparator, ComparatorWriter},
    task::TaskId,
    timer_queue::WakeQueue,
};

// The boot context hands the bank to its one writer executor. Each alarm
// handler holds its own comparator's alarm part and never borrows BANK.
static BANK_STORAGE: static_cell::StaticCell<ComparatorWriter> = static_cell::StaticCell::new();
static BANK: oer_memory::HandlerCell<ComparatorWriter> = oer_memory::HandlerCell::new();

const COMPARATOR_COUNT: usize = 3;
const NO_COMPARATOR: u8 = u8::MAX;
/// The comparators' 52-bit range.
const TICK_MASK: u64 = 0xF_FFFF_FFFF_FFFF;

/// One executor's timers. Only that executor's context touches it.
struct Slot {
    queue: WakeQueue,
    /// The writer's view of each comparator it programs for another
    /// executor: the generation of the deadline it programmed.
    programmed: [Option<u32>; COMPARATOR_COUNT],
    #[cfg(oer_profile)]
    observation: crate::timer_observation::Recorder,
}

struct SlotCell(UnsafeCell<Slot>);

#[allow(
    unsafe_code,
    reason = "a slot is touched only from its executor's context"
)]
// SAFETY: `with_slot` hands out a slot only to the context of the executor
// that owns it (`running()`), which polls on one core and does not reenter
// itself; a higher context that preempts it uses its own slot.
unsafe impl Sync for SlotCell {}

static SLOTS: [SlotCell; SOFTWARE_INTERRUPT_COUNT] = [const {
    SlotCell(UnsafeCell::new(Slot {
        queue: WakeQueue::new(),
        programmed: [None; COMPARATOR_COUNT],
        #[cfg(oer_profile)]
        observation: crate::timer_observation::Recorder::new(),
    }))
}; SOFTWARE_INTERRUPT_COUNT];

/// Each executor's comparator, or [`NO_COMPARATOR`].
static COMPARATOR_OF: [AtomicU8; SOFTWARE_INTERRUPT_COUNT] =
    [const { AtomicU8::new(NO_COMPARATOR) }; SOFTWARE_INTERRUPT_COUNT];
/// Each comparator's executor, or [`NO_EXECUTOR`].
static EXECUTOR_OF: [AtomicU8; COMPARATOR_COUNT] =
    [const { AtomicU8::new(NO_EXECUTOR) }; COMPARATOR_COUNT];
/// The one executor that writes the SYSTIMER registers.
static WRITER: AtomicU8 = AtomicU8::new(NO_EXECUTOR);
/// Each executor's next deadline, published to the writer.
static PUBLISHED: [DeadlineCell; SOFTWARE_INTERRUPT_COUNT] =
    [const { DeadlineCell::new(u64::MAX) }; SOFTWARE_INTERRUPT_COUNT];

#[cfg(oer_profile)]
/// Each comparator's last alarm entry and acknowledgement, in microseconds.
static ALARM_AT: [(DeadlineCell, DeadlineCell); COMPARATOR_COUNT] =
    [const { (DeadlineCell::new(0), DeadlineCell::new(0)) }; COMPARATOR_COUNT];
static ALARM_FIRED: [AtomicBool; COMPARATOR_COUNT] =
    [const { AtomicBool::new(false) }; COMPARATOR_COUNT];

/// Run `f` on executor `swi`'s slot, when `swi`'s context is running.
fn with_slot<R>(swi: u8, f: impl FnOnce(&mut Slot) -> R) -> Option<R> {
    if running() != swi {
        return None;
    }
    let cell = SLOTS.get(swi as usize)?;
    #[allow(
        unsafe_code,
        reason = "a slot is touched only from its executor's context"
    )]
    // SAFETY: the running context is executor `swi`'s, the only one that
    // reaches this slot, and no caller of `with_slot` calls it again inside
    // `f`.
    let slot = unsafe { &mut *cell.0.get() };
    Some(f(slot))
}

use crate::chip::now;

/// The comparator whose alarm interrupt is `source`.
const fn comparator_of_source(source: u16) -> u8 {
    if source as u16 == alarm_source(0) {
        0
    } else if source as u16 == alarm_source(1) {
        1
    } else if source as u16 == alarm_source(2) {
        2
    } else {
        NO_COMPARATOR
    }
}

/// Install the time driver over the SoC's system timer: `systimer` is
/// consumed for the rest of the program. Executor `WRITER` serves the
/// comparator of `alarm`'s source and writes the SYSTIMER registers for
/// every executor; bind the others with [`bind`].
///
/// Call it before interrupts are enabled.
///
/// # Panics
///
/// When the time driver was installed already, or the table routes the
/// source to another core.
pub fn init<const WRITER_SWI: u8, A, C>(
    systimer: crate::systimer::SystemTimer,
    router: &mut oer_interrupt_table::Router<C>,
    alarm: A,
    part: A::Part,
) where
    A: Entry,
    A::Part: AlarmPart,
    C: oer_interrupt_table::Controller,
{
    let bank = ComparatorWriter::adopt(systimer);
    assert!(
        BANK.install(BANK_STORAGE.init(bank)).is_ok(),
        "the SYSTIMER bank owner is already installed"
    );
    assert!(
        WRITER
            .compare_exchange(NO_EXECUTOR, WRITER_SWI, Ordering::AcqRel, Ordering::Acquire)
            .is_ok(),
        "the time driver is installed already"
    );
    bind::<WRITER_SWI, A, C>(router, alarm, part);
}

/// Give executor `SWI` the comparator of `alarm`'s source, whose table entry
/// names [`alarm_interrupt`] for that comparator on this core.
///
/// Call it after [`init`] and before interrupts are enabled.
///
/// # Panics
///
/// When the executor or the comparator is bound already, or the table routes
/// the source to another core.
pub fn bind<const SWI: u8, A, C>(
    router: &mut oer_interrupt_table::Router<C>,
    alarm: A,
    mut part: A::Part,
) where
    A: Entry,
    A::Part: AlarmPart,
    C: oer_interrupt_table::Controller,
{
    let comparator = const {
        let comparator =
            comparator_of_source(<A::Part as oer_interrupt_table::InterruptSource>::SOURCE);
        assert!(
            comparator != NO_COMPARATOR,
            "the token is not a SYSTIMER alarm"
        );
        assert!(
            (SWI as usize) < SOFTWARE_INTERRUPT_COUNT,
            "no such executor"
        );
        comparator
    };
    assert!(
        WRITER.load(Ordering::Acquire) != NO_EXECUTOR,
        "the time driver is not installed"
    );
    assert!(
        COMPARATOR_OF[SWI as usize]
            .compare_exchange(
                NO_COMPARATOR,
                comparator,
                Ordering::AcqRel,
                Ordering::Acquire
            )
            .is_ok()
            && EXECUTOR_OF[comparator as usize]
                .compare_exchange(NO_EXECUTOR, SWI, Ordering::AcqRel, Ordering::Acquire)
                .is_ok(),
        "executor {SWI} or comparator {comparator} is bound already"
    );
    // Setup still owns the bank and the alarm part before this alarm's
    // route is enabled.
    assert!(
        BANK.with(|bank| bank.bind(&mut part)).is_some(),
        "the SYSTIMER bank is installed before binding"
    );
    assert_eq!(
        A::HART as usize,
        crate::chip::current_core(),
        "alarm route is on this hart"
    );
    if let Err(error) = router.bind(alarm, part) {
        panic!("time driver alarm {comparator}: {error:?}");
    }
}

/// Program `comparator` for `deadline` (microseconds), in the writer's
/// context only. A deadline already past fires at once: the S31 comparator
/// raises its interrupt when enabled with a target behind the counter
/// (ESP-IDF `SYSTIMER_LL_ALARM_MISS_COMPENSATE`).
fn program(comparator: Comparator, deadline: u64) {
    let ticks = (deadline != u64::MAX).then(|| {
        deadline
            .saturating_mul(crate::chip::TICKS_PER_MICROSECOND)
            .min(TICK_MASK)
    });
    assert!(
        BANK.with(|bank| bank.program(comparator, ticks)).is_some(),
        "only the SYSTIMER writer programs the bank"
    );
}

/// Mark executor `swi`'s task ready, in its context before it polls.
fn mark(swi: u8) -> impl FnMut(u8) {
    let ready = ready(swi);
    move |index| {
        if let Some(ready) = ready {
            ready.mark(index);
        }
    }
}

/// After `slot`'s queue changed: program its comparator (the writer) or
/// publish its next deadline to the writer.
fn rearm(swi: u8, comparator: Comparator, slot: &mut Slot, now: u64, fired: bool) {
    let Some(next) = slot.queue.alarm_deadline(fired) else {
        return;
    };
    let writer = WRITER.load(Ordering::Relaxed);
    #[cfg(oer_profile)]
    let start = now;
    if swi == writer {
        program(comparator, next);
    } else {
        PUBLISHED[swi as usize].publish(next);
        pend(writer);
    }
    #[cfg(oer_profile)]
    if next == u64::MAX {
        slot.observation.stop();
    } else {
        slot.observation.arm(next, start, self::now());
    }
    let _ = now;
}

/// Serve executor `swi`'s timers, in its context before it polls: dispatch
/// its expired timers and rearm; the writer also programs the comparators
/// whose executors published a new deadline.
pub(crate) fn service(swi: u8) {
    let Some(comparator) = COMPARATOR_OF
        .get(swi as usize)
        .and_then(|assignment| Comparator::from_index(assignment.load(Ordering::Acquire)))
    else {
        return;
    };
    let _ = with_slot(swi, |slot| {
        let fired = ALARM_FIRED[comparator as usize].swap(false, Ordering::AcqRel);
        let start = now();
        #[cfg(oer_profile)]
        if fired {
            let (entered, acknowledged) = &ALARM_AT[comparator as usize];
            if let (Some(entered), Some(acknowledged)) = (entered.read(), acknowledged.read()) {
                slot.observation
                    .interrupt(entered.value, acknowledged.value);
            }
        }
        slot.queue.dispatch_expired(start, mark(swi));
        #[cfg(oer_profile)]
        slot.observation.dispatch(start, now());
        // Only an actual alarm leaves the comparator idle. A task's self-wake
        // must not disable and program an unchanged comparator on every poll.
        rearm(swi, comparator, slot, start, fired);
        if swi == WRITER.load(Ordering::Relaxed) {
            program_published(slot);
        }
    });
}

/// The writer programs every other executor's comparator whose published
/// deadline it has not programmed yet. A torn read is left for the next
/// pass, which the publisher's pend brings.
fn program_published(writer: &mut Slot) {
    for (swi, cell) in PUBLISHED.iter().enumerate() {
        let Some(comparator) = Comparator::from_index(COMPARATOR_OF[swi].load(Ordering::Acquire))
        else {
            continue;
        };
        if swi as u8 == WRITER.load(Ordering::Relaxed) {
            continue;
        }
        let Some(published) = cell.read() else {
            continue;
        };
        let programmed = &mut writer.programmed[comparator as usize];
        if *programmed != Some(published.generation) {
            program(comparator, published.value);
            *programmed = Some(published.generation);
        }
    }
}

/// The interrupt-table handler of comparator `COMPARATOR`'s alarm
/// (`SYSTIMER_TARGET<COMPARATOR>`), which holds the comparator's alarm part:
/// it acknowledges the alarm and pends the comparator's executor. It writes
/// no other register and touches no queue.
pub fn alarm_interrupt<const COMPARATOR: u8, P: AlarmPart>(part: &mut P) {
    const {
        assert!(
            P::COMPARATOR as u8 == COMPARATOR,
            "the part is another comparator's"
        );
    };
    #[cfg(oer_profile)]
    let entered = now();
    part.acknowledge();
    #[cfg(oer_profile)]
    {
        let (entry, acknowledgement) = &ALARM_AT[COMPARATOR as usize];
        entry.publish(entered);
        acknowledgement.publish(now());
    }
    ALARM_FIRED[COMPARATOR as usize].store(true, Ordering::Release);
    let executor = EXECUTOR_OF[COMPARATOR as usize].load(Ordering::Acquire);
    if executor != NO_EXECUTOR {
        pend(executor);
    }
}

struct EmbassyTimeDriver;

#[used]
#[allow(
    unsafe_code,
    reason = "Embassy requires one exported global time-driver instance"
)]
#[unsafe(link_section = ".critical.data.executor_time")]
static EMBASSY_TIME_DRIVER: EmbassyTimeDriver = EmbassyTimeDriver;

#[allow(
    unsafe_code,
    reason = "Embassy time ABI requires this unique global symbol"
)]
#[unsafe(no_mangle)]
fn _embassy_time_now() -> u64 {
    <EmbassyTimeDriver as Driver>::now(&EMBASSY_TIME_DRIVER)
}

#[allow(
    unsafe_code,
    reason = "Embassy time ABI requires this unique global symbol"
)]
#[unsafe(no_mangle)]
fn _embassy_time_schedule_wake(at: u64, waker: &Waker) {
    <EmbassyTimeDriver as Driver>::schedule_wake(&EMBASSY_TIME_DRIVER, at, waker);
}

impl Driver for EmbassyTimeDriver {
    #[inline]
    fn now(&self) -> u64 {
        now()
    }

    fn schedule_wake(&self, at: u64, waker: &Waker) {
        let Some(task) = TaskId::of(waker) else {
            waker.wake_by_ref();
            return;
        };
        let swi = task.executor();
        let Some(comparator) = COMPARATOR_OF
            .get(usize::from(swi))
            .and_then(|assignment| Comparator::from_index(assignment.load(Ordering::Acquire)))
        else {
            panic!("executor {swi} has no timer comparator");
        };
        let registered = with_slot(swi, |slot| {
            #[cfg(oer_profile)]
            slot.observation.registration(at, now());
            if let Some(now) = slot.queue.schedule_wake(at, task.index(), now, mark(swi)) {
                rearm(swi, comparator, slot, now, false);
            }
        });
        if registered.is_none() {
            // Registered from another context than the task's executor: the
            // task polls again in its own context and registers there.
            waker.wake_by_ref();
        }
    }
}

#[cfg(oer_profile)]
pub(crate) fn begin_observation() -> bool {
    with_slot(running(), |slot| slot.observation.begin(now())).unwrap_or(false)
}

#[cfg(oer_profile)]
// Keep the recorder snapshot's temporary report out of the UART caller frame.
#[inline(never)]
pub(crate) fn finish_observation() -> crate::timer_observation::Report {
    with_slot(running(), |slot| slot.observation.finish(now())).unwrap_or_default()
}
