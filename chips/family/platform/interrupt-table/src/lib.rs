#![expect(
    clippy::fn_params_excessive_bools,
    reason = "step 7: P, a level's task"
)]
#![cfg_attr(
    test,
    expect(
        clippy::cast_possible_truncation,
        reason = "test code: a cast or a dropped result in a test loses no product's value"
    )
)]
//! Each image's static table of its interrupt bindings: for every bound
//! peripheral interrupt part its source, CPU line, level, hart, handler and
//! handler class, declared once with [`interrupt_table!`].
//!
//! One line, one part, one direct call: each entry names the CPU line its
//! part's sources are routed to, and that line's vector calls the entry's
//! handler and nothing else. The sources come from the bound part's type
//! ([`InterruptSources`]: one source, or several a handler serves
//! together), never from the entry's text. Every source starts
//! silent ([`Router::install`]); [`Router::bind`] consumes the entry's token
//! and the part, moves the part where only the handler reaches it, writes
//! the line and the route, and returns the [`Bound`] proof a driver asks for
//! before it enables the peripheral's interrupt.
//!
//! ```compile_fail,E0308
//! # oer_chips_family_platform_interrupt_table::__fake_table!();
//! # let mut router = fake_router::<0>();
//! let tokens = Interrupts::take().unwrap();
//! // The timer's entry binds the timer's part, not the radio's.
//! let _ = router.bind(tokens.timer, RadioIrq);
//! ```
//!
//! ```compile_fail,E0382
//! # oer_chips_family_platform_interrupt_table::__fake_table!();
//! # let mut router = fake_router::<0>();
//! # let tokens = Interrupts::take().unwrap();
//! // A part moves into its one binding.
//! let part = TimerIrq::default();
//! let _ = router.bind(tokens.timer, part);
//! let _ = router.bind(tokens.timer, part);
//! ```
//!
//! ```compile_fail,E0423
//! # oer_chips_family_platform_interrupt_table::__fake_table!();
//! fn enable_timer_interrupt(_bound: &oer_chips_family_platform_interrupt_table::Bound<TimerToken>) {}
//! // Only a binding makes the proof the interrupt's enable takes.
//! enable_timer_interrupt(&oer_chips_family_platform_interrupt_table::Bound { _part: core::marker::PhantomData });
//! ```
//!
//! ```compile_fail,E0382
//! # oer_chips_family_platform_interrupt_table::__fake_table!();
//! // A token is unique: it is neither `Copy` nor `Clone`.
//! let tokens = Interrupts::take().unwrap();
//! let first = tokens.timer;
//! let second = tokens.timer;
//! ```
//!
//! The same fake binds its own part with its own token:
//!
//! ```
//! # oer_chips_family_platform_interrupt_table::__fake_table!();
//! # let mut router = fake_router::<0>();
//! let tokens = Interrupts::take().unwrap();
//! let bound = router.bind(tokens.timer, TimerIrq::default()).unwrap();
//! fn enable_timer_interrupt(_bound: &oer_chips_family_platform_interrupt_table::Bound<TimerToken>) {}
//! enable_timer_interrupt(&bound);
//! ```
#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]
#![expect(clippy::arithmetic_side_effects, reason = "step 7: P, a level's task")]

mod binding;
pub use binding::{BindFault, BindFaultCode, BindFaultKind, Quarantined};
mod here;
pub use here::Here;
mod pend;
pub use pend::{Pend, PendLine};
mod router;
pub use router::{
    Bound, Controller, Error, EventAuthority, LineExpectation, LineObservation, Router, Silent,
};

pub mod signal;

pub use oer_portable_mmio::{InterruptSource, InterruptSources};

/// What an entry's handler may do. The image gate checks each interrupt
/// context's code by the classes of its entries; [`interrupt_table!`] checks
/// at compile time where each class may sit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Class {
    /// Captures, acknowledges, latches and raises signals: strict code from
    /// SRAM or ROM with its data in SRAM, no task poll and no waker call.
    /// Never on the level of its hart's [`Class::Executor`].
    Hardware = 0,
    /// An executor's interrupt: serves its signals and timers, then polls its
    /// tasks. Strict up to the poll, its tasks by the task rules; one per
    /// level and hart. Its entry names a handler type ([`ExecutorHandler`]),
    /// not a function, which its token states ([`ExecutorEntry`]).
    Executor = 1,
    /// A [`Class::Hardware`] handler on the level of its hart's
    /// [`Class::Executor`], the one rule it breaks; declared
    /// `Exempt(ExecutorLevel, "reason")`. The entry names its reason, which
    /// the image gate's report prints.
    ExemptExecutorLevel = 2,
    /// A [`Class::Hardware`] handler that calls a waker, the one rule it
    /// breaks; declared `Exempt(WakerCall, "reason")`, never on its hart's
    /// executor level.
    ExemptWakerCall = 3,
    /// A level's one task: an entry whose part has no source, on a software
    /// line of its own that its raisers pend; strict code as
    /// [`Class::Hardware`], run to its end on each entry. Alone on its level
    /// of its hart, never on an executor's level. Hardware reaches it only
    /// through a handler above it, which pends it: a timer a task one day
    /// needs is a `Hardware` entry that pends it, which is why a task's
    /// part never lists a source. The image exports it in its own table
    /// ([`TaskBinding`]), never in the source table: no element of
    /// `__OER_INTERRUPT_TABLE` carries this byte.
    LevelTask = 4,
}

/// One entry of an image's interrupt table, laid out for the tools that read
/// it from the image: source `u16` at 0, line at 2, level at 3, class at 4,
/// hart at 5, the handler's word at 8, 12 bytes in all.
#[derive(Debug)]
#[repr(C)]
pub struct Binding {
    /// The interrupt-matrix source, from the bound part's type.
    pub source: u16,
    /// The CPU line the source is routed to; its vector calls `handler`.
    pub line: u8,
    /// The line's level, 1 for the lowest.
    pub level: u8,
    pub class: Class,
    pub hart: u8,
    /// The function the line's vector calls; `None` (a zero word) for an
    /// entry a `cfg` leaves out of the image.
    pub handler: Option<extern "C" fn()>,
}

const _: () = {
    use core::mem::{offset_of, size_of};
    if offset_of!(Binding, source) != 0 {
        panic!("a binding's source is at byte 0")
    }
    if offset_of!(Binding, line) != 2 {
        panic!("a binding's line is at byte 2")
    }
    if offset_of!(Binding, level) != 3 {
        panic!("a binding's level is at byte 3")
    }
    if offset_of!(Binding, class) != 4 || size_of::<Class>() != 1 {
        panic!("a binding's class is one byte at byte 4")
    }
    if offset_of!(Binding, hart) != 5 {
        panic!("a binding's hart is at byte 5")
    }
    if offset_of!(Binding, handler) != 8 {
        panic!("a binding's handler is at byte 8")
    }
    if size_of::<Binding>() != 8 + size_of::<usize>() {
        panic!("a binding is eight bytes and a handler word")
    }
};

/// One [`Class::LevelTask`] entry of an image's table, laid out for the
/// tools that read it from the image: line at 0, level at 1, hart at 2, the
/// handler's word at the next word (byte 4 on the chip), two words in all.
/// A level task has no source: its element has no source field and no
/// class (the table is its class), and its line is a software line that
/// only its raisers pend.
#[derive(Debug)]
#[repr(C)]
pub struct TaskBinding {
    /// The task's line, which no source is routed to; its vector calls
    /// `handler`.
    pub line: u8,
    /// The task's level, 1 for the lowest.
    pub level: u8,
    pub hart: u8,
    /// The function the line's vector calls; `None` (a zero word) for a
    /// task a `cfg` leaves out of the image, as [`Binding::handler`] is for
    /// an entry.
    pub handler: Option<extern "C" fn()>,
}

const _: () = {
    use core::mem::{offset_of, size_of};
    if offset_of!(TaskBinding, line) != 0 {
        panic!("a level task's line is at byte 0")
    }
    if offset_of!(TaskBinding, level) != 1 {
        panic!("a level task's level is at byte 1")
    }
    if offset_of!(TaskBinding, hart) != 2 {
        panic!("a level task's hart is at byte 2")
    }
    if offset_of!(TaskBinding, handler) != size_of::<usize>() {
        panic!("a level task's handler is its second word")
    }
    if size_of::<TaskBinding>() != 2 * size_of::<usize>() {
        panic!("a level task is two words")
    }
};

