//! The Embassy time driver: one timer queue and one SYSTIMER comparator per
//! executor, and our own sole writer of the SYSTIMER comparator registers.
//!
//! - Each executor bound with [`bind`] owns a queue that only its own context
//!   touches: a timer is registered while the executor polls the task, and
//!   expired timers are dispatched before it polls. A timer registered from
//!   another context than its task's executor wakes the task at once, so it
//!   registers again from its own context. No queue is shared between
//!   contexts, and none is locked.
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
//! [`init`] consumes esp-hal's SYSTIMER singleton through [`ComparatorWriter`].
//! Comparator programming and W1C acknowledgement belong to this executor.
//! `Instant::now()` and `SystemTimer::ticks_per_second()` stay with esp-hal:
//! its `UNIT0_OP` counter latch shares no word with the comparators.

use core::{cell::UnsafeCell, task::Waker};

use embassy_time_driver::Driver;
use esp_hal::{
    interrupt::Priority,
    peripherals::{Interrupt, SYSTIMER},
    system::Cpu,
    time::Instant,
    timer::systimer::SystemTimer,
};
use oer_interrupt_table::Entry;
use portable_atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use crate::{
    deadline_cell::DeadlineCell,
    executor::{NO_EXECUTOR, SOFTWARE_INTERRUPT_COUNT, executor_of, pend, running},
    systimer::{Comparator, ComparatorAcknowledger, ComparatorWriter},
    timer_queue::WakeQueue,
};

enum Acknowledger {
    Zero(ComparatorAcknowledger<0>),
    One(ComparatorAcknowledger<1>),
    Two(ComparatorAcknowledger<2>),
}
impl Acknowledger {
    fn bind(&mut self, bank: &mut ComparatorWriter) {
        match self {
            Self::Zero(ack) => bank.bind(ack),
            Self::One(ack) => bank.bind(ack),
            Self::Two(ack) => bank.bind(ack),
        }
    }
    #[inline(always)]
    fn acknowledge(&mut self) {
        match self {
            Self::Zero(ack) => ack.acknowledge(),
            Self::One(ack) => ack.acknowledge(),
            Self::Two(ack) => ack.acknowledge(),
        }
    }
}

// The boot context hands the bank to its one writer executor. Alarm ISRs
// receive independent W1C owners and never borrow BANK.
static BANK_STORAGE: static_cell::StaticCell<ComparatorWriter> = static_cell::StaticCell::new();
static BANK: oer_memory::HandlerCell<ComparatorWriter> = oer_memory::HandlerCell::new();
static ACK_STORAGE: [static_cell::StaticCell<Acknowledger>; COMPARATOR_COUNT] =
    [const { static_cell::StaticCell::new() }; COMPARATOR_COUNT];
static ACKNOWLEDGERS: [oer_memory::HandlerCell<Acknowledger>; COMPARATOR_COUNT] =
    [const { oer_memory::HandlerCell::new() }; COMPARATOR_COUNT];

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
/// SYSTIMER ticks per microsecond.
static TICKS_PER_MICROSECOND: AtomicU32 = AtomicU32::new(0);

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

#[inline]
fn now() -> u64 {
    Instant::now().duration_since_epoch().as_micros()
}

