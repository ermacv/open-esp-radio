//! A level task's pend: the store of its line's pending byte by the two
//! raisers its type names, a handler above the task and a task of an
//! executor below it, all on one hart, checked at build.
//!
//! A wake from the same hart is the CLIC's pend of the woken line: the
//! chip's handle ([`PendLine`]) stores the line's pending byte with the
//! proof of the hart. The handler's raise is proven by its entry's witness
//! ([`Here`]); the task's is named by the pend's type and takes the proof
//! its hart's runtime gives it.

use core::marker::PhantomData;

use oer_portable_riscv::OnHart;

use crate::{Bound, Class, Entry, ExecutorEntry, Here};

/// The pending byte of one line of hart `H`, fixed at build: the chip's
/// handle, which stores it pended with the hart's proof and reaches no
/// other line.
pub trait PendLine<const H: u8>: Sync {
    /// The line whose pending byte the handle stores.
    const LINE: u8;
    /// Store the line's pending byte as pended.
    fn pend(&self, hart: &OnHart<H>);
}

/// A handler pends the task through its witness, and a task of the
/// executor with its hart's proof:
///
/// ```
/// # oer_chips_family_platform_interrupt_table::__fake_pend!(executor 2, keeper 3, handler 4, line 34);
/// # let mut router = oer_chips_family_platform_interrupt_table::Router::<_, 0, _>::install(FakeController::new(0), InterruptTable).unwrap();
/// # let tokens = Interrupts::take().unwrap();
/// # let keeper = router.bind(tokens.keeper, KeeperPart::default()).unwrap();
/// # let timer = router.bind(tokens.timer, TimerIrq::default()).unwrap();
/// use core::sync::atomic::Ordering::Relaxed;
/// PEND.set(KeeperPend::new(&keeper, FakePendLine)).unwrap();
/// // The timer's vector lends its part and its witness; its handler pends.
/// let vector = INTERRUPT_TABLE.iter().find(|binding| binding.line == 21).unwrap().handler.unwrap();
/// vector();
/// assert_eq!(PENDS.load(Relaxed), 1);
/// PEND.get().unwrap().from_task(&oer_portable_riscv::OnHart::host());
/// assert_eq!(PENDS.load(Relaxed), 2);
/// # let _ = timer;
/// ```
///
/// The build refuses a handler that does not sit above the task:
///
/// ```compile_fail,E0080
/// # oer_chips_family_platform_interrupt_table::__fake_pend!(executor 2, keeper 3, handler 1, line 34);
/// # let mut router = oer_chips_family_platform_interrupt_table::Router::<_, 0, _>::install(FakeController::new(0), InterruptTable).unwrap();
/// # let tokens = Interrupts::take().unwrap();
/// # let keeper = router.bind(tokens.keeper, KeeperPart::default()).unwrap();
/// # let timer = router.bind(tokens.timer, TimerIrq::default()).unwrap();
/// let _ = KeeperPend::new(&keeper, FakePendLine);
/// # let _ = timer;
/// ```
///
/// an executor that does not sit below it:
///
/// ```compile_fail,E0080
/// # oer_chips_family_platform_interrupt_table::__fake_pend!(executor 5, keeper 3, handler 4, line 34);
/// # let mut router = oer_chips_family_platform_interrupt_table::Router::<_, 0, _>::install(FakeController::new(0), InterruptTable).unwrap();
/// # let tokens = Interrupts::take().unwrap();
/// # let keeper = router.bind(tokens.keeper, KeeperPart::default()).unwrap();
/// # let timer = router.bind(tokens.timer, TimerIrq::default()).unwrap();
/// let _ = KeeperPend::new(&keeper, FakePendLine);
/// # let _ = timer;
/// ```
///
/// a handle of another line:
///
/// ```compile_fail,E0080
/// # oer_chips_family_platform_interrupt_table::__fake_pend!(executor 2, keeper 3, handler 4, line 35);
/// # let mut router = oer_chips_family_platform_interrupt_table::Router::<_, 0, _>::install(FakeController::new(0), InterruptTable).unwrap();
/// # let tokens = Interrupts::take().unwrap();
/// # let keeper = router.bind(tokens.keeper, KeeperPart::default()).unwrap();
/// # let timer = router.bind(tokens.timer, TimerIrq::default()).unwrap();
/// let _ = KeeperPend::new(&keeper, FakePendLine);
/// # let _ = timer;
/// ```
///
/// the pend of an entry that is no level task:
///
/// ```compile_fail,E0080
/// # oer_chips_family_platform_interrupt_table::__fake_parts!();
/// # oer_chips_family_platform_interrupt_table::__fake_controller!();
/// # pub struct FakePendLine;
/// # impl oer_chips_family_platform_interrupt_table::PendLine<0> for FakePendLine {
/// #     const LINE: u8 = 18;
/// #     fn pend(&self, _: &oer_portable_riscv::OnHart<0>) {}
/// # }
/// # pub fn on_keeper(_: &mut KeeperPart) {}
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_pair(_: &mut PairIrq) {}
/// oer_chips_family_platform_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio_executor: 2; keeper: 3; radio: 4; }
///     radio: RadioToken = RadioIrq => OnExecutor, line 17, context radio_executor, hart 0, Executor;
///     timer: TimerToken = TimerIrq => on_timer, line 21, context radio, hart 0, Hardware;
///     // A hardware entry, not a level task, between them.
///     pair: PairToken = PairIrq => on_pair, line 18, context keeper, hart 0, Hardware;
/// }
/// # let mut router = oer_chips_family_platform_interrupt_table::Router::<_, 0, _>::install(FakeController::new(0), InterruptTable).unwrap();
/// # let tokens = Interrupts::take().unwrap();
/// let pair = router.bind(tokens.pair, PairIrq::default()).unwrap();
/// let _ = oer_chips_family_platform_interrupt_table::Pend::<PairToken, TimerToken, RadioToken, FakePendLine, 0>::new(&pair, FakePendLine);
/// ```
///
/// or a raiser on another hart:
///
/// ```compile_fail,E0080
/// # oer_chips_family_platform_interrupt_table::__fake_parts!();
/// # oer_chips_family_platform_interrupt_table::__fake_controller!();
/// # pub struct FakePendLine;
/// # impl oer_chips_family_platform_interrupt_table::PendLine<0> for FakePendLine {
/// #     const LINE: u8 = 34;
/// #     fn pend(&self, _: &oer_portable_riscv::OnHart<0>) {}
/// # }
/// # pub fn on_keeper(_: &mut KeeperPart) {}
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_pair(_: &mut PairIrq) {}
/// oer_chips_family_platform_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio_executor: 2; keeper: 3; radio: 4; }
///     radio: RadioToken = RadioIrq => OnExecutor, line 17, context radio_executor, hart 1, Executor;
///     timer: TimerToken = TimerIrq => on_timer, line 21, context radio, hart 0, Hardware;
///     keeper: KeeperToken = KeeperPart => on_keeper, line 34, context keeper, hart 0, LevelTask;
/// }
/// # let mut router = oer_chips_family_platform_interrupt_table::Router::<_, 0, _>::install(FakeController::new(0), InterruptTable).unwrap();
/// # let tokens = Interrupts::take().unwrap();
/// let keeper = router.bind(tokens.keeper, KeeperPart::default()).unwrap();
/// // The radio executor's tasks run on hart 1.
/// let _ = oer_chips_family_platform_interrupt_table::Pend::<KeeperToken, TimerToken, RadioToken, FakePendLine, 0>::new(&keeper, FakePendLine);
/// ```
///
/// Level task `T`'s pend, by its two raisers: the handler of entry `R`
/// above `T` (its raise proven by its witness, [`Pend::from_handler`]) and
/// a task of the executor whose entry is `X`, below `T` (its raise named
/// here, [`Pend::from_task`]); `T`, `R` and `X` on hart `H`, and `L` the
/// handle of `T`'s line. Checked at build ([`Pend::new`]).
pub struct Pend<T, R, X, L, const H: u8> {
    line: L,
    _raisers: PhantomData<fn() -> (T, R, X)>,
}

