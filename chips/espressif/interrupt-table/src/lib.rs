#![cfg_attr(not(test), deny(clippy::expect_used))]
#![deny(
    clippy::unwrap_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing
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
//! # oer_espressif_interrupt_table::__fake_table!();
//! # let mut router = fake_router(0);
//! let tokens = Interrupts::take().unwrap();
//! // The timer's entry binds the timer's part, not the radio's.
//! let _ = router.bind(tokens.timer, RadioIrq);
//! ```
//!
//! ```compile_fail,E0382
//! # oer_espressif_interrupt_table::__fake_table!();
//! # let mut router = fake_router(0);
//! # let tokens = Interrupts::take().unwrap();
//! // A part moves into its one binding.
//! let part = TimerIrq::default();
//! let _ = router.bind(tokens.timer, part);
//! let _ = router.bind(tokens.timer, part);
//! ```
//!
//! ```compile_fail,E0423
//! # oer_espressif_interrupt_table::__fake_table!();
//! fn enable_timer_interrupt(_bound: &oer_espressif_interrupt_table::Bound<TimerToken>) {}
//! // Only a binding makes the proof the interrupt's enable takes.
//! enable_timer_interrupt(&oer_espressif_interrupt_table::Bound { _part: core::marker::PhantomData });
//! ```
//!
//! ```compile_fail,E0382
//! # oer_espressif_interrupt_table::__fake_table!();
//! // A token is unique: it is neither `Copy` nor `Clone`.
//! let tokens = Interrupts::take().unwrap();
//! let first = tokens.timer;
//! let second = tokens.timer;
//! ```
//!
//! The same fake binds its own part with its own token:
//!
//! ```
//! # oer_espressif_interrupt_table::__fake_table!();
//! # let mut router = fake_router(0);
//! let tokens = Interrupts::take().unwrap();
//! let bound = router.bind(tokens.timer, TimerIrq::default()).unwrap();
//! fn enable_timer_interrupt(_bound: &oer_espressif_interrupt_table::Bound<TimerToken>) {}
//! enable_timer_interrupt(&bound);
//! ```
#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

mod binding;
pub use binding::{BindError, BindFault, BindFaultCode, BindFaultKind, Quarantined};
mod router;
pub use router::{Bound, Controller, Error, EventAuthority, LineState, Router, Silent, Trigger};

pub mod signal;

pub use oer_mmio::{InterruptSource, InterruptSources};

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
    /// level and hart.
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
    assert!(offset_of!(Binding, source) == 0);
    assert!(offset_of!(Binding, line) == 2);
    assert!(offset_of!(Binding, level) == 3);
    assert!(offset_of!(Binding, class) == 4 && size_of::<Class>() == 1);
    assert!(offset_of!(Binding, hart) == 5);
    assert!(offset_of!(Binding, handler) == 8);
    assert!(size_of::<Binding>() == 8 + size_of::<usize>());
};

/// An image's table.
pub type Table = [Binding];

/// The control byte of a line at `level` (1 for the lowest) of a controller
/// with `level_bits` level bits: the level, less one, in the top bits, every
/// lower bit one (the CLIC's `clicintctl`; level `n` of three bits is
/// `0x1f | (n - 1) << 5`).
///
/// SOURCE: RISC-V CLIC specification (`clicintctl`, `mnlbits`); ESP-IDF
/// 4d59230d `riscv/include/riscv/csr_clic.h` (`NLBITS` 3 on the ESP32-S31
/// and ESP32-C5).
#[must_use]
pub const fn line_control(level: u8, level_bits: u8) -> u8 {
    let low = 8 - level_bits;
    ((level - 1) << low) | ((1u16 << low) - 1) as u8
}

