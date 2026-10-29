//! Chip access for the executor through its binding: the blocks, source
//! identities and reads of the platform port ([`ExecutorPlatform`]) the
//! binding names, the port's hart geometry and architectural CSR
//! instructions.

pub(crate) use crate::binding::Binding;
use crate::{Failure, Fault, SoftwareInterrupt};
use oer_espressif_boot::drivers::system_timer;
use oer_espressif_platform_port::{ExecutorPlatform, Hart};

use crate::software_line::{SoftwareLine, serve};

/// The port of binding `B`.
pub(crate) type PortOf<B> = <B as Binding>::Port;
/// A port's counter owner.
pub(crate) type Counter<P> = <P as ExecutorPlatform>::Counter;
/// A port's comparator owner.
pub(crate) type Comparators<P> = <P as ExecutorPlatform>::Comparators;
/// The system timer of binding `B`'s port.
type SystemTimer<B> = crate::systimer::SystemTimer<PortOf<B>>;
/// The comparators of a published counter, or the refused timer.
type CounterSetup<B> = Result<Comparators<PortOf<B>>, Failure<PortOf<B>, SystemTimer<B>>>;

/// The software interrupts an executor can own.
pub(crate) const SOFTWARE_INTERRUPT_COUNT: usize = crate::task::EXECUTORS as usize;

/// The core running the caller.
#[inline(always)]
pub(crate) fn current_core<B: Binding>() -> usize {
    current_hart::<B>().get()
}

/// Validate the architectural ID before selecting any per-hart state.
#[inline(always)]
pub(crate) fn current_hart<B: Binding>() -> Hart {
    match Hart::new(oer_riscv::hart_id() as u32) {
        Ok(hart) => hart,
        Err(_fault) => {
            // The existing fatal path retains InvalidHart's full raw ID;
            // no executor or register owner is released on this path.
            #[cfg(target_arch = "riscv32")]
            {
                oer_espressif_staged_runtime::fatal::reset_recording::<PortOf<B>>(|_, _| ())
            }
            // Host type checks have no reset hardware or retained boot record.
            #[cfg(not(target_arch = "riscv32"))]
            {
                <PortOf<B> as oer_espressif_platform_port::Platform>::fatal(|_| ())
            }
        }
    }
}

/// The hart running the caller, when the architectural ID names one: the
/// lookup of the execution contexts, which name no binding. Every executor
/// round and pend validates the hart through [`current_hart`] first.
#[inline(always)]
pub(crate) fn checked_hart() -> Option<Hart> {
    Hart::new(oer_riscv::hart_id() as u32).ok()
}

/// The interrupt level the caller runs at: the CLIC's `mintstatus.mil`,
/// which the hart raises on a trap's entry to the taken interrupt's level
/// and restores on its return; 0 in thread mode. The core takes interrupts
/// through a CLIC.
#[inline(always)]
pub(crate) fn interrupt_level() -> u8 {
    let status = oer_riscv::read_mintstatus();
    // `mil` is the top byte: the truncation keeps exactly the field.
    (status >> oer_riscv::clic::MINTSTATUS_MIL_SHIFT) as u8
}

/// Publish the counter latch, from the system timer's handoff, once its
/// latch has settled within its bound ([`system_timer::check`]).
///
/// # Errors
/// A latch that does not settle returns the complete timer to its caller.
pub(crate) fn install_counter<B: Binding>(systimer: SystemTimer<B>) -> CounterSetup<B> {
    let storage = B::storage();
    if storage.counter.get().is_some() {
        Fault::CounterPublished.reset::<PortOf<B>>();
    }
    if system_timer::check(&systimer.counter).is_err() {
        return Err(Failure::new(Fault::CounterLatchStuck, systimer));
    }
    let cell = match storage.counter_storage.try_uninit() {
        Some(cell) => cell,
        None => Fault::CounterPublished.reset::<PortOf<B>>(),
    };
    storage.counter.set(cell.write(systimer.counter));
    Ok(systimer.comparators)
}

/// The system timer's counter, in microseconds: a verified read, or the
/// floor of a read that preempting strobes kept from verifying, which is no
/// smaller than any earlier reading and no larger than any later one.
/// Before the time driver's [`init`](crate::init) publishes the counter
/// the clock reads 0, which every later read exceeds. The read never panics.
// Share the same clock read with cooperative admission and poll stamping.
#[inline(never)]
pub(crate) fn now<B: Binding>() -> u64 {
    B::storage().counter.get().map_or(0, |counter| {
        system_timer::read(counter).unwrap_or_else(|unverified| unverified.floor)
            / PortOf::<B>::TICKS_PER_MICROSECOND
    })
}

/// Whether `source` is pending at the current core's interrupt matrix.
#[inline(always)]
pub(crate) fn is_pending<B: Binding>(source: u16) -> bool {
    PortOf::<B>::is_pending(current_hart::<B>(), source)
}

pub(crate) const fn software_interrupt_source<P: ExecutorPlatform>(swi: SoftwareInterrupt) -> u16 {
    let [zero, one, two, three] = P::SOFTWARE_SOURCES;
    match swi {
        SoftwareInterrupt::Zero => zero,
        SoftwareInterrupt::One => one,
        SoftwareInterrupt::Two => two,
        SoftwareInterrupt::Three => three,
    }
}