/// The comparator whose alarm interrupt is `source`.
const fn comparator_of_source(source: Interrupt) -> u8 {
    if source as u16 == Interrupt::SYSTIMER_TARGET0 as u16 {
        0
    } else if source as u16 == Interrupt::SYSTIMER_TARGET1 as u16 {
        1
    } else if source as u16 == Interrupt::SYSTIMER_TARGET2 as u16 {
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
pub fn init<const WRITER_SWI: u8, A>(systimer: SYSTIMER<'static>, alarm: A)
where
    A: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
{
    let (bank, zero, one, two) = ComparatorWriter::adopt(systimer);
    assert!(
        BANK.install(BANK_STORAGE.init(bank)).is_ok(),
        "the SYSTIMER bank owner is already installed"
    );
    for ((storage, acknowledger), ack) in ACK_STORAGE.iter().zip(&ACKNOWLEDGERS).zip([
        Acknowledger::Zero(zero),
        Acknowledger::One(one),
        Acknowledger::Two(two),
    ]) {
        assert!(
            acknowledger.install(storage.init(ack)).is_ok(),
            "the SYSTIMER acknowledger is already installed"
        );
    }
    assert!(
        WRITER
            .compare_exchange(NO_EXECUTOR, WRITER_SWI, Ordering::AcqRel, Ordering::Acquire)
            .is_ok(),
        "the time driver is installed already"
    );
    let ticks_per_second = SystemTimer::ticks_per_second();
    let ticks_per_microsecond = u32::try_from(ticks_per_second / 1_000_000).unwrap_or(0);
    assert!(
        ticks_per_microsecond != 0 && ticks_per_second.is_multiple_of(1_000_000),
        "the system timer counts whole ticks per microsecond"
    );
    TICKS_PER_MICROSECOND.store(ticks_per_microsecond, Ordering::Relaxed);
    bind::<WRITER_SWI, A>(alarm);
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
pub fn bind<const SWI: u8, A>(alarm: A)
where
    A: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
{
    let comparator = const {
        let comparator = comparator_of_source(A::SOURCE);
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
    // Setup still owns both halves before this alarm's route is enabled.
    assert!(
        BANK.with(|bank| {
            assert!(
                ACKNOWLEDGERS
                    .get(usize::from(comparator))
                    .and_then(|ack| ack.with(|ack| ack.bind(bank)))
                    .is_some(),
                "the comparator owner is installed before its route"
            );
        })
        .is_some(),
        "the SYSTIMER bank is installed before binding"
    );
    if let Err(error) = oer_espressif_esp_hal_irq::enable(&alarm) {
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
            .saturating_mul(u64::from(TICKS_PER_MICROSECOND.load(Ordering::Relaxed)))
            .min(TICK_MASK)
    });
    assert!(
        BANK.with(|bank| bank.program(comparator, ticks)).is_some(),
        "only the SYSTIMER writer programs the bank"
    );
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
        slot.queue.dispatch_expired(start);
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
/// (`SYSTIMER_TARGET<COMPARATOR>`): it acknowledges the alarm and pends the
/// comparator's executor. It writes no other register and touches no queue.
pub fn alarm_interrupt<const COMPARATOR: u8>() {
    #[cfg(oer_profile)]
    let entered = now();
    assert!(
        ACKNOWLEDGERS
            .get(usize::from(COMPARATOR))
            .and_then(|ack| ack.with(Acknowledger::acknowledge))
            .is_some(),
        "only the alarm handler acknowledges its installed comparator"
    );
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
#[unsafe(link_section = ".critical.data.embassy_time")]
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
        let task = embassy_executor::raw::task_from_waker(waker);
        #[allow(
            unsafe_code,
            reason = "the task's executor is only compared, never dereferenced for polling"
        )]
        // SAFETY: the reference is used only to read the executor's id.
        let executor = unsafe { task.executor() }.and_then(|executor| executor_of(executor.id()));
        let Some(swi) = executor else {
            waker.wake_by_ref();
            return;
        };
        let Some(comparator) =
            Comparator::from_index(COMPARATOR_OF[swi as usize].load(Ordering::Acquire))
        else {
            panic!("executor {swi} has no timer comparator");
        };
        let registered = with_slot(swi, |slot| {
            #[cfg(oer_profile)]
            slot.observation.registration(at, now());
            if let Some(now) = slot.queue.schedule_wake(at, waker, now) {
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
pub(crate) fn finish_observation() -> crate::timer_observation::Report {
    with_slot(running(), |slot| slot.observation.finish(now())).unwrap_or_default()
}