/// The token of one table entry: the only way to bind its part.
///
/// # Safety
///
/// Only [`interrupt_table!`] implements it: the image's table lists each
/// of the part's sources with `LINE`, `LEVEL` and `HART`, the entry's handler
/// reaches its part through `cell` alone, and the type has one value.
#[allow(
    unsafe_code,
    reason = "a token is the unique capability to bind its entry; forging one needs unsafe"
)]
pub unsafe trait Entry {
    /// The interrupt part the entry's handler holds.
    type Part: InterruptSources + Send + 'static;
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
    /// The cell the entry's handler borrows the part from.
    #[doc(hidden)]
    fn cell() -> &'static oer_memory::HandlerCell<Self::Part>;
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
/// is a `fn(&mut Part)`, and the macro defines the token type `Token`, while
/// `Interrupts::take` hands out every token once. `CLASS` is a [`Class`]:
/// `Hardware`, `Executor` or `Exempt(VIOLATION, "reason")`, where
/// `VIOLATION` names the one hardware rule the entry breaks
/// (`ExecutorLevel` or `WakerCall`); an exempt entry's reason is exported as
/// `__OER_INTERRUPT_EXEMPT_` followed by the handler's symbol for the image
/// gate's report.
///
/// The table fails to compile when a line lies outside
/// `lines.first..lines.count`, a declared context level outside `1..=lines.levels` or a hart
/// outside `0..lines.harts`; when two entries share a line and hart, or
/// two entries, or one part, list a source twice; when two `Executor` entries share a level and hart, a `Hardware`
/// or `Exempt(WakerCall, ..)` entry sits on the level of its hart's
/// `Executor`, or an `Exempt(ExecutorLevel, ..)` entry does not. An entry's
/// `#[cfg(...)]` attributes, before its doc comment, leave all of it out:
/// token, handler symbol, reason and table entry.
///
/// ```compile_fail,E0080
/// # oer_espressif_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// oer_espressif_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio_executor: 2; }
///     radio: RadioToken = RadioIrq => on_radio, line 17, context radio_executor, hart 0, Executor;
///     // A hardware handler would wait for the executor's whole poll.
///     timer: TimerToken = TimerIrq => on_timer, line 16, context radio_executor, hart 0, Hardware;
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_espressif_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// oer_espressif_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio_executor: 2; }
///     // One executor per level and hart.
///     radio: RadioToken = RadioIrq => on_radio, line 17, context radio_executor, hart 0, Executor;
///     timer: TimerToken = TimerIrq => on_timer, line 16, context radio_executor, hart 0, Executor;
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_espressif_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// oer_espressif_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio_executor: 2; }
///     // Nothing to exempt: no executor shares the level.
///     timer: TimerToken = TimerIrq => on_timer, line 16, context radio_executor, hart 0, Exempt(ExecutorLevel, "no executor here");
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_espressif_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// oer_espressif_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio_executor: 2; }
///     radio: RadioToken = RadioIrq => on_radio, line 17, context radio_executor, hart 0, Executor;
///     // A waker exemption does not exempt the executor's level too.
///     timer: TimerToken = TimerIrq => on_timer, line 16, context radio_executor, hart 0, Exempt(WakerCall, "its driver");
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_espressif_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// oer_espressif_interrupt_table::interrupt_table! {
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
/// # oer_espressif_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_again(_: &mut TimerIrq) {}
/// oer_espressif_interrupt_table::interrupt_table! {
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
/// # oer_espressif_interrupt_table::__fake_parts!();
/// # pub fn on_pair(_: &mut PairIrq) {}
/// # pub fn on_seven(_: &mut SevenIrq) {}
/// pub struct SevenIrq;
/// impl oer_espressif_interrupt_table::InterruptSource for SevenIrq {
///     const SOURCE: u16 = 7;
/// }
/// oer_espressif_interrupt_table::interrupt_table! {
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
/// # oer_espressif_interrupt_table::__fake_parts!();
/// # pub fn on_twice(_: &mut TwiceIrq) {}
/// pub struct TwiceIrq;
/// impl oer_espressif_interrupt_table::InterruptSources for TwiceIrq {
///     const SOURCES: &'static [u16] = &[4, 4];
/// }
/// oer_espressif_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio: 3; }
///     // One part lists source 4 twice.
///     twice: TwiceToken = TwiceIrq => on_twice, line 16, context radio, hart 0, Hardware;
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_espressif_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// oer_espressif_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio: 3; }
///     // Lines below the first external one are the core's own.
///     timer: TimerToken = TimerIrq => on_timer, line 7, context radio, hart 0, Hardware;
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_espressif_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// oer_espressif_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio: 3; unused: 9; }
///     // Even an unused context must fit the facts' eight levels.
///     timer: TimerToken = TimerIrq => on_timer, line 16, context radio, hart 0, Hardware;
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_espressif_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// oer_espressif_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio: 3; }
///     // The fake has two harts.
///     timer: TimerToken = TimerIrq => on_timer, line 16, context radio, hart 2, Hardware;
/// }
/// ```
///
/// ```compile_fail,E0308
/// # oer_espressif_interrupt_table::__fake_parts!();
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// oer_espressif_interrupt_table::interrupt_table! {
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
/// # oer_espressif_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// # pub fn on_absent(_: &mut AbsentIrq) {}
/// # pub fn on_pair(_: &mut PairIrq) {}
/// oer_espressif_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     contexts { radio_executor: 2; radio: 3; }
///     radio: RadioToken = RadioIrq => on_radio, line 16, context radio_executor, hart 0, Executor;
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
            $field:ident: $token:ident = $part:ty => $handler:path,
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
            // SAFETY: the table below lists this part's source with this
            // line, level and hart; the handler below reaches the part only
            // through `cell`, and `Interrupts::take` creates this token's one
            // value.
            unsafe impl $crate::Entry for $token {
                type Part = $part;
                const LINE: u8 = $line;
                const LEVEL: u8 = contexts::$context;
                const HART: u8 = $hart;
                const CLASS: $crate::Class = $crate::__class!($class $($violation)?);
                fn storage() -> &'static $crate::__private::StaticCell<$part> {
                    static STORAGE: $crate::__private::StaticCell<$part> =
                        $crate::__private::StaticCell::new();
                    &STORAGE
                }
                fn cell() -> &'static $crate::__private::HandlerCell<$part> {
                    static CELL: $crate::__private::HandlerCell<$part> =
                        $crate::__private::HandlerCell::new();
                    &CELL
                }
            }

            $(#[cfg($cfg)])*
            const _: () = {
                $crate::__interrupt_handler!($handler_attributes, $token, $handler, $hart, $line);
                $crate::__exemption!($hart, $line $(, $reason)?);
            };
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
                        let handler = ::core::option::Option::Some({
                            // SAFETY: `__interrupt_handler!` defines this exact C ABI
                            // symbol without a memory-safety precondition on the caller.
                            unsafe extern "C" {
                                #[link_name = concat!("__oer_interrupt_h", $hart, "_l", $line)]
                                safe fn handler();
                            }
                            handler as extern "C" fn()
                        });
                        #[cfg(not(all($($cfg),*)))]
                        let handler = ::core::option::Option::None;
                        handler
                    },
                },
            },)*
        ];

        const _: () = $crate::__private::check($lines, &[$(contexts::$context_name),*], __OER_DECLARED);

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
                    // SAFETY: `TAKEN` admits this once per image.
                    $($(#[cfg($cfg)])* $field: $token(unsafe { $crate::__private::Unique::new() }),)*
                })
            }
        }

        /// The bindings of the table: one per source of each entry the
        /// build holds, one without a handler for each it leaves out.
        static __OER_BINDINGS: [$crate::Binding; $crate::__private::count(__OER_DECLARED)] =
            $crate::__private::bindings(__OER_DECLARED);

        /// The image's interrupt table. `#[used]` keeps the exported slice
        /// in the image: reads of an immutable static are folded into their
        /// uses, which would leave the tools no symbol to find it by.
        #[used]
        #[unsafe(export_name = "__OER_INTERRUPT_TABLE")]
        pub static INTERRUPT_TABLE: &[$crate::Binding] = &__OER_BINDINGS;
    };
}

