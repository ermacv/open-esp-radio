use core::marker::PhantomData;

use embassy_executor::{SendSpawner, Spawner, raw};
use esp_hal::{
    interrupt::{
        Priority,
        software::{SoftwareInterrupt, SoftwareInterruptAcknowledger, SoftwareInterruptSender},
    },
    peripherals::Interrupt,
    system::Cpu,
};
use oer_interrupt_table::{
    Entry,
    signal::{IrqSignal, SignalExecutor, SignalList},
};
use portable_atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

pub(crate) const SOFTWARE_INTERRUPT_COUNT: usize = 4;
const CORE_COUNT: usize = 2;
const THREAD_MODE_CONTEXT: usize = 16;
const INTERRUPT_MODE_CONTEXT: usize = 32;
const UNASSIGNED_CORE: usize = usize::MAX;
/// No executor's context runs on the core.
pub(crate) const NO_EXECUTOR: u8 = u8::MAX;

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

/// Each software interrupt's raw executor, once it runs: the `id` its tasks'
/// wakers name, and the executor an interrupt-mode handler polls.
#[used]
#[allow(
    unsafe_code,
    reason = "board linker owns this exported executor core-state section"
)]
#[unsafe(link_section = ".critical.data.embassy_executor")]
static EMBASSY_EXECUTORS: [AtomicUsize; SOFTWARE_INTERRUPT_COUNT] =
    [const { AtomicUsize::new(0) }; SOFTWARE_INTERRUPT_COUNT];

/// Which executor's context runs on each core now: the software interrupt of
/// the executor that is polling, or [`NO_EXECUTOR`]. An interrupt-mode
/// executor sets it for its poll and restores the context it preempted.
#[used]
#[allow(
    unsafe_code,
    reason = "board linker owns this exported executor core-state section"
)]
#[unsafe(link_section = ".critical.data.embassy_executor")]
static EMBASSY_RUNNING: [AtomicU8; CORE_COUNT] = [const { AtomicU8::new(NO_EXECUTOR) }; CORE_COUNT];

/// The executor whose context runs on the current core now.
pub(crate) fn running() -> u8 {
    EMBASSY_RUNNING[Cpu::current() as usize].load(Ordering::Relaxed)
}

/// The software interrupt of the executor whose raw executor has `id`.
pub(crate) fn executor_of(id: usize) -> Option<u8> {
    (0..SOFTWARE_INTERRUPT_COUNT as u8)
        .find(|&swi| id != 0 && EMBASSY_EXECUTORS[swi as usize].load(Ordering::Acquire) == id)
}

/// Run `poll` as executor `swi`'s context and restore the preempted one.
fn as_running(swi: u8, poll: impl FnOnce()) {
    let running = &EMBASSY_RUNNING[Cpu::current() as usize];
    let preempted = running.swap(swi, Ordering::Relaxed);
    poll();
    running.store(preempted, Ordering::Relaxed);
}

