use core::marker::PhantomData;

use embassy_executor::{Spawner, raw};
use esp_hal::{
    interrupt::{Priority, software::SoftwareInterrupt},
    peripherals::Interrupt,
    system::Cpu,
};
use esp_sync::NonReentrantMutex;
use oer_interrupt_table::Entry;
use portable_atomic::{AtomicBool, AtomicUsize, Ordering};

const SOFTWARE_INTERRUPT_COUNT: usize = 4;
const THREAD_MODE_CONTEXT: usize = 16;
const UNASSIGNED_CORE: usize = usize::MAX;

/// One wake flag per software interrupt; zero is "no work pending".
#[repr(transparent)]
struct WorkPending([AtomicBool; SOFTWARE_INTERRUPT_COUNT]);

#[allow(unsafe_code, reason = "false flags are zero bytes")]
// SAFETY: `portable_atomic::AtomicBool` has the in-memory representation of
// `bool` (its documented guarantee), so zero bytes are `false` flags.
unsafe impl bytemuck::Zeroable for WorkPending {}

oer_memory::zeroed_static! {
    /// Placement: the board linker owns this exported wake-state section.
    #[used]
    static EMBASSY_WORK_PENDING: WorkPending =
        zeroed in ".critical.bss.embassy_executor";
}
#[used]
#[allow(
    unsafe_code,
    reason = "board linker owns this exported executor core-state section"
)]
#[unsafe(link_section = ".critical.data.embassy_executor")]
static EMBASSY_EXECUTOR_CORE: [AtomicUsize; SOFTWARE_INTERRUPT_COUNT] =
    [const { AtomicUsize::new(UNASSIGNED_CORE) }; SOFTWARE_INTERRUPT_COUNT];