impl<T, R, X, L, const H: u8> core::fmt::Debug for Pend<T, R, X, L, H> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Pend")
    }
}

impl<T: Entry, R: Entry, X: ExecutorEntry, L: PendLine<H>, const H: u8> Pend<T, R, X, L, H> {
    /// The pend of `T`'s line through `line`, with the proof that `T` is
    /// bound. The build checks that `T` is a level task of hart `H` on
    /// `line`'s line, the handler `R` sits strictly above it and the
    /// executor `X` strictly below it, both on hart `H`.
    pub const fn new(_bound: &Bound<T>, line: L) -> Self {
        const {
            if !matches!(T::CLASS, Class::LevelTask) {
                panic!("a pend wakes a level task's line")
            }
            if T::HART != H || R::HART != H || X::HART != H {
                panic!("a level task and its raisers share its hart")
            }
            if L::LINE != T::LINE {
                panic!("a pend's handle stores its task's line")
            }
            if R::LEVEL <= T::LEVEL {
                panic!("a pending handler sits above its level task")
            }
            if X::LEVEL >= T::LEVEL {
                panic!("a pending task's executor sits below its level task")
            }
        };
        Self {
            line,
            _raisers: PhantomData,
        }
    }

    /// Pend the task from its handler, the raise its entry's witness
    /// proves.
    #[inline(always)]
    pub fn from_handler(&self, here: &Here<'_, R>) {
        self.line.pend(&here.on_hart::<H>());
    }

    /// Pend the task from a task of the executor `X`, with its hart's
    /// proof.
    #[inline(always)]
    pub fn from_task(&self, hart: &OnHart<H>) {
        self.line.pend(hart);
    }
}