// The sender is published once for wakers; only the line's ISR borrows
// its acknowledger. Boot resets the line before either half is published.
enum Sender {
    Zero(SoftwareInterruptSender<'static, 0>),
    One(SoftwareInterruptSender<'static, 1>),
    Two(SoftwareInterruptSender<'static, 2>),
    Three(SoftwareInterruptSender<'static, 3>),
}
enum Acknowledger {
    Zero(SoftwareInterruptAcknowledger<'static, 0>),
    One(SoftwareInterruptAcknowledger<'static, 1>),
    Two(SoftwareInterruptAcknowledger<'static, 2>),
    Three(SoftwareInterruptAcknowledger<'static, 3>),
}
static SENDERS: [oer_memory::zeroed::ZeroedOnce<Sender>; SOFTWARE_INTERRUPT_COUNT] =
    [const { oer_memory::zeroed::ZeroedOnce::new() }; SOFTWARE_INTERRUPT_COUNT];
static ACK_STORAGE: [static_cell::StaticCell<Acknowledger>; SOFTWARE_INTERRUPT_COUNT] =
    [const { static_cell::StaticCell::new() }; SOFTWARE_INTERRUPT_COUNT];
static ACKNOWLEDGERS: [oer_memory::HandlerCell<Acknowledger>; SOFTWARE_INTERRUPT_COUNT] =
    [const { oer_memory::HandlerCell::new() }; SOFTWARE_INTERRUPT_COUNT];

impl Acknowledger {
    fn reset(&mut self) {
        match self {
            Self::Zero(ack) => ack.reset(),
            Self::One(ack) => ack.reset(),
            Self::Two(ack) => ack.reset(),
            Self::Three(ack) => ack.reset(),
        }
    }
}

fn install_line(
    senders: &'static oer_memory::zeroed::ZeroedOnce<Sender>,
    acknowledger: &'static oer_memory::HandlerCell<Acknowledger>,
    storage: &'static static_cell::StaticCell<Acknowledger>,
    sender: Sender,
    mut ack: Acknowledger,
) {
    ack.reset();
    assert!(
        senders.get().is_none(),
        "software interrupt owner is already installed"
    );
    let _sender = senders.get_or_init(|| sender);
    assert!(
        acknowledger.install(storage.init(ack)).is_ok(),
        "software interrupt acknowledger is already installed"
    );
}

#[inline(always)]
fn raise_interrupt(swi: u8) {
    let raised = SENDERS
        .get(usize::from(swi))
        .and_then(oer_memory::zeroed::ZeroedOnce::get)
        .map(|sender| match sender {
            Sender::Zero(sender) => sender.raise(),
            Sender::One(sender) => sender.raise(),
            Sender::Two(sender) => sender.raise(),
            Sender::Three(sender) => sender.raise(),
        });
    assert!(
        raised.is_some(),
        "software interrupt sender is installed before use"
    );
}

#[inline(always)]
fn acknowledge_interrupt(swi: u8) {
    assert!(
        ACKNOWLEDGERS
            .get(usize::from(swi))
            .and_then(|ack| ack.with(Acknowledger::reset))
            .is_some(),
        "only the installed software interrupt handler acknowledges its line"
    );
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
    High: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
    Mid: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
    Low: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
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
                High::CORE as u8 == Mid::CORE as u8 && Mid::CORE as u8 == Low::CORE as u8,
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
    High: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
    Mid: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
    Low: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
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
/// esp-hal runs all sources of one level on one core one after another, so
/// neither handler preempts the other. The host tests of `level` cover the
/// refusals: a `compile_fail` doctest proves nothing on the chip target,
/// where every doctest that links fails on esp-hal's interrupt vector.
///
/// The doctest stands in table tokens for the image's (`Entry` is
/// implemented here by hand, as only the table's macro may in an image):
///
/// ```no_run
/// # #![no_std]
/// # #![no_main]
/// # #[panic_handler]
/// # fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
/// # use esp_hal::{interrupt::Priority, peripherals::Interrupt, system::Cpu};
/// # macro_rules! token {
/// #     ($token:ident = $source:ident, $level:ident, $core:ident) => {
/// #         struct $token;
/// #         // SAFETY: a doctest's stand-in for a table token.
/// #         unsafe impl oer_interrupt_table::Entry for $token {
/// #             type Source = Interrupt;
/// #             type Level = Priority;
/// #             type Core = Cpu;
/// #             const SOURCE: Interrupt = Interrupt::$source;
/// #             const LEVEL: Priority = Priority::$level;
/// #             const CORE: Cpu = Cpu::$core;
/// #         }
/// #     };
/// # }
/// # token!(MacToken = MODEM_WIFI_MAC, Priority3, ProCpu);
/// # token!(DoorbellToken = FROM_CPU_INTR3, Priority3, ProCpu);
/// use oer_espressif_executor::SameLevel;
/// // The MAC interrupt and the doorbell, both at Priority3 on ProCpu.
/// const BELL: SameLevel<MacToken, DoorbellToken> = SameLevel::new();
/// ```
pub struct SameLevel<Radio, Bell> {
    _levels: PhantomData<Pair<Radio, Bell>>,
}

type Pair<Radio, Bell> = fn() -> (Radio, Bell);

impl<Radio, Bell> SameLevel<Radio, Bell>
where
    Radio: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
    Bell: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
{
    /// The two entries' common level and core, or a compile error.
    pub const fn new() -> Self {
        const {
            crate::level::same_level(
                (Radio::LEVEL as u8, Radio::CORE as u8),
                (Bell::LEVEL as u8, Bell::CORE as u8),
            )
        }
        Self {
            _levels: PhantomData,
        }
    }
}

impl<Radio, Bell> Default for SameLevel<Radio, Bell>
where
    Radio: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
    Bell: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
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

/// Scheduler-free thread-mode Embassy executor.
///
/// One software interrupt is reserved per executor so a waker on another CPU
/// can wake the sleeping owner without introducing a scheduler or RTOS.
pub struct Executor<const SWI: u8> {
    inner: raw::Executor,
    not_send: PhantomData<*mut ()>,
}

impl<const SWI: u8> Executor<SWI> {
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
        if let Err(error) = oer_espressif_esp_hal_irq::enable(&wake) {
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
        let inner: &'static raw::Executor = &self.inner;
        EMBASSY_EXECUTORS[SWI as usize].store(inner.id(), Ordering::Release);
        init(inner.spawner());
        loop {
            EMBASSY_WORK_PENDING.0[SWI as usize].store(false, Ordering::Release);
            as_running(SWI, || {
                crate::time_driver::service(SWI);
                #[allow(
                    unsafe_code,
                    reason = "the static executor owner is polled only by its run loop"
                )]
                // SAFETY: only this non-returning run loop polls `inner`, and
                // the pender merely marks work and wakes the software
                // interrupt whose handler only resets it, so `poll` is never
                // entered reentrantly.
                unsafe {
                    inner.poll()
                };
            });
            wait_for_work::<SWI>();
        }
    }
}

/// The signals each interrupt-mode executor's handler services before it
/// polls; only that executor's context touches its list.
static SIGNAL_LISTS: [SignalList; SOFTWARE_INTERRUPT_COUNT] =
    [const { SignalList::new() }; SOFTWARE_INTERRUPT_COUNT];

/// Pend the interrupt-mode executor `SWI` for a raised signal: raise its
/// software interrupt, whose handler services the signal and polls. It
/// wakes no task itself.
#[esp_hal::ram]
fn pend_signal<const SWI: u8>(_: &'static IrqSignal<InterruptExecutor<SWI>>) {
    raise_interrupt(SWI);
}

/// Whether the caller runs in the interrupt-mode executor `SWI`'s context:
/// its handler's poll on this core.
#[esp_hal::ram]
fn signal_context<const SWI: u8>() -> bool {
    running() == SWI
        && EMBASSY_EXECUTORS_MODE
            .get(SWI as usize)
            .is_some_and(|interrupt_mode| interrupt_mode.load(Ordering::Acquire))
}

/// Embassy executor polled in its software interrupt's handler, at that
/// interrupt's level: the radio executor (P2) on `FROM_CPU` line 2.
///
/// Its handler preempts lower levels; a higher level preempts it. A task
/// spawned on it must be `Send`, since it is created in another context than
/// the one that polls it.
pub struct InterruptExecutor<const SWI: u8> {
    inner: raw::Executor,
}

#[allow(unsafe_code, reason = "the executor's signal context contract")]
// SAFETY: `signal_context::<SWI>` is true only while this executor's
// handler polls (`as_running`), which never reenters itself on its core;
// `pend_signal::<SWI>` raises that handler's interrupt, which services
// `SIGNAL_LISTS[SWI]` before it polls (`interrupt_handler`).
unsafe impl<const SWI: u8> SignalExecutor for InterruptExecutor<SWI> {
    fn pend(signal: &'static IrqSignal<Self>) {
        pend_signal::<SWI>(signal);
    }
    fn running() -> bool {
        signal_context::<SWI>()
    }
    fn list() -> &'static SignalList {
        &SIGNAL_LISTS[SWI as usize]
    }
}

impl<const SWI: u8> InterruptExecutor<SWI> {
    /// Start polling in the handler of `wake`'s source, whose table entry
    /// names [`interrupt_handler::<SWI>`](interrupt_handler) on this core.
    /// `order` proves that this executor's level lies between the radio
    /// interrupts' and the application's.
    ///
    /// # Panics
    ///
    /// When the table routes the source to another core, or the software
    /// interrupt already serves an executor.
    pub fn start<W, High, Low>(
        &'static mut self,
        wake: W,
        order: LevelOrder<High, W, Low>,
    ) -> SendSpawner
    where
        W: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
        High: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
        Low: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
    {
        const {
            assert!(
                W::SOURCE as u16 == software_interrupt_source(SWI) as u16,
                "the token is not this executor's software interrupt"
            )
        };
        let _ = order;
        EMBASSY_EXECUTOR_CORE[SWI as usize]
            .compare_exchange(
                UNASSIGNED_CORE,
                Cpu::current() as usize,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .unwrap_or_else(|_| panic!("software interrupt {SWI} is already used by an executor"));
        let inner: &'static raw::Executor = &self.inner;
        // Published before the source is enabled, so the first handler finds
        // the executor and a pend raises its interrupt.
        EMBASSY_EXECUTORS_MODE[SWI as usize].store(true, Ordering::Release);
        EMBASSY_EXECUTORS[SWI as usize].store(inner.id(), Ordering::Release);
        if let Err(error) = oer_espressif_esp_hal_irq::enable(&wake) {
            panic!("executor software interrupt {SWI}: {error:?}");
        }
        inner.spawner().make_send()
    }
}

macro_rules! impl_executor_constructors {
    ($number:literal, $variant:ident) => {
        impl Executor<$number> {
            pub fn new(interrupt: SoftwareInterrupt<'static, $number>) -> Self {
                let (sender, ack) = interrupt.split();
                install_line(
                    &SENDERS[$number],
                    &ACKNOWLEDGERS[$number],
                    &ACK_STORAGE[$number],
                    Sender::$variant(sender),
                    Acknowledger::$variant(ack),
                );
                Self {
                    inner: raw::Executor::new((THREAD_MODE_CONTEXT + $number) as *mut ()),
                    not_send: PhantomData,
                }
            }
        }

        impl InterruptExecutor<$number> {
            pub fn new(interrupt: SoftwareInterrupt<'static, $number>) -> Self {
                let (sender, ack) = interrupt.split();
                install_line(
                    &SENDERS[$number],
                    &ACKNOWLEDGERS[$number],
                    &ACK_STORAGE[$number],
                    Sender::$variant(sender),
                    Acknowledger::$variant(ack),
                );
                Self {
                    inner: raw::Executor::new((INTERRUPT_MODE_CONTEXT + $number) as *mut ()),
                }
            }
        }
    };
}

impl_executor_constructors!(0, Zero);
impl_executor_constructors!(1, One);
impl_executor_constructors!(2, Two);
impl_executor_constructors!(3, Three);

/// The peripheral interrupt source of software interrupt `SWI`.
pub(crate) const fn software_interrupt_source(swi: u8) -> Interrupt {
    match swi {
        0 => Interrupt::FROM_CPU_INTR0,
        1 => Interrupt::FROM_CPU_INTR1,
        2 => Interrupt::FROM_CPU_INTR2,
        _ => Interrupt::FROM_CPU_INTR3,
    }
}

/// The interrupt-table handler of a thread-mode executor's software interrupt
/// (`FROM_CPU_INTR<SWI>`): it wakes the executor.
#[esp_hal::ram]
pub fn wake_handler<const SWI: u8>() {
    acknowledge_interrupt(SWI);
}

/// The interrupt-table handler of an interrupt-mode executor's software
/// interrupt (`FROM_CPU_INTR<SWI>`): it serves the executor's timers and polls
/// it.
#[esp_hal::ram]
pub fn interrupt_handler<const SWI: u8>() {
    acknowledge_interrupt(SWI);
    let id = EMBASSY_EXECUTORS[SWI as usize].load(Ordering::Acquire);
    if id == 0 {
        return;
    }
    #[allow(
        unsafe_code,
        reason = "the published id is the address of the started 'static executor"
    )]
    // SAFETY: `start` published the address of its `&'static raw::Executor`
    // before it enabled this source; `raw::Executor::id` is that address.
    let executor = unsafe { &*(id as *const raw::Executor) };
    as_running(SWI, || {
        crate::time_driver::service(SWI);
        // Raised signals wake their tasks here, in this context, before
        // the poll; a signal raised meanwhile raises this interrupt again.
        oer_interrupt_table::signal::service::<InterruptExecutor<SWI>>();
        #[allow(
            unsafe_code,
            reason = "only this software interrupt's handler polls the executor"
        )]
        // SAFETY: only this handler polls the executor, and the handler does
        // not reenter itself: a raise during the poll is taken after it
        // returns.
        unsafe {
            executor.poll()
        };
    });
}

#[inline(always)]
pub(crate) fn mark_work(swi: u8) {
    EMBASSY_WORK_PENDING.0[swi as usize].store(true, Ordering::Release);
}

/// Make executor `swi` poll again: a thread-mode executor sees its work flag
/// at its next wake, an interrupt-mode executor's handler runs.
pub(crate) fn pend(swi: u8) {
    if EMBASSY_EXECUTORS_MODE[swi as usize].load(Ordering::Acquire) {
        raise_interrupt(swi);
        return;
    }
    mark_work(swi);
    let target_core = EMBASSY_EXECUTOR_CORE[swi as usize].load(Ordering::Acquire);
    if target_core != UNASSIGNED_CORE && target_core != Cpu::current() as usize {
        raise_interrupt(swi);
    }
}

/// Whether each software interrupt serves an interrupt-mode executor.
static EMBASSY_EXECUTORS_MODE: [AtomicBool; SOFTWARE_INTERRUPT_COUNT] =
    [const { AtomicBool::new(false) }; SOFTWARE_INTERRUPT_COUNT];

fn wait_for_work<const SWI: u8>() {
    riscv::interrupt::free(|| {
        if !EMBASSY_WORK_PENDING.0[SWI as usize].load(Ordering::Acquire) {
            #[cfg(oer_profile)]
            crate::idle_accounting::add_wait(SWI as usize);
            #[cfg(not(oer_profile))]
            esp_hal::interrupt::wait_for_interrupt();
        }
    });
}

/// The core executor `executor` runs on, once it runs.
#[cfg(oer_profile)]
pub(crate) fn executor_core(executor: usize) -> Option<usize> {
    let core = EMBASSY_EXECUTOR_CORE[executor].load(Ordering::Acquire);
    (core != UNASSIGNED_CORE).then_some(core)
}

/// Raise the software interrupt of the running executor `executor`, waking
/// its core from its wait.
#[cfg(oer_profile)]
pub(crate) fn raise(executor: usize) {
    if executor_core(executor).is_some() {
        raise_interrupt(executor as u8);
    }
}

#[esp_hal::ram]
#[allow(
    unsafe_code,
    reason = "Embassy requires this unique global pender ABI symbol"
)]
#[unsafe(export_name = "__pender")]
fn embassy_pender(context: *mut ()) {
    let context = context as usize;
    let swi = (context % THREAD_MODE_CONTEXT) as u8;
    match context {
        16..=19 | 32..=35 => pend(swi),
        _ => unreachable!("invalid Embassy executor context"),
    }
}