/// The sources of a part made of several members, for its
/// [`InterruptSources::SOURCES`]: every member's sources, in member order,
/// as a `&'static [u16]` built at compile time. A source two members list,
/// or one member lists twice, fails to compile.
///
/// ```
/// # oer_espressif_interrupt_table::__fake_parts!();
/// /// One handler's timer and radio banks together.
/// pub struct RadioAndTimer {
///     pub timer: TimerIrq,
///     pub radio: RadioIrq,
/// }
/// impl oer_espressif_interrupt_table::InterruptSources for RadioAndTimer {
///     const SOURCES: &'static [u16] =
///         oer_espressif_interrupt_table::interrupt_sources![TimerIrq, RadioIrq, PairIrq];
/// }
/// assert_eq!(<RadioAndTimer as oer_espressif_interrupt_table::InterruptSources>::SOURCES, [3, 5, 4, 7]);
/// ```
///
/// ```compile_fail,E0080
/// # oer_espressif_interrupt_table::__fake_parts!();
/// pub struct Twice;
/// impl oer_espressif_interrupt_table::InterruptSources for Twice {
///     // The timer's source twice.
///     const SOURCES: &'static [u16] = oer_espressif_interrupt_table::interrupt_sources![TimerIrq, TimerIrq];
/// }
/// # let _ = <Twice as oer_espressif_interrupt_table::InterruptSources>::SOURCES;
/// ```
#[macro_export]
macro_rules! interrupt_sources {
    ($($member:ty),+ $(,)?) => {{
        const LISTS: &[&[u16]] = &[$(<$member as $crate::InterruptSources>::SOURCES),+];
        const UNION: [u16; $crate::__private::total(LISTS)] = $crate::__private::union(LISTS);
        const _: () = assert!(
            !$crate::__private::repeats(&UNION),
            "two members of a part, or one member, list a source twice"
        );
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
    ([$(#[$attribute:meta])*], $token:ident, $handler:path, $hart:literal, $line:literal) => {
        $(#[$attribute])*
        #[unsafe(export_name = concat!("__oer_interrupt_h", $hart, "_l", $line))]
        extern "C" fn handler() {
            let handler: fn(&mut <$token as $crate::Entry>::Part) = $handler;
            let _ = <$token as $crate::Entry>::cell().with(handler);
        }
    };
}

/// The exempt entry's reason, exported for the image gate's report: the
/// bytes of `reason` named after the entry's handler symbol.
#[doc(hidden)]
#[macro_export]
macro_rules! __exemption {
    ($hart:literal, $line:literal) => {};
    ($hart:literal, $line:literal, $reason:literal) => {
        const REASON: &str = $reason;
        #[used]
        #[unsafe(export_name = concat!("__OER_INTERRUPT_EXEMPT___oer_interrupt_h", $hart, "_l", $line))]
        static EXEMPTION: [u8; REASON.len()] = $crate::__private::bytes(REASON);
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
}

#[doc(hidden)]
pub mod __private {
    use crate::{Binding, Class, Lines};

    pub use oer_memory::HandlerCell;
    pub use static_cell::StaticCell;

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

    /// The bindings `entries` lay out: one per source of a present entry,
    /// one for an entry a `cfg` leaves out.
    pub const fn count(entries: &[Declared]) -> usize {
        let mut count = 0usize;
        let mut rest = entries;
        while let [entry, tail @ ..] = rest {
            rest = tail;
            count = count.saturating_add(if entry.present {
                entry.sources.len()
            } else {
                1
            });
        }
        count
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

    /// The rules of a table, at compile time: declared context levels and
    /// present entries' lines and harts in range, one entry per line and hart and per source,
    /// one `Executor` per level and hart, no `Hardware` or waker exemption
    /// on its level, every level exemption on it.
    pub const fn check(lines: Lines, contexts: &[u8], entries: &[Declared]) {
        let mut levels = contexts;
        while let [level, tail @ ..] = levels {
            levels = tail;
            assert!(
                *level >= 1 && *level <= lines.levels,
                "a context's level is no level a line can hold"
            );
        }
        let mut rest = entries;
        let mut position = 0;
        while let [entry, tail @ ..] = rest {
            rest = tail;
            position += 1;
            if !entry.present {
                continue;
            }
            assert!(
                entry.binding.line >= lines.first && entry.binding.line < lines.count,
                "an entry's line is no line a peripheral source can be routed to"
            );
            assert!(
                entry.binding.hart < lines.harts,
                "an entry's hart is no hart of the image"
            );
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
                if !same {
                    assert!(
                        !(other.binding.line == entry.binding.line
                            && other.binding.hart == entry.binding.hart),
                        "two entries share a line and hart"
                    );
                }
                assert!(
                    !twice(entry.sources, other.sources, same),
                    "two entries, or one part, list a source twice"
                );
                if other_position > position
                    && other.binding.level == entry.binding.level
                    && other.binding.hart == entry.binding.hart
                {
                    match lines.tie_break {
                        crate::TieBreak::HighestLine => assert!(
                            entry.binding.line < other.binding.line,
                            "declared low-to-high priority contradicts the chip's line tie-break"
                        ),
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
                Class::Executor => assert!(
                    executors == 1,
                    "two Executor entries share a level and hart"
                ),
                Class::Hardware | Class::ExemptWakerCall => assert!(
                    executors == 0,
                    "a Hardware entry shares the level of its hart's Executor"
                ),
                Class::ExemptExecutorLevel => assert!(
                    executors == 1,
                    "an Exempt(ExecutorLevel) entry is not on the level of its hart's Executor"
                ),
            }
        }
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

    /// The bytes of `text`, which holds `N`.
    pub const fn bytes<const N: usize>(text: &str) -> [u8; N] {
        let mut out = [0; N];
        let mut from = text.as_bytes();
        let mut to: &mut [u8] = &mut out;
        while let ([byte, from_tail @ ..], [slot, to_tail @ ..]) = (from, to) {
            *slot = *byte;
            from = from_tail;
            to = to_tail;
        }
        assert!(from.is_empty(), "the reason holds its length");
        out
    }

    /// The private part of a token.
    #[derive(Debug)]
    pub struct Unique(());

    impl Unique {
        /// # Safety
        ///
        /// Only [`interrupt_table!`](crate::interrupt_table) creates a token,
        /// once per entry and image.
        #[doc(hidden)]
        #[allow(unsafe_code, reason = "only the table's macro creates a token")]
        pub const unsafe fn new() -> Self {
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
        /// A part every build leaves out, source 6.
        #[allow(dead_code, reason = "no build binds the absent part")]
        pub struct AbsentIrq;
        impl $crate::InterruptSource for AbsentIrq {
            const SOURCE: u16 = 6;
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
            Route { source: u16, line: u8 },
            Silence { source: u16 },
            Line { line: u8, state: $crate::LineState },
        }

        /// Eight sources, 48 lines, three level bits; the writes it took.
        pub struct FakeController {
            pub hart: u8,
            pub routes: [Option<u8>; 8],
            pub lines: [$crate::LineState; 48],
            pub writes: ::std::vec::Vec<Write>,
        }

        impl FakeController {
            /// Every source routed to line 20 and every line enabled, as a
            /// boot before the table might leave them.
            pub fn new(hart: u8) -> Self {
                Self {
                    hart,
                    routes: [Some(20); 8],
                    lines: [$crate::LineState::bound(1, 3); 48],
                    writes: ::std::vec::Vec::new(),
                }
            }
        }

        impl $crate::Controller for FakeController {
            const SOURCES: u16 = 8;
            const LINES: u8 = 48;
            const LEVEL_BITS: u8 = 3;
            fn hart(&self) -> u8 {
                self.hart
            }
            fn verify_configuration(&self) -> Result<(), $crate::Error> {
                Ok(())
            }
            fn route(&mut self, source: u16, line: u8) -> Result<(), $crate::Error> {
                if line >= Self::LINES {
                    return Err($crate::Error::NoSuchLine { line });
                }
                *self
                    .routes
                    .get_mut(usize::from(source))
                    .ok_or($crate::Error::NoSuchSource { source })? = Some(line);
                self.writes.push(Write::Route { source, line });
                Ok(())
            }
            fn silence(&mut self, source: u16) -> Result<(), $crate::Error> {
                *self
                    .routes
                    .get_mut(usize::from(source))
                    .ok_or($crate::Error::NoSuchSource { source })? = None;
                self.writes.push(Write::Silence { source });
                Ok(())
            }
            fn routed(&self, source: u16) -> Option<u8> {
                self.routes[usize::from(source)]
            }
            fn set_line(
                &mut self,
                line: u8,
                state: $crate::LineState,
            ) -> Result<(), $crate::Error> {
                *self
                    .lines
                    .get_mut(usize::from(line))
                    .ok_or($crate::Error::NoSuchLine { line })? = state;
                self.writes.push(Write::Line { line, state });
                Ok(())
            }
            fn line(&self, line: u8) -> $crate::LineState {
                self.lines[usize::from(line)]
            }
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

        $crate::interrupt_table! {
            lines: FAKE_LINES,
            handler_attributes: [];
            contexts { app_wake: 1; radio_executor: 2; }
            timer: TimerToken = TimerIrq => on_timer, line 16, context app_wake, hart 0, Hardware;
            radio: RadioToken = RadioIrq => on_radio, line 17, context radio_executor, hart 1, Executor;
            pair: PairToken = PairIrq => on_pair, line 18, context app_wake, hart 0, Hardware;
            #[cfg(any())]
            /// Left out of every build; on its own it would break the line
            /// rule, which only the build's entries obey.
            absent: AbsentToken = AbsentIrq => on_absent, line 16, context app_wake, hart 0, Hardware;
        }

        /// A router of `hart` over a fresh fake controller and the table.
        pub fn fake_router(hart: u8) -> $crate::Router<FakeController> {
            $crate::Router::install(FakeController::new(hart), INTERRUPT_TABLE)
                .expect("a fresh fake installs")
        }
    };
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
#[allow(
    unsafe_code,
    reason = "the tests' fake controller declares an interrupt table"
)]
mod tests;