/// An image's table, as a type: [`interrupt_table!`] declares it
/// (`InterruptTable`) beside its entries, and [`NoBindings`] is the table of
/// none. Sealed: only the macro and this crate implement it, so an entry's
/// table ([`Entry::Table`]) is the one its declaration lists it in.
pub trait Table: __private::SealedTable + Copy + 'static {
    /// Every entry of the table, as its declaration lists it.
    #[doc(hidden)]
    const DECLARED: &'static [__private::Declared];
    /// Every [`Class::LevelTask`] entry of the table, as the image exports
    /// them: a task a `cfg` leaves out has no handler.
    #[doc(hidden)]
    const TASKS: &'static [TaskBinding];
    /// The table's bindings, as the image exports them.
    fn bindings(self) -> &'static [Binding];
}

/// The table with no entry: a session or an image that binds nothing.
#[derive(Clone, Copy, Debug)]
pub struct NoBindings;

impl __private::SealedTable for NoBindings {}

impl Table for NoBindings {
    const DECLARED: &'static [__private::Declared] = &[];
    const TASKS: &'static [TaskBinding] = &[];
    fn bindings(self) -> &'static [Binding] {
        &[]
    }
}

/// The token of one table entry: the only way to bind its part.
///
/// [`interrupt_table!`] implements it: the image's table lists each
/// of the part's sources with `LINE`, `LEVEL` and `HART`, the entry's handler
/// reaches its part through `cell` alone, and the type has one value.
/// Every implementation supplies `EntryProof<Self>`. Root Clippy policy
/// restricts both proof and token constructors to the macro; a proof of
/// another entry does not type-check. An explicit lint waiver can still lie
/// about scheduling metadata, which compile-time context checks trust.
/// [`Router::bind`] checks table metadata, class and the proof's handler/cell
/// identity before placement. Storage and handler cells enforce memory safety
/// independently of token origin; claimed storage is refused in quarantine.
pub trait Entry: Sized {
    /// Type-specific declaration proof, issued only by the table macro.
    #[doc(hidden)]
    const PROOF: __private::EntryProof<Self>;
    /// The interrupt part the entry's handler holds.
    type Part: InterruptSources + Send + 'static;
    /// The table that declares the entry: a router binds it only over that
    /// table.
    type Table: Table;
    const LINE: u8;
    const LEVEL: u8;
    const HART: u8;
    const CLASS: Class;
    const CONTEXT: Context = Context {
        level: Self::LEVEL,
        hart: Self::HART,
    };
    /// The place the bound part lives in.
    #[doc(hidden)]
    fn storage() -> &'static static_cell::StaticCell<Self::Part>;
}

/// The handler of a [`Class::Executor`] entry, named by type: one round of
/// the executor the entry runs. The entry's token states it
/// ([`ExecutorEntry::Handler`]), so an executor's start can check at compile
/// time that the entry it binds polls the task set it starts.
pub trait ExecutorHandler<P> {
    /// The round, with the entry's part.
    fn handle(part: &mut P);
}

/// The token of a [`Class::Executor`] entry: [`interrupt_table!`]
/// implements it with the handler type the entry names.
///
/// ```
/// # oer_chips_family_platform_interrupt_table::__fake_table!();
/// fn polls<W: oer_chips_family_platform_interrupt_table::ExecutorEntry<Handler = OnExecutor>>() {}
/// polls::<RadioToken>();
/// ```
///
/// ```compile_fail,E0277
/// # oer_chips_family_platform_interrupt_table::__fake_table!();
/// fn polls<W: oer_chips_family_platform_interrupt_table::ExecutorEntry>() {}
/// // A hardware entry runs no executor.
/// polls::<TimerToken>();
/// ```
///
/// ```compile_fail,E0573
/// # oer_chips_family_platform_interrupt_table::__fake_parts!();
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// oer_chips_family_platform_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio_executor: 2; }
///     // An executor entry names its round's type, not a function.
///     radio: RadioToken = RadioIrq => on_radio, line 17, context radio_executor, hart 0, Executor;
/// }
/// ```
pub trait ExecutorEntry: Entry {
    type Handler: ExecutorHandler<Self::Part>;
}

/// Scheduling metadata; register ownership is carried by the interrupt part.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Context {
    pub level: u8,
    pub hart: u8,
}

/// Whether a request handler preempts its executor on the same hart.
/// Binding APIs assert this predicate in inline const blocks.
#[must_use]
pub const fn request_direction(handler: Context, executor: Context) -> bool {
    handler.hart == executor.hart && handler.level > executor.level
}

/// Arbitration among pending lines of the same level, from the chip facts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TieBreak {
    HighestLine,
}

/// The layout of a chip's CPU lines, which a chip's platform passes to
/// [`interrupt_table!`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Lines {
    /// The first line a peripheral source may be routed to.
    pub first: u8,
    /// The lines: `0..count`.
    pub count: u8,
    /// The levels a line can hold: `1..=levels`.
    pub levels: u8,
    /// The harts: `0..harts`.
    pub harts: u8,
    /// Arbitration at equal level, generated from the chip facts.
    pub tie_break: TieBreak,
}