pub(crate) const fn alarm_source<P: ExecutorPlatform>(
    comparator: oer_espressif_platform_port::Comparator,
) -> u16 {
    use oer_espressif_platform_port::Comparator;
    let [zero, one, two] = P::ALARM_SOURCES;
    match comparator {
        Comparator::Zero => zero,
        Comparator::One => one,
        Comparator::Two => two,
    }
}

pub(crate) fn wait_for_interrupt<B: Binding, T: crate::Tasks<Binding = B>>() {
    let _: core::marker::PhantomData<T> = core::marker::PhantomData;
    if !PortOf::<B>::debugger_connected(current_hart::<B>()) {
        #[cfg(oer_trace)]
        let started = now::<B>() as u32;
        oer_riscv::wait_for_interrupt();
        #[cfg(oer_trace)]
        {
            let ended = now::<B>() as u32;
            let observer = T::observer();
            // Publish both edges after the wait, while still masked: observer
            // work is busy time, and the waking handler has not run yet.
            oer_trace::observe!(
                observer,
                crate::observation::Activity::new(
                    started,
                    T::EXECUTOR,
                    current_core::<B>() as u8,
                    interrupt_level(),
                    crate::observation::Phase::IdleStart
                )
            );
            oer_trace::observe!(
                observer,
                crate::observation::Activity::new(
                    ended,
                    T::EXECUTOR,
                    current_core::<B>() as u8,
                    interrupt_level(),
                    crate::observation::Phase::IdleEnd
                )
            );
        }
    }
}

/// One software interrupt line: its typed block, lent to every context.
pub(crate) enum Line<P: ExecutorPlatform> {
    Zero(&'static P::Line0),
    One(&'static P::Line1),
    Two(&'static P::Line2),
    Three(&'static P::Line3),
}

impl<P: ExecutorPlatform> Clone for Line<P> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<P: ExecutorPlatform> Copy for Line<P> {}

impl<P: ExecutorPlatform> SoftwareLine for Line<P> {
    #[inline(always)]
    fn raise(&self) {
        match self {
            Self::Zero(line) => line.raise(),
            Self::One(line) => line.raise(),
            Self::Two(line) => line.raise(),
            Self::Three(line) => line.raise(),
        }
    }
    #[inline(always)]
    fn lower(&self) {
        match self {
            Self::Zero(line) => line.lower(),
            Self::One(line) => line.lower(),
            Self::Two(line) => line.lower(),
            Self::Three(line) => line.lower(),
        }
    }
}

/// A port's software interrupt line 0 owner.
pub(crate) type Line0<B> = <PortOf<B> as ExecutorPlatform>::Line0;
/// A port's software interrupt line 1 owner.
pub(crate) type Line1<B> = <PortOf<B> as ExecutorPlatform>::Line1;
/// A port's software interrupt line 2 owner.
pub(crate) type Line2<B> = <PortOf<B> as ExecutorPlatform>::Line2;
/// A port's software interrupt line 3 owner.
pub(crate) type Line3<B> = <PortOf<B> as ExecutorPlatform>::Line3;

macro_rules! own_line {
    ($number:literal, $variant:ident, $block:ident, $take:ident, $storage:ident) => {
        /// Publish a line moved from the single reset root, once.
        pub(crate) fn $take<B: Binding>(block: $block<B>) -> Line<PortOf<B>> {
            let storage = match B::storage().$storage.try_uninit() {
                Some(storage) => storage,
                None => Fault::LinePublished { executor: $number }.reset::<PortOf<B>>(),
            };
            Line::$variant(storage.write(block))
        }
    };
}

own_line!(0, Zero, Line0, own_line_0, line0);
own_line!(1, One, Line1, own_line_1, line1);
own_line!(2, Two, Line2, own_line_2, line2);
own_line!(3, Three, Line3, own_line_3, line3);

/// Install software interrupt `swi`'s line for its executor.
pub(crate) fn install_line<B: Binding>(swi: SoftwareInterrupt, line: Line<PortOf<B>>) {
    let slot = swi.at(&B::storage().lines);
    line.lower();
    if slot.set(line).is_err() {
        Fault::LinePublished {
            executor: swi as u8,
        }
        .reset::<PortOf<B>>();
    }
}

/// Software interrupt `swi`'s installed line: none before its executor or
/// doorbell took it, when there is nothing it could wake.
fn line<B: Binding>(swi: SoftwareInterrupt) -> Option<Line<PortOf<B>>> {
    swi.at(&B::storage().lines).get()
}

/// Raise software interrupt `swi`; a line no executor took yet has no
/// handler to run, so nothing is raised. Never panics.
#[inline(always)]
pub(crate) fn raise_interrupt<B: Binding>(swi: SoftwareInterrupt) {
    if let Some(line) = line::<B>(swi) {
        line.raise();
    }
}

/// One handler round of software interrupt `swi`: lower its line, then take
/// the work (`drain`), so a raise during the work runs another round. Only
/// an installed line's handler runs, since only an installed line is raised
/// and routed. Never panics.
#[inline(always)]
pub(crate) fn serve_interrupt<B: Binding, R>(
    swi: SoftwareInterrupt,
    drain: impl FnOnce() -> R,
) -> R {
    match line::<B>(swi) {
        Some(line) => serve(&line, drain),
        None => drain(),
    }
}