enum OwnedSoftwareInterrupt {
    Zero(SoftwareInterrupt<'static, 0>),
    One(SoftwareInterrupt<'static, 1>),
    Two(SoftwareInterrupt<'static, 2>),
    Three(SoftwareInterrupt<'static, 3>),
}

impl OwnedSoftwareInterrupt {
    fn reset(&self) {
        match self {
            Self::Zero(interrupt) => interrupt.reset(),
            Self::One(interrupt) => interrupt.reset(),
            Self::Two(interrupt) => interrupt.reset(),
            Self::Three(interrupt) => interrupt.reset(),
        }
    }

    fn raise(&self) {
        match self {
            Self::Zero(interrupt) => interrupt.raise(),
            Self::One(interrupt) => interrupt.raise(),
            Self::Two(interrupt) => interrupt.raise(),
            Self::Three(interrupt) => interrupt.raise(),
        }
    }
}

#[used]
#[allow(
    unsafe_code,
    reason = "board linker owns the runtime software-interrupt token section"
)]
#[unsafe(link_section = ".critical.data.embassy_executor")]
static EMBASSY_INTERRUPTS: NonReentrantMutex<
    [Option<OwnedSoftwareInterrupt>; SOFTWARE_INTERRUPT_COUNT],
> = NonReentrantMutex::new([const { None }; SOFTWARE_INTERRUPT_COUNT]);

/// Scheduler-free thread-mode Embassy executor.
///
/// One software interrupt is reserved per executor so a waker on another CPU
/// can wake the sleeping owner without introducing a scheduler or RTOS.
pub struct Executor<const SWI: u8> {
    inner: raw::Executor,
    interrupt: Option<OwnedSoftwareInterrupt>,
    not_send: PhantomData<*mut ()>,
}

impl<const SWI: u8> Executor<SWI> {
    fn with_interrupt(interrupt: OwnedSoftwareInterrupt) -> Self {
        Self {
            inner: raw::Executor::new((THREAD_MODE_CONTEXT + SWI as usize) as *mut ()),
            interrupt: Some(interrupt),
            not_send: PhantomData,
        }
    }

    /// Run the executor on the calling core, woken through its software
    /// interrupt: `wake` is that source's token of the image's interrupt table,
    /// whose entry names [`wake_handler::<SWI>`](wake_handler) on this core.
    ///
    /// # Panics
    ///
    /// When the table routes the source to another core.
    pub fn run<W>(&'static mut self, wake: W, init: impl FnOnce(Spawner)) -> !
    where
        W: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
    {
        const {
            assert!(
                W::SOURCE as u16 == software_interrupt_source(SWI) as u16,
                "the token is not this executor's software interrupt"
            )
        };
        let current_core = Cpu::current() as usize;
        let interrupt = self
            .interrupt
            .take()
            .expect("executor software interrupt was already installed");
        interrupt.reset();
        EMBASSY_INTERRUPTS.with(|interrupts| {
            assert!(
                interrupts[SWI as usize].is_none(),
                "software interrupt {SWI} is already installed"
            );
            interrupts[SWI as usize] = Some(interrupt);
        });
        if let Err(error) = oer_espressif_interrupt_table_esp_hal::enable(&wake) {
            panic!("executor software interrupt {SWI}: {error:?}");
        }
        EMBASSY_EXECUTOR_CORE[SWI as usize]
            .compare_exchange(
                UNASSIGNED_CORE,
                current_core,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .unwrap_or_else(|_| panic!("software interrupt {SWI} is already used by an executor"));
        init(self.inner.spawner());

        loop {
            EMBASSY_WORK_PENDING.0[SWI as usize].store(false, Ordering::Release);
            if SWI == 0 {
                crate::time_driver::dispatch_pending();
            }
            #[allow(
                unsafe_code,
                reason = "the static executor owner is polled only by its run loop"
            )]
            // SAFETY: only this non-returning run loop polls `inner`, and the
            // pender merely marks work and wakes the software interrupt whose
            // handler only resets it, so `poll` is never entered reentrantly.
            unsafe {
                self.inner.poll()
            };
            wait_for_work::<SWI>();
        }
    }
}

macro_rules! impl_executor_constructor {
    ($number:literal, $variant:ident) => {
        impl Executor<$number> {
            pub fn new(interrupt: SoftwareInterrupt<'static, $number>) -> Self {
                Self::with_interrupt(OwnedSoftwareInterrupt::$variant(interrupt))
            }
        }
    };
}

impl_executor_constructor!(0, Zero);
impl_executor_constructor!(1, One);
impl_executor_constructor!(2, Two);
impl_executor_constructor!(3, Three);

/// The peripheral interrupt source of software interrupt `SWI`.
const fn software_interrupt_source(swi: u8) -> Interrupt {
    match swi {
        0 => Interrupt::FROM_CPU_INTR0,
        1 => Interrupt::FROM_CPU_INTR1,
        2 => Interrupt::FROM_CPU_INTR2,
        _ => Interrupt::FROM_CPU_INTR3,
    }
}

/// The interrupt-table handler of an executor's software interrupt
/// (`FROM_CPU_INTR<SWI>`): it wakes the executor.
#[esp_hal::ram]
pub fn wake_handler<const SWI: u8>() {
    EMBASSY_INTERRUPTS.with(|interrupts| {
        interrupts[SWI as usize]
            .as_ref()
            .expect("software interrupt fired before executor installation")
            .reset();
    });
}

#[inline(always)]
pub(crate) fn mark_work<const SWI: u8>() {
    EMBASSY_WORK_PENDING.0[SWI as usize].store(true, Ordering::Release);
}

#[inline(always)]
fn pend<const SWI: u8>() {
    mark_work::<SWI>();
    let target_core = EMBASSY_EXECUTOR_CORE[SWI as usize].load(Ordering::Acquire);
    if target_core != UNASSIGNED_CORE && target_core != Cpu::current() as usize {
        EMBASSY_INTERRUPTS.with(|interrupts| {
            interrupts[SWI as usize]
                .as_ref()
                .expect("executor core assigned before software interrupt installation")
                .raise();
        });
    }
}

fn wait_for_work<const SWI: u8>() {
    riscv::interrupt::free(|| {
        if !EMBASSY_WORK_PENDING.0[SWI as usize].load(Ordering::Acquire) {
            #[cfg(feature = "idle-accounting")]
            crate::idle_accounting::add_wait(SWI as usize);
            #[cfg(not(feature = "idle-accounting"))]
            esp_hal::interrupt::wait_for_interrupt();
        }
    });
}

/// The core executor `executor` runs on, once it runs.
#[cfg(feature = "idle-accounting")]
pub(crate) fn executor_core(executor: usize) -> Option<usize> {
    let core = EMBASSY_EXECUTOR_CORE[executor].load(Ordering::Acquire);
    (core != UNASSIGNED_CORE).then_some(core)
}

/// Raise the software interrupt of the running executor `executor`, waking
/// its core from its wait.
#[cfg(feature = "idle-accounting")]
pub(crate) fn raise(executor: usize) {
    EMBASSY_INTERRUPTS.with(|interrupts| {
        if let Some(interrupt) = interrupts[executor].as_ref() {
            interrupt.raise();
        }
    });
}

#[esp_hal::ram]
#[allow(
    unsafe_code,
    reason = "Embassy requires this unique global pender ABI symbol"
)]
#[unsafe(export_name = "__pender")]
fn embassy_pender(context: *mut ()) {
    match context as usize {
        16 => pend::<0>(),
        17 => pend::<1>(),
        18 => pend::<2>(),
        19 => pend::<3>(),
        _ => unreachable!("invalid Embassy executor context"),
    }
}