/// Declare an image's interrupt table: once per image, since it defines
/// `INTERRUPT_TABLE` (exported as `__OER_INTERRUPT_TABLE` for the tools that
/// read the image) and, for each entry, the strong symbol
/// `__oer_interrupt_h<HART>_l<LINE>` that the line's vector calls. The image
/// hands `INTERRUPT_TABLE` to its platform's runtime.
///
/// A chip's platform wraps it with its [`Lines`]; each entry reads
/// `field: Token = Part => handler, line LINE, context NAME, hart HART, CLASS;`
/// after `contexts { NAME: LEVEL; ... }`.
/// The levels are declared once; same-level entries on a hart are written
/// from lowest to highest arbitration priority and checked against `Lines::tie_break`. `Part` implements [`InterruptSources`] (each
/// [`InterruptSource`] does), `handler`
/// is a `fn(&mut Part)` or, for an `Executor` entry, a type implementing
/// [`ExecutorHandler<Part>`](ExecutorHandler) that the token states
/// ([`ExecutorEntry`]), and the macro defines the token type `Token`, while
/// `Interrupts::take` hands out every token once. `CLASS` is a [`Class`]:
/// `Hardware`, `Executor` or `Exempt(VIOLATION, "reason")`, where
/// `VIOLATION` names the one hardware rule the entry breaks
/// (`ExecutorLevel` or `WakerCall`); an exempt entry's reason is exported as
/// `__OER_INTERRUPT_EXEMPT_` followed by the handler's symbol for the image
/// gate's report.
///
/// A level's task is an entry of class `LevelTask` whose part lists no
/// source ([`InterruptSources::SOURCES`] empty): the same token, proof,
/// cell, storage and [`Router::bind`], its handler a `fn(&mut Part)`. Its
/// line is a software line its raisers pend; the router binds it as a
/// task's ([`Controller::bind_task`]) and routes nothing to it. The image
/// exports it in `LEVEL_TASK_TABLE` (`__OER_LEVEL_TASK_TABLE`), one
/// [`TaskBinding`] each, and never in the source table.
///
/// The table fails to compile when a line lies outside
/// `lines.first..lines.count`, a declared context level outside `1..=lines.levels` or a hart
/// outside `0..lines.harts`; when two entries share a line and hart, or
/// two entries, or one part, list a source twice; when two `Executor` entries share a level and hart, a `Hardware`
/// or `Exempt(WakerCall, ..)` entry sits on the level of its hart's
/// `Executor`, or an `Exempt(ExecutorLevel, ..)` entry does not. An entry's
/// `#[cfg(...)]` attributes, before its doc comment, leave all of it out:
/// token, handler symbol, reason and table entry. A `LevelTask` entry's
/// part lists no source and every other entry's lists at least one; one
/// `LevelTask` a level and hart; and no entry of another class (an
/// `Executor` or one with a source) sits on the level of its hart's task.
///
/// ```compile_fail,E0080
/// # oer_chips_family_platform_interrupt_table::__fake_parts!();
/// # pub fn on_keeper(_: &mut KeeperPart) {}
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// # pub fn on_other(_: &mut OtherPart) {}
/// # #[derive(Default)] pub struct OtherPart;
/// # impl oer_chips_family_platform_interrupt_table::InterruptSources for OtherPart { const SOURCES: &'static [u16] = &[]; }
/// oer_chips_family_platform_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     // One task a level and hart.
///     contexts { keeper: 3; }
///     keeper: KeeperToken = KeeperPart => on_keeper, line 34, context keeper, hart 0, LevelTask;
///     other: OtherToken = OtherPart => on_other, line 35, context keeper, hart 0, LevelTask;
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_chips_family_platform_interrupt_table::__fake_parts!();
/// # pub fn on_keeper(_: &mut KeeperPart) {}
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// oer_chips_family_platform_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     // A task never sits on an executor's level.
///     contexts { radio_executor: 2; }
///     radio: RadioToken = RadioIrq => OnExecutor, line 17, context radio_executor, hart 0, Executor;
///     keeper: KeeperToken = KeeperPart => on_keeper, line 34, context radio_executor, hart 0, LevelTask;
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_chips_family_platform_interrupt_table::__fake_parts!();
/// # pub fn on_keeper(_: &mut KeeperPart) {}
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// oer_chips_family_platform_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     // A task's level runs the task and nothing else: no source there.
///     contexts { keeper: 3; }
///     timer: TimerToken = TimerIrq => on_timer, line 16, context keeper, hart 0, Hardware;
///     keeper: KeeperToken = KeeperPart => on_keeper, line 34, context keeper, hart 0, LevelTask;
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_chips_family_platform_interrupt_table::__fake_parts!();
/// # pub fn on_keeper(_: &mut KeeperPart) {}
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// oer_chips_family_platform_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     // A task's part lists no source: hardware reaches it through a handler.
///     contexts { keeper: 3; }
///     timer: TimerToken = TimerIrq => on_timer, line 34, context keeper, hart 0, LevelTask;
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_chips_family_platform_interrupt_table::__fake_parts!();
/// # pub fn on_keeper(_: &mut KeeperPart) {}
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// oer_chips_family_platform_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     // Every other entry's part lists a source.
///     contexts { radio: 4; }
///     keeper: KeeperToken = KeeperPart => on_keeper, line 34, context radio, hart 0, Hardware;
/// }
/// ```
///
/// A task on each hart at one level, and entries of the task's hart on
/// other levels, compile; neither task has an element in the source table:
///
/// ```
/// # oer_chips_family_platform_interrupt_table::__fake_parts!();
/// # pub fn on_keeper(_: &mut KeeperPart) {}
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// # pub fn on_keeper1(_: &mut Keeper1Part) {}
/// # pub struct Keeper1Part;
/// # impl oer_chips_family_platform_interrupt_table::InterruptSources for Keeper1Part { const SOURCES: &'static [u16] = &[]; }
/// oer_chips_family_platform_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio_executor: 2; keeper: 3; radio: 4; }
///     radio: RadioToken = RadioIrq => OnExecutor, line 17, context radio_executor, hart 0, Executor;
///     timer: TimerToken = TimerIrq => on_timer, line 21, context radio, hart 0, Hardware;
///     keeper: KeeperToken = KeeperPart => on_keeper, line 34, context keeper, hart 0, LevelTask;
///     keeper1: Keeper1Token = Keeper1Part => on_keeper1, line 34, context keeper, hart 1, LevelTask;
/// }
/// assert_eq!(<KeeperToken as oer_chips_family_platform_interrupt_table::Entry>::CLASS, oer_chips_family_platform_interrupt_table::Class::LevelTask);
/// assert_eq!(LEVEL_TASK_TABLE.len(), 2);
/// assert!(INTERRUPT_TABLE.iter().all(|binding| binding.line != 34));
/// ```
///
/// ```compile_fail,E0080
/// # oer_chips_family_platform_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// oer_chips_family_platform_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio_executor: 2; }
///     radio: RadioToken = RadioIrq => OnExecutor, line 17, context radio_executor, hart 0, Executor;
///     // A hardware handler would wait for the executor's whole poll.
///     timer: TimerToken = TimerIrq => on_timer, line 16, context radio_executor, hart 0, Hardware;
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_chips_family_platform_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// oer_chips_family_platform_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio_executor: 2; }
///     // One executor per level and hart.
///     radio: RadioToken = RadioIrq => OnExecutor, line 17, context radio_executor, hart 0, Executor;
///     timer: TimerToken = TimerIrq => OnExecutor, line 16, context radio_executor, hart 0, Executor;
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_chips_family_platform_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// oer_chips_family_platform_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio_executor: 2; }
///     // Nothing to exempt: no executor shares the level.
///     timer: TimerToken = TimerIrq => on_timer, line 16, context radio_executor, hart 0, Exempt(ExecutorLevel, "no executor here");
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_chips_family_platform_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// oer_chips_family_platform_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio_executor: 2; }
///     radio: RadioToken = RadioIrq => OnExecutor, line 17, context radio_executor, hart 0, Executor;
///     // A waker exemption does not exempt the executor's level too.
///     timer: TimerToken = TimerIrq => on_timer, line 16, context radio_executor, hart 0, Exempt(WakerCall, "its driver");
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_chips_family_platform_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// oer_chips_family_platform_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio: 3; }
///     // One line, one source: a line's vector calls one handler.
///     radio: RadioToken = RadioIrq => on_radio, line 16, context radio, hart 0, Hardware;
///     timer: TimerToken = TimerIrq => on_timer, line 16, context radio, hart 0, Hardware;
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_chips_family_platform_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_again(_: &mut TimerIrq) {}
/// oer_chips_family_platform_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio: 3; }
///     // One source, one binding: the same part type binds once.
///     timer: TimerToken = TimerIrq => on_timer, line 16, context radio, hart 0, Hardware;
///     again: AgainToken = TimerIrq => on_again, line 17, context radio, hart 0, Hardware;
/// }
/// ```
///
/// A part of several sources ([`InterruptSources`]) binds them all to its
/// entry's one line; no other entry, and no other position of its own list,
/// names one of them:
///
/// ```compile_fail,E0080
/// # oer_chips_family_platform_interrupt_table::__fake_parts!();
/// # pub fn on_pair(_: &mut PairIrq) {}
/// # pub fn on_seven(_: &mut SevenIrq) {}
/// pub struct SevenIrq;
/// impl oer_chips_family_platform_interrupt_table::InterruptSource for SevenIrq {
///     const SOURCE: u16 = 7;
/// }
/// oer_chips_family_platform_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio: 3; }
///     // The pair's handler serves source 7 already.
///     pair: PairToken = PairIrq => on_pair, line 16, context radio, hart 0, Hardware;
///     seven: SevenToken = SevenIrq => on_seven, line 17, context radio, hart 0, Hardware;
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_chips_family_platform_interrupt_table::__fake_parts!();
/// # pub fn on_twice(_: &mut TwiceIrq) {}
/// pub struct TwiceIrq;
/// impl oer_chips_family_platform_interrupt_table::InterruptSources for TwiceIrq {
///     const SOURCES: &'static [u16] = &[4, 4];
/// }
/// oer_chips_family_platform_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio: 3; }
///     // One part lists source 4 twice.
///     twice: TwiceToken = TwiceIrq => on_twice, line 16, context radio, hart 0, Hardware;
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_chips_family_platform_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// oer_chips_family_platform_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio: 3; }
///     // Lines below the first external one are the core's own.
///     timer: TimerToken = TimerIrq => on_timer, line 7, context radio, hart 0, Hardware;
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_chips_family_platform_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// oer_chips_family_platform_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio: 3; unused: 9; }
///     // Even an unused context must fit the facts' eight levels.
///     timer: TimerToken = TimerIrq => on_timer, line 16, context radio, hart 0, Hardware;
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_chips_family_platform_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// oer_chips_family_platform_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio: 3; }
///     // The fake has two harts.
///     timer: TimerToken = TimerIrq => on_timer, line 16, context radio, hart 2, Hardware;
/// }
/// ```
///
/// ```compile_fail,E0308
/// # oer_chips_family_platform_interrupt_table::__fake_parts!();
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// oer_chips_family_platform_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio: 3; }
///     // A handler gets exactly its entry's part.
///     timer: TimerToken = TimerIrq => on_radio, line 16, context radio, hart 0, Hardware;
/// }
/// ```
///
/// The same entries on other harts, lines and levels, or an exemption
/// beside the executor, compile:
///
/// ```
/// # oer_chips_family_platform_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// # pub fn on_absent(_: &mut AbsentIrq) {}
/// # pub fn on_pair(_: &mut PairIrq) {}
/// oer_chips_family_platform_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio_executor: 2; radio: 3; }
///     radio: RadioToken = RadioIrq => OnExecutor, line 16, context radio_executor, hart 0, Executor;
///     timer: TimerToken = TimerIrq => on_timer, line 16, context radio_executor, hart 1, Hardware;
///     pair: PairToken = PairIrq => on_pair, line 17, context radio, hart 1, Hardware;
///     absent: AbsentToken = AbsentIrq => on_absent, line 47, context radio_executor, hart 0, Exempt(ExecutorLevel, "until its port");
/// }
/// ```
#[macro_export]
macro_rules! interrupt_table {
    (
        lines: $lines:expr,
        handler_attributes: $handler_attributes:tt;
        contexts { $($context_name:ident: $context_level:literal;)* }
        $(
            $(#[cfg($cfg:meta)])*
            $(#[doc = $doc:literal])*
            $field:ident: $token:ident = $part:ty => $handler:path $(, with $here:ident)?,
                line $line:literal, context $context:ident, hart $hart:literal,
                $class:ident $(($violation:ident, $reason:literal))?;
        )*
    ) => {
        /// The image's named execution levels. Entries select one by name.
        #[allow(non_upper_case_globals, reason = "image contexts are named by role")]
        mod contexts {
            $(pub const $context_name: u8 = $context_level;)*
        }
        $(
            $(#[cfg($cfg)])*
            $(#[doc = $doc])*
            #[must_use = "a dropped token can never bind its part"]
            pub struct $token($crate::__private::Unique);

            $(#[cfg($cfg)])*
            impl $crate::Entry for $token {
                type Part = $part;
                type Table = InterruptTable;
                const PROOF: $crate::__private::EntryProof<Self> = {
                    static CELL: $crate::__private::HandlerCell<$part> = $crate::__private::HandlerCell::new();
                    #[allow(clippy::disallowed_methods, reason = "the table owns its handler and cell mapping")]
                    let proof = $crate::__private::EntryProof::new(Self::__handler, &CELL);
                    proof
                };
                const LINE: u8 = $line;
                const LEVEL: u8 = contexts::$context;
                const HART: u8 = $hart;
                const CLASS: $crate::Class = $crate::__class!($class $($violation)?);
                fn storage() -> &'static $crate::__private::StaticCell<$part> {
                    static STORAGE: $crate::__private::StaticCell<$part> =
                        $crate::__private::StaticCell::new();
                    &STORAGE
                }
            }

            $(#[cfg($cfg)])*
            $crate::__executor_entry!($class, $token, $handler);

            $(#[cfg($cfg)])*
            const _: () = {
                $crate::__exemption!($hart, $line $(, $reason)?);
            };

            $(#[cfg($cfg)])*
            impl $token {
                $crate::__interrupt_handler!($handler_attributes, $token, $class, $handler, [$($here)?], $hart, $line);
            }
        )*

        /// Every entry of the table, as its rules and its image layout
        /// read it.
        const __OER_DECLARED: &[$crate::__private::Declared] = &[
            $($crate::__private::Declared {
                sources: {
                    #[cfg(all($($cfg),*))]
                    let sources = <$part as $crate::InterruptSources>::SOURCES;
                    #[cfg(not(all($($cfg),*)))]
                    let sources: &[u16] = &[];
                    sources
                },
                present: cfg!(all($($cfg),*)),
                binding: $crate::Binding {
                    source: 0,
                    line: $line,
                    level: contexts::$context,
                    class: $crate::__class!($class $($violation)?),
                    hart: $hart,
                    handler: {
                        #[cfg(all($($cfg),*))]
                        let handler = ::core::option::Option::Some($token::__handler as extern "C" fn());
                        #[cfg(not(all($($cfg),*)))]
                        let handler = ::core::option::Option::None;
                        handler
                    },
                },
            },)*
        ];

        /// Every level task of the table, as the image exports them.
        const __OER_TASKS: &[$crate::TaskBinding] =
            &$crate::__private::task_bindings::<{ $crate::__private::task_count(__OER_DECLARED) }>(__OER_DECLARED);

        const _: () = if let Err(why) = $crate::__private::check($lines, &[$(contexts::$context_name),*], __OER_DECLARED) {
            panic!("{}", why)
        };

        /// Every token of the image's interrupt table.
        #[allow(dead_code, reason = "an image without bindings takes no token")]
        pub struct Interrupts {
            $($(#[cfg($cfg)])* pub $field: $token,)*
        }

        #[allow(dead_code, reason = "an image without bindings takes no token")]
        impl Interrupts {
            /// The tokens, once per image.
            pub fn take() -> ::core::option::Option<Self> {
                static TAKEN: ::core::sync::atomic::AtomicBool =
                    ::core::sync::atomic::AtomicBool::new(false);
                if TAKEN.swap(true, ::core::sync::atomic::Ordering::AcqRel) {
                    return ::core::option::Option::None;
                }
                ::core::option::Option::Some(Self {
                    $($(#[cfg($cfg)])* $field: $token({
                        #[allow(
                            clippy::disallowed_methods,
                            reason = "the table's once gate admits one token per entry and image"
                        )]
                        let unique = $crate::__private::Unique::new();
                        unique
                    }),)*
                })
            }
        }

        /// The bindings of the table: one per source of each entry the
        /// build holds, one without a handler for each it leaves out.
        static __OER_BINDINGS: [$crate::Binding; $crate::__private::count(__OER_DECLARED)] =
            $crate::__private::bindings(__OER_DECLARED);

        /// The image's interrupt table, as the type its entries and its
        /// router name.
        #[derive(Clone, Copy, Debug)]
        pub struct InterruptTable;

        impl $crate::__private::SealedTable for InterruptTable {}

        impl $crate::Table for InterruptTable {
            const DECLARED: &'static [$crate::__private::Declared] = __OER_DECLARED;
            const TASKS: &'static [$crate::TaskBinding] = __OER_TASKS;
            fn bindings(self) -> &'static [$crate::Binding] {
                &__OER_BINDINGS
            }
        }

        $crate::__private::link_fn! {
            table $crate::__private::TableSlot<$crate::Binding> =>
            /// The image's interrupt table. `#[used]` keeps the exported slice
            /// in the image: reads of an immutable static are folded into their
            /// uses, which would leave the tools no symbol to find it by.
            #[used]
            pub static INTERRUPT_TABLE: &[$crate::Binding] = &__OER_BINDINGS;
        }

        $crate::__private::link_fn! {
            task_table $crate::__private::TableSlot<$crate::TaskBinding> =>
            /// The image's level tasks, for the tools as the interrupt table
            /// is; empty in an image without one.
            #[used]
            pub static LEVEL_TASK_TABLE: &[$crate::TaskBinding] = __OER_TASKS;
        }
    };
}

/// The sources of a part made of several members, for its
/// [`InterruptSources::SOURCES`]: every member's sources, in member order,
/// as a `&'static [u16]` built at compile time. A source two members list,
/// or one member lists twice, fails to compile.
///
/// ```
/// # oer_chips_family_platform_interrupt_table::__fake_parts!();
/// /// One handler's timer and radio banks together.
/// pub struct RadioAndTimer {
///     pub timer: TimerIrq,
///     pub radio: RadioIrq,
/// }
/// impl oer_chips_family_platform_interrupt_table::InterruptSources for RadioAndTimer {
///     const SOURCES: &'static [u16] =
///         oer_chips_family_platform_interrupt_table::interrupt_sources![TimerIrq, RadioIrq, PairIrq];
/// }
/// assert_eq!(<RadioAndTimer as oer_chips_family_platform_interrupt_table::InterruptSources>::SOURCES, [3, 5, 4, 7]);
/// ```
///
/// ```compile_fail,E0080
/// # oer_chips_family_platform_interrupt_table::__fake_parts!();
/// pub struct Twice;
/// impl oer_chips_family_platform_interrupt_table::InterruptSources for Twice {
///     // The timer's source twice.
///     const SOURCES: &'static [u16] = oer_chips_family_platform_interrupt_table::interrupt_sources![TimerIrq, TimerIrq];
/// }
/// # let _ = <Twice as oer_chips_family_platform_interrupt_table::InterruptSources>::SOURCES;
/// ```
#[macro_export]
macro_rules! interrupt_sources {
    ($($member:ty),+ $(,)?) => {{
        const LISTS: &[&[u16]] = &[$(<$member as $crate::InterruptSources>::SOURCES),+];
        const UNION: [u16; $crate::__private::total(LISTS)] = $crate::__private::union(LISTS);
        const _: () = if $crate::__private::repeats(&UNION) { panic!("two members of a part, or one member, list a source twice") };
        &UNION
    }};
}

/// The strong symbol a line's vector calls: it lends the entry's part to
/// the handler. Before the binding placed the part the line is disabled
/// and silent, so the cell always holds it; a cell found empty or lent runs
/// nothing and never panics.
#[doc(hidden)]
#[macro_export]
macro_rules! __interrupt_handler {
    ([$(#[$attribute:meta])*], $token:ident, Executor, $handler:path, [], $hart:literal, $line:literal) => {
        $crate::__private::link_fn! {
            interrupt $crate::__private::InterruptHandlerSlot<$hart, $line>, hart $hart, line $line =>
            $(#[$attribute])*
            extern "C" fn __handler() {
                #[allow(clippy::disallowed_methods, reason = "the entry's vector is its entry point's one caller")]
                $crate::__private::vector::<$token>(|part, _here| {
                    <$handler as $crate::ExecutorHandler<<$token as $crate::Entry>::Part>>::handle(part)
                });
            }
        }
    };
    ([$(#[$attribute:meta])*], $token:ident, $class:ident, $handler:path, [here], $hart:literal, $line:literal) => {
        $crate::__private::link_fn! {
            interrupt $crate::__private::InterruptHandlerSlot<$hart, $line>, hart $hart, line $line =>
            $(#[$attribute])*
            extern "C" fn __handler() {
                let handler: fn(&mut <$token as $crate::Entry>::Part, $crate::Here<'_, $token>) = $handler;
                #[allow(clippy::disallowed_methods, reason = "the entry's vector is its entry point's one caller")]
                $crate::__private::vector::<$token>(handler);
            }
        }
    };
    ([$(#[$attribute:meta])*], $token:ident, $class:ident, $handler:path, [], $hart:literal, $line:literal) => {
        $crate::__private::link_fn! {
            interrupt $crate::__private::InterruptHandlerSlot<$hart, $line>, hart $hart, line $line =>
            $(#[$attribute])*
            extern "C" fn __handler() {
                #[allow(clippy::disallowed_methods, reason = "the entry's vector is its entry point's one caller")]
                $crate::__private::vector::<$token>(|part, _here| {
                    let handler: fn(&mut <$token as $crate::Entry>::Part) = $handler;
                    handler(part)
                });
            }
        }
    };
}

/// An [`Class::Executor`] entry's token states its handler type
/// ([`ExecutorEntry`]); no other class has one.
#[doc(hidden)]
#[macro_export]
macro_rules! __executor_entry {
    (Executor, $token:ident, $handler:path) => {
        impl $crate::ExecutorEntry for $token {
            type Handler = $handler;
        }
    };
    ($class:ident, $token:ident, $handler:path) => {};
}

/// The exempt entry's reason, exported for the image gate's report: the
/// bytes of `reason` named after the entry's handler symbol.
#[doc(hidden)]
#[macro_export]
macro_rules! __exemption {
    ($hart:literal, $line:literal) => {};
    ($hart:literal, $line:literal, $reason:literal) => {
        const REASON: &str = $reason;
        $crate::__private::link_fn! {
            exemption $crate::__private::ExemptionSlot<$hart, $line, { REASON.len() }>, hart $hart, line $line =>
            #[used]
            static EXEMPTION: [u8; REASON.len()] = $crate::__private::bytes(REASON);
        }
    };
}

/// An entry's [`Class`] from its declaration.
#[doc(hidden)]
#[macro_export]
macro_rules! __class {
    (Hardware) => {
        $crate::Class::Hardware
    };
    (Executor) => {
        $crate::Class::Executor
    };
    (Exempt ExecutorLevel) => {
        $crate::Class::ExemptExecutorLevel
    };
    (Exempt WakerCall) => {
        $crate::Class::ExemptWakerCall
    };
    (LevelTask) => {
        $crate::Class::LevelTask
    };
}

#[doc(hidden)]
pub mod __private {
    use crate::{Binding, Class, Lines};

    pub use oer_portable_memory::{HandlerCell, link_fn};
    pub use oer_portable_riscv::clic;
    pub use static_cell::StaticCell;

    /// The one entry point of every entry's vector: lend the entry's part
    /// and the witness of the call ([`Here`](crate::Here)) to `handler`.
    /// Before the binding placed the part the line is disabled and silent,
    /// so the cell always holds it; a cell found empty or lent runs nothing
    /// and never panics. Root Clippy policy forbids every call but the
    /// table macro's vector (`disallowed-methods`).
    #[inline(always)]
    pub fn vector<E: crate::Entry>(handler: for<'call> fn(&mut E::Part, crate::Here<'call, E>)) {
        let _ = E::PROOF
            .cell()
            .with(|part| handler(part, crate::Here::new()));
    }

    /// A type-specific handler/cell mapping. No conversion between entries.
    pub struct EntryProof<E: crate::Entry> {
        handler: extern "C" fn(),
        cell: &'static HandlerCell<E::Part>,
        _entry: core::marker::PhantomData<fn(E) -> E>,
    }
    impl<E: crate::Entry> EntryProof<E> {
        /// Construction is restricted to the table macro by root Clippy policy.
        pub const fn new(handler: extern "C" fn(), cell: &'static HandlerCell<E::Part>) -> Self {
            Self {
                handler,
                cell,
                _entry: core::marker::PhantomData,
            }
        }
        pub fn handler(&self) -> extern "C" fn() {
            self.handler
        }
        /// The cell the entry's handler borrows the part from: only this
        /// crate reaches it, the router to place the part and the vector's
        /// entry point to lend it.
        pub(crate) fn cell(&self) -> &'static HandlerCell<E::Part> {
            self.cell
        }
    }

    /// The seal of [`crate::Table`]: the table macro and this crate alone
    /// implement it.
    pub trait SealedTable {}

    mod element {
        /// The seal of [`super::TableElement`]: unreachable outside this
        /// crate, so its two implementations below are all there are.
        pub trait Sealed {}
        impl Sealed for crate::Binding {}
        impl Sealed for crate::TaskBinding {}
    }

    /// An element of a table the image exports, with the one symbol that
    /// holds the slice of it: [`Binding`] in `__OER_INTERRUPT_TABLE`,
    /// [`TaskBinding`](crate::TaskBinding) in `__OER_LEVEL_TASK_TABLE`.
    /// Sealed: implemented here alone, once each.
    pub trait TableElement: element::Sealed + Sync + 'static {
        const SYMBOL: &'static str;
    }
    impl TableElement for Binding {
        const SYMBOL: &'static str = "__OER_INTERRUPT_TABLE";
    }
    impl TableElement for crate::TaskBinding {
        const SYMBOL: &'static str = "__OER_LEVEL_TASK_TABLE";
    }

    /// The slot of the table of `B`s an image exports.
    pub struct TableSlot<B: TableElement>(core::marker::PhantomData<fn() -> B>);
    #[allow(
        unsafe_code,
        reason = "the interrupt table owns its existing table ABIs"
    )]
    // SAFETY: each of the two sealed elements names its own symbol, and
    // that symbol holds an immutable slice of that element, whose repr(C)
    // layout is checked above; the export checks this exact type.
    unsafe impl<B: TableElement> oer_portable_memory::LinkSlot for TableSlot<B> {
        type Value = &'static [B];
        const NAME: oer_portable_memory::SlotName =
            oer_portable_memory::SlotName(&[oer_portable_memory::NamePart::Text(B::SYMBOL)]);
        const KIND: oer_portable_memory::SlotKind = oer_portable_memory::SlotKind::Static;
    }
    pub enum InterruptHandlerSlot<const H: u8, const L: u8> {}
    #[allow(
        unsafe_code,
        reason = "the interrupt table owns its existing vector ABI"
    )]
    // SAFETY: each hart/line names one safe C fn() vector handler. The table
    // macro emits it once, and the export checks both indices and signature.
    unsafe impl<const H: u8, const L: u8> oer_portable_memory::LinkSlot for InterruptHandlerSlot<H, L> {
        type Value = extern "C" fn();
        const NAME: oer_portable_memory::SlotName = oer_portable_memory::SlotName(&[
            oer_portable_memory::NamePart::Text("__oer_interrupt_h"),
            oer_portable_memory::NamePart::Decimal(H),
            oer_portable_memory::NamePart::Text("_l"),
            oer_portable_memory::NamePart::Decimal(L),
        ]);
        const KIND: oer_portable_memory::SlotKind = oer_portable_memory::SlotKind::Function;
    }
    pub enum ExemptionSlot<const H: u8, const L: u8, const N: usize> {}
    #[allow(
        unsafe_code,
        reason = "the interrupt table owns its existing exemption ABI"
    )]
    // SAFETY: each reason symbol is immutable bytes for its hart/line, of
    // exactly N bytes. Its export checks the complete name and array type.
    unsafe impl<const H: u8, const L: u8, const N: usize> oer_portable_memory::LinkSlot
        for ExemptionSlot<H, L, N>
    {
        type Value = [u8; N];
        const NAME: oer_portable_memory::SlotName = oer_portable_memory::SlotName(&[
            oer_portable_memory::NamePart::Text("__OER_INTERRUPT_EXEMPT___oer_interrupt_h"),
            oer_portable_memory::NamePart::Decimal(H),
            oer_portable_memory::NamePart::Text("_l"),
            oer_portable_memory::NamePart::Decimal(L),
        ]);
        const KIND: oer_portable_memory::SlotKind = oer_portable_memory::SlotKind::Static;
    }

    /// One entry as the table's rules and its image layout see it.
    pub struct Declared {
        /// The bound part's sources; none when a `cfg` leaves it out.
        pub sources: &'static [u16],
        /// A `cfg` keeps the entry in this build.
        pub present: bool,
        /// The entry's line, level, class, hart and handler (`None` when a
        /// `cfg` leaves it out); its source is each of `sources` in turn.
        pub binding: Binding,
    }

    /// Whether `entry` is a level task's: the task table's, not the
    /// source table's.
    const fn task(entry: &Declared) -> bool {
        matches!(entry.binding.class, Class::LevelTask)
    }

    /// The bindings `entries` lay out in the source table: one per source
    /// of a present entry, one for an entry a `cfg` leaves out, none for a
    /// level task.
    pub const fn count(entries: &[Declared]) -> usize {
        let mut count = 0usize;
        let mut rest = entries;
        while let [entry, tail @ ..] = rest {
            rest = tail;
            count = count.saturating_add(if task(entry) {
                0
            } else if entry.present {
                entry.sources.len()
            } else {
                1
            });
        }
        count
    }

    /// The level tasks of `entries`, present or left out: one element of
    /// the task table each.
    pub const fn task_count(entries: &[Declared]) -> usize {
        let mut count = 0usize;
        let mut rest = entries;
        while let [entry, tail @ ..] = rest {
            rest = tail;
            if task(entry) {
                count = count.saturating_add(1);
            }
        }
        count
    }

    /// The task table of `entries`, in their order: [`task_count`] of
    /// them, each with its entry's line, level, hart and handler (`None`
    /// for a task a `cfg` leaves out).
    pub const fn task_bindings<const N: usize>(entries: &[Declared]) -> [crate::TaskBinding; N] {
        const VACANT_TASK: crate::TaskBinding = crate::TaskBinding {
            line: 0,
            level: 0,
            hart: 0,
            handler: None,
        };
        let mut out = [VACANT_TASK; N];
        let mut slots: &mut [crate::TaskBinding] = &mut out;
        let mut rest = entries;
        while let [entry, tail @ ..] = rest {
            rest = tail;
            if !task(entry) {
                continue;
            }
            let [slot, others @ ..] = slots else {
                return out;
            };
            *slot = crate::TaskBinding {
                line: entry.binding.line,
                level: entry.binding.level,
                hart: entry.binding.hart,
                handler: entry.binding.handler,
            };
            slots = others;
        }
        out
    }

    /// No binding: the filler of a slot `bindings` writes over.
    const VACANT: Binding = Binding {
        source: 0,
        line: 0,
        level: 0,
        class: Class::Hardware,
        hart: 0,
        handler: None,
    };

    /// The bindings of `entries`, in their order, each source of an entry
    /// in its part's order; [`count`] of them.
    pub const fn bindings<const N: usize>(entries: &[Declared]) -> [Binding; N] {
        let mut out = [VACANT; N];
        let mut slots: &mut [Binding] = &mut out;
        let mut rest = entries;
        while let [entry, tail @ ..] = rest {
            rest = tail;
            if task(entry) {
                continue;
            }
            let absent: &[u16] = &[0];
            let mut sources = if entry.present { entry.sources } else { absent };
            while let [source, more @ ..] = sources {
                sources = more;
                let [slot, others @ ..] = slots else {
                    return out;
                };
                *slot = Binding {
                    source: *source,
                    line: entry.binding.line,
                    level: entry.binding.level,
                    class: entry.binding.class,
                    hart: entry.binding.hart,
                    handler: entry.binding.handler,
                };
                slots = others;
            }
        }
        out
    }

    /// The rules of a table, or the first it breaks, at compile time: declared context levels and
    /// present entries' lines and harts in range, one entry per line and hart and per source,
    /// one `Executor` per level and hart, no `Hardware` or waker exemption
    /// on its level, every level exemption on it; and the level tasks'
    /// rules ([`check_tasks`]).
    pub const fn check(
        lines: Lines,
        contexts: &[u8],
        entries: &[Declared],
    ) -> Result<(), &'static str> {
        let mut levels = contexts;
        while let [level, tail @ ..] = levels {
            levels = tail;
            if *level < 1 || *level > lines.levels {
                return Err("a context's level is no level a line can hold");
            }
        }
        if let Err(why) = check_tasks(entries) {
            return Err(why);
        }
        let mut rest = entries;
        let mut position = 0;
        while let [entry, tail @ ..] = rest {
            rest = tail;
            position += 1;
            if !entry.present {
                continue;
            }
            if entry.binding.line < lines.first || entry.binding.line >= lines.count {
                return Err("an entry's line is no line a peripheral source can be routed to");
            }
            if entry.binding.hart >= lines.harts {
                return Err("an entry's hart is no hart of the image");
            }
            let mut executors = 0;
            let mut others = entries;
            let mut other_position = 0;
            while let [other, tail @ ..] = others {
                others = tail;
                other_position += 1;
                let same = other_position == position;
                if !other.present {
                    continue;
                }
                if !same
                    && other.binding.line == entry.binding.line
                    && other.binding.hart == entry.binding.hart
                {
                    return Err("two entries share a line and hart");
                }
                if twice(entry.sources, other.sources, same) {
                    return Err("two entries, or one part, list a source twice");
                }
                if other_position > position
                    && other.binding.level == entry.binding.level
                    && other.binding.hart == entry.binding.hart
                {
                    match lines.tie_break {
                        crate::TieBreak::HighestLine => {
                            if entry.binding.line >= other.binding.line {
                                return Err(
                                    "declared low-to-high priority contradicts the chip's line tie-break",
                                );
                            }
                        }
                    }
                }
                if other.binding.level == entry.binding.level
                    && other.binding.hart == entry.binding.hart
                    && matches!(other.binding.class, Class::Executor)
                {
                    executors += 1;
                }
            }
            match entry.binding.class {
                Class::Executor => {
                    if executors != 1 {
                        return Err("two Executor entries share a level and hart");
                    }
                }
                Class::Hardware | Class::ExemptWakerCall => {
                    if executors != 0 {
                        return Err("a Hardware entry shares the level of its hart's Executor");
                    }
                }
                // An executor on a task's level: `check_tasks` refuses it.
                Class::LevelTask => {}
                Class::ExemptExecutorLevel => {
                    if executors != 1 {
                        return Err(
                            "an Exempt(ExecutorLevel) entry is not on the level of its hart's Executor",
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// The rules of the level tasks, or the first one breaks, at compile
    /// time: a present `LevelTask` entry's part lists no source and every
    /// other entry's lists at least one; one task a level and hart; and the
    /// level of a task on its hart holds no entry of another class (an
    /// executor never, and no source either), since a level between the
    /// handlers and the application runs its one task and nothing else.
    pub const fn check_tasks(entries: &[Declared]) -> Result<(), &'static str> {
        let mut rest = entries;
        let mut position = 0;
        while let [entry, tail @ ..] = rest {
            rest = tail;
            position += 1;
            if !entry.present {
                continue;
            }
            if !task(entry) {
                if entry.sources.is_empty() {
                    return Err("an entry's part lists no source: only a LevelTask's has none");
                }
                continue;
            }
            if !entry.sources.is_empty() {
                return Err(
                    "a LevelTask entry's part lists a source: hardware reaches a task through a handler above it",
                );
            }
            let mut others = entries;
            let mut other_position = 0;
            while let [other, more @ ..] = others {
                others = more;
                other_position += 1;
                if other_position == position
                    || !other.present
                    || other.binding.hart != entry.binding.hart
                    || other.binding.level != entry.binding.level
                {
                    continue;
                }
                return Err(match other.binding.class {
                    Class::LevelTask => "two LevelTask entries share a level and hart",
                    Class::Executor => {
                        "an Executor entry shares the level of its hart's level task"
                    }
                    _ => "an entry with a source shares the level of its hart's level task",
                });
            }
        }
        Ok(())
    }

    /// The sources `lists` hold together.
    pub const fn total(lists: &[&[u16]]) -> usize {
        let mut total = 0usize;
        let mut rest = lists;
        while let [list, tail @ ..] = rest {
            rest = tail;
            total = total.saturating_add(list.len());
        }
        total
    }

    /// The sources of `lists`, in order: [`total`] of them.
    pub const fn union<const N: usize>(lists: &[&[u16]]) -> [u16; N] {
        let mut out = [0; N];
        let mut slots: &mut [u16] = &mut out;
        let mut rest = lists;
        while let [list, tail @ ..] = rest {
            rest = tail;
            let mut sources = *list;
            while let [source, more @ ..] = sources {
                sources = more;
                let [slot, others @ ..] = slots else {
                    return out;
                };
                *slot = *source;
                slots = others;
            }
        }
        out
    }

    /// Whether `sources` lists a source twice.
    pub const fn repeats(sources: &[u16]) -> bool {
        twice(sources, sources, true)
    }

    /// Whether a source of `first` is one of `second`'s, the same list's
    /// other positions when `same`.
    const fn twice(first: &[u16], second: &[u16], same: bool) -> bool {
        let mut rest = first;
        while let [source, tail @ ..] = rest {
            rest = tail;
            // Within one list only the later positions: each pair once,
            // and never a source with itself.
            let mut others = if same { tail } else { second };
            while let [other, more @ ..] = others {
                others = more;
                if *other == *source {
                    return true;
                }
            }
        }
        false
    }

    /// The bytes of `text`, which holds `N`: the table's exemption macro
    /// takes `N` as the text's own length.
    pub const fn bytes<const N: usize>(text: &str) -> [u8; N] {
        let mut out = [0; N];
        let mut from = text.as_bytes();
        let mut to: &mut [u8] = &mut out;
        while let ([byte, from_tail @ ..], [slot, to_tail @ ..]) = (from, to) {
            *slot = *byte;
            from = from_tail;
            to = to_tail;
        }
        out
    }

    /// The private part of a token.
    #[derive(Debug)]
    pub struct Unique(());

    impl Unique {
        /// Only [`interrupt_table!`](crate::interrupt_table) creates a token,
        /// once per entry and image. The root Clippy configuration forbids
        /// direct calls outside that macro's once gate.
        #[doc(hidden)]
        pub const fn new() -> Self {
            Self(())
        }
    }
}

/// Four fake interrupt parts and the fake's [`Lines`] (`FAKE_LINES`: lines
/// 16 to 47, eight levels, two harts), for this crate's tests and examples.
#[doc(hidden)]
#[macro_export]
macro_rules! __fake_parts {
    () => {
        /// The timer's interrupt part, source 3; it counts the calls of
        /// its handler.
        #[derive(Default)]
        pub struct TimerIrq {
            pub handled: u32,
        }
        impl $crate::InterruptSource for TimerIrq {
            const SOURCE: u16 = 3;
        }
        /// The radio's interrupt part, source 5.
        pub struct RadioIrq;
        impl $crate::InterruptSource for RadioIrq {
            const SOURCE: u16 = 5;
        }
        /// A part of two sources one handler serves, 4 and 7; it counts
        /// the calls of its handler.
        #[derive(Default)]
        pub struct PairIrq {
            pub handled: u32,
        }
        impl $crate::InterruptSources for PairIrq {
            const SOURCES: &'static [u16] = &[4, 7];
        }
        /// A part whose handler cell a test occupies before its binding,
        /// source 2; it counts the calls of its handler.
        #[derive(Default)]
        pub struct OccupiedIrq {
            pub handled: u32,
        }
        impl $crate::InterruptSource for OccupiedIrq {
            const SOURCE: u16 = 2;
        }
        /// A part every build leaves out, source 6.
        #[allow(dead_code, reason = "no build binds the absent part")]
        pub struct AbsentIrq;
        impl $crate::InterruptSource for AbsentIrq {
            const SOURCE: u16 = 6;
        }
        /// A level task's part: no source; it counts the runs of its
        /// handler.
        #[derive(Default)]
        pub struct KeeperPart {
            pub handled: u32,
        }
        impl $crate::InterruptSources for KeeperPart {
            const SOURCES: &'static [u16] = &[];
        }
        /// The handler type of a fake executor's entry: a round that does
        /// nothing, for any part.
        pub struct OnExecutor;
        impl<P> $crate::ExecutorHandler<P> for OnExecutor {
            fn handle(_: &mut P) {}
        }
        pub const FAKE_LINES: $crate::Lines = $crate::Lines {
            first: 16,
            count: 48,
            levels: 8,
            harts: 2,
            tie_break: $crate::TieBreak::HighestLine,
        };
    };
}

/// A host model of one hart's matrix bank and CLIC (`FakeController`,
/// which records every write in order), for this crate's tests and
/// examples.
#[doc(hidden)]
#[macro_export]
macro_rules! __fake_controller {
    () => {
        /// One write of the fake controller.
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum Write {
            Route {
                source: u16,
                line: u8,
            },
            Silence {
                source: u16,
            },
            Bind {
                line: u8,
                control: $crate::__private::clic::Control,
            },
            BindTask {
                line: u8,
                control: $crate::__private::clic::Control,
            },
            Disable {
                line: u8,
            },
        }

        /// A fake line: what it was written, or a word a test puts there as
        /// a boot or a fault might leave it.
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum FakeLine {
            Disabled,
            Bound($crate::__private::clic::Control),
            /// A level task's line.
            Task($crate::__private::clic::Control),
            /// Enabled, with an attribute or control no binding writes.
            Foreign(u32),
        }

        impl FakeLine {
            /// The fake's own word for a record: a written line as the
            /// expectation it holds records, a foreign one as put there.
            fn word(self) -> u32 {
                match self {
                    Self::Disabled => 0,
                    Self::Bound(control) => $crate::LineExpectation::Bound(control).record(),
                    Self::Task(control) => $crate::LineExpectation::Task(control).record(),
                    Self::Foreign(word) => word,
                }
            }
        }

        /// Eight sources, 48 lines, three level bits; the writes it took.
        pub struct FakeController {
            pub hart: u8,
            pub routes: [Option<u8>; 8],
            pub lines: [FakeLine; 48],
            pub writes: ::std::vec::Vec<Write>,
        }

        impl FakeController {
            /// Every source routed to line 20 and every line enabled, as a
            /// boot before the table might leave them.
            pub fn new(hart: u8) -> Self {
                Self {
                    hart,
                    routes: [Some(20); 8],
                    lines: [FakeLine::Bound($crate::__private::clic::Control::level::<1, 3>()); 48],
                    writes: ::std::vec::Vec::new(),
                }
            }
        }

        impl $crate::Controller for FakeController {
            type Source = u16;
            type Line = u8;
            const SOURCES: &'static [u16] = &[0, 1, 2, 3, 4, 5, 6, 7];
            const LINES: &'static [u8] = &{
                let mut lines = [0u8; 48];
                let mut line = 0;
                while line < 48 {
                    lines[line] = line as u8;
                    line += 1;
                }
                lines
            };
            const LEVEL_BITS: $crate::__private::clic::LevelBits =
                $crate::__private::clic::LevelBits::constant::<3>();
            fn hart(&self) -> u8 {
                self.hart
            }
            fn verify_configuration(&self) -> Result<(), $crate::Error> {
                Ok(())
            }
            fn route(&mut self, source: u16, line: u8) {
                self.routes[usize::from(source)] = Some(line);
                self.writes.push(Write::Route { source, line });
            }
            fn silence(&mut self, source: u16) {
                self.routes[usize::from(source)] = None;
                self.writes.push(Write::Silence { source });
            }
            fn routed(&self, source: u16) -> Option<u8> {
                self.routes[usize::from(source)]
            }
            fn bind(&mut self, line: u8, control: $crate::__private::clic::Control) {
                self.lines[usize::from(line)] = FakeLine::Bound(control);
                self.writes.push(Write::Bind { line, control });
            }
            fn bind_task(&mut self, line: u8, control: $crate::__private::clic::Control) {
                self.lines[usize::from(line)] = FakeLine::Task(control);
                self.writes.push(Write::BindTask { line, control });
            }
            fn disable(&mut self, line: u8) {
                self.lines[usize::from(line)] = FakeLine::Disabled;
                self.writes.push(Write::Disable { line });
            }
            fn check(
                &self,
                line: u8,
                expected: $crate::LineExpectation,
            ) -> Result<(), $crate::LineObservation> {
                let held = self.lines[usize::from(line)];
                let holds = match expected {
                    $crate::LineExpectation::Disabled => held == FakeLine::Disabled,
                    $crate::LineExpectation::Bound(control) => held == FakeLine::Bound(control),
                    $crate::LineExpectation::Task(control) => held == FakeLine::Task(control),
                };
                if holds {
                    Ok(())
                } else {
                    Err($crate::LineObservation::of_word(held.word()))
                }
            }
        }
    };
}

/// A table of hart 0 with an interrupt-mode executor, the keeper's level
/// task on line 34 and a hardware handler that pends the task through its
/// witness, at the given levels; a fake handle of a line (`FakePendLine`,
/// counting its pends in `PENDS`) and the keeper's pend (`KeeperPend`, set
/// in `PEND`), for this crate's examples of [`Pend`].
#[doc(hidden)]
#[macro_export]
macro_rules! __fake_pend {
    (executor $executor:literal, keeper $keeper:literal, handler $handler:literal, line $line:literal) => {
        $crate::__fake_parts!();
        $crate::__fake_controller!();
        /// The fake handle of the keeper's line.
        pub struct FakePendLine;
        /// The pends of the fake line.
        pub static PENDS: ::core::sync::atomic::AtomicU32 = ::core::sync::atomic::AtomicU32::new(0);
        impl $crate::PendLine<0> for FakePendLine {
            const LINE: u8 = $line;
            fn pend(&self, _hart: &oer_portable_riscv::OnHart<0>) {
                PENDS.fetch_add(1, ::core::sync::atomic::Ordering::Relaxed);
            }
        }
        /// The keeper's pend: by the timer's handler and the radio
        /// executor's tasks.
        pub type KeeperPend = $crate::Pend<KeeperToken, TimerToken, RadioToken, FakePendLine, 0>;
        /// The keeper's pend, once set.
        pub static PEND: ::std::sync::OnceLock<KeeperPend> = ::std::sync::OnceLock::new();
        pub fn on_keeper(_: &mut KeeperPart) {}
        /// The timer's handler: it pends the keeper through its witness.
        pub fn on_timer(_: &mut TimerIrq, here: $crate::Here<'_, TimerToken>) {
            if let Some(pend) = PEND.get() {
                pend.from_handler(&here);
            }
        }
        $crate::interrupt_table! {
            lines: FAKE_LINES,
            handler_attributes: [];
            contexts { radio_executor: $executor; keeper: $keeper; radio: $handler; }
            radio: RadioToken = RadioIrq => OnExecutor, line 17, context radio_executor, hart 0, Executor;
            timer: TimerToken = TimerIrq => on_timer, with here, line 21, context radio, hart 0, Hardware;
            keeper: KeeperToken = KeeperPart => on_keeper, line 34, context keeper, hart 0, LevelTask;
        }
    };
}

/// The fake parts and controller and an interrupt table over the fake
/// parts; once per binary, as [`interrupt_table!`].
#[doc(hidden)]
#[macro_export]
macro_rules! __fake_table {
    () => {
        $crate::__fake_parts!();

        $crate::__fake_controller!();

        /// The handler of the timer's entry: one more call on its part.
        pub fn on_timer(part: &mut TimerIrq) {
            part.handled += 1;
        }
        /// The handler of the radio's entry.
        pub fn on_radio(_: &mut RadioIrq) {}
        /// The handler of the pair's entry: one more call on its part.
        pub fn on_pair(part: &mut PairIrq) {
            part.handled += 1;
        }
        /// The handler of the occupied entry: one more call on its part.
        pub fn on_occupied(part: &mut OccupiedIrq) {
            part.handled += 1;
        }
        /// The handler of the keeper's level task: one more run on its
        /// part.
        pub fn on_keeper(part: &mut KeeperPart) {
            part.handled += 1;
        }

        $crate::interrupt_table! {
            lines: FAKE_LINES,
            handler_attributes: [];
            contexts { app_wake: 1; radio_executor: 2; keeper: 3; }
            timer: TimerToken = TimerIrq => on_timer, line 16, context app_wake, hart 0, Hardware;
            radio: RadioToken = RadioIrq => OnExecutor, line 17, context radio_executor, hart 1, Executor;
            pair: PairToken = PairIrq => on_pair, line 18, context app_wake, hart 0, Hardware;
            /// An entry whose handler cell a test occupies before its
            /// binding.
            occupied: OccupiedToken = OccupiedIrq => on_occupied, line 19, context app_wake, hart 0, Hardware;
            /// A level task on hart 0, between the radio's executor and the
            /// handlers above it.
            keeper: KeeperToken = KeeperPart => on_keeper, line 34, context keeper, hart 0, LevelTask;
            #[cfg(any())]
            /// Left out of every build; on its own it would break the line
            /// rule, which only the build's entries obey.
            absent: AbsentToken = AbsentIrq => on_absent, line 16, context app_wake, hart 0, Hardware;
            #[cfg(any())]
            /// Left out of every build; on its own it would break the
            /// one-task-a-level rule.
            absent_task: AbsentTaskToken = KeeperPart => on_absent_task, line 35, context keeper, hart 0, LevelTask;
        }

        /// A router of hart `H` over a fresh fake controller and the table.
        pub fn fake_router<const H: u8>() -> $crate::Router<FakeController, H, InterruptTable> {
            $crate::Router::install(FakeController::new(H), InterruptTable)
                .expect("a fresh fake installs")
        }
    };
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests;
