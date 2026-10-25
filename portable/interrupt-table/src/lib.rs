//! Each image's static table of its interrupt bindings: for every bound
//! peripheral interrupt part its source, CPU line, level, hart, handler and
//! handler class, declared once with [`interrupt_table!`].
//!
//! One line, one source, one direct call: each entry names the CPU line its
//! source is routed to, and that line's vector calls the entry's handler and
//! nothing else. The source comes from the bound part's type
//! ([`InterruptSource`]), never from the entry's text. Every source starts
//! silent ([`Router::install`]); [`Router::bind`] consumes the entry's token
//! and the part, moves the part where only the handler reaches it, writes
//! the line and the route, and returns the [`Bound`] proof a driver asks for
//! before it enables the peripheral's interrupt.
//!
//! ```compile_fail,E0308
//! # oer_interrupt_table::__fake_table!();
//! # let mut router = fake_router(0);
//! let tokens = Interrupts::take().unwrap();
//! // The timer's entry binds the timer's part, not the radio's.
//! let _ = router.bind(tokens.timer, RadioIrq);
//! ```
//!
//! ```compile_fail,E0382
//! # oer_interrupt_table::__fake_table!();
//! # let mut router = fake_router(0);
//! # let tokens = Interrupts::take().unwrap();
//! // A part moves into its one binding.
//! let part = TimerIrq::default();
//! let _ = router.bind(tokens.timer, part);
//! let _ = router.bind(tokens.timer, part);
//! ```
//!
//! ```compile_fail,E0423
//! # oer_interrupt_table::__fake_table!();
//! fn enable_timer_interrupt(_bound: &oer_interrupt_table::Bound<TimerIrq>) {}
//! // Only a binding makes the proof the interrupt's enable takes.
//! enable_timer_interrupt(&oer_interrupt_table::Bound { _part: core::marker::PhantomData });
//! ```
//!
//! ```compile_fail,E0382
//! # oer_interrupt_table::__fake_table!();
//! // A token is unique: it is neither `Copy` nor `Clone`.
//! let tokens = Interrupts::take().unwrap();
//! let first = tokens.timer;
//! let second = tokens.timer;
//! ```
//!
//! The same fake binds its own part with its own token:
//!
//! ```
//! # oer_interrupt_table::__fake_table!();
//! # let mut router = fake_router(0);
//! let tokens = Interrupts::take().unwrap();
//! let bound = router.bind(tokens.timer, TimerIrq::default()).unwrap();
//! fn enable_timer_interrupt(_bound: &oer_interrupt_table::Bound<TimerIrq>) {}
//! enable_timer_interrupt(&bound);
//! ```
#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

mod router;
pub use router::{
    Bound, Controller, Error, EventAuthority, LineState, Refused, Router, Silent, Trigger,
};

pub mod signal;

pub use oer_mmio::InterruptSource;

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
    pub handler: Option<unsafe extern "C" fn()>,
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
/// Only [`interrupt_table!`] implements it: the image's table lists the
/// part's source with `LINE`, `LEVEL` and `HART`, the entry's handler
/// reaches its part through `cell` alone, and the type has one value.
#[allow(
    unsafe_code,
    reason = "a token is the unique capability to bind its entry; forging one needs unsafe"
)]
pub unsafe trait Entry {
    /// The interrupt part the entry's handler holds.
    type Part: InterruptSource + Send + 'static;
    const LINE: u8;
    const LEVEL: u8;
    const HART: u8;
    /// The place the bound part lives in.
    #[doc(hidden)]
    fn storage() -> &'static static_cell::StaticCell<Self::Part>;
    /// The cell the entry's handler borrows the part from.
    #[doc(hidden)]
    fn cell() -> &'static oer_memory::HandlerCell<Self::Part>;
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
}

/// Declare an image's interrupt table: once per image, since it defines
/// `INTERRUPT_TABLE` (exported as `__OER_INTERRUPT_TABLE` for the tools that
/// read the image) and, for each entry, the strong symbol
/// `__oer_interrupt_h<HART>_l<LINE>` that the line's vector calls. The image
/// hands `INTERRUPT_TABLE` to its platform's runtime.
///
/// A chip's platform wraps it with its [`Lines`]; each entry reads
/// `field: Token = Part => handler, line LINE, level LEVEL, hart HART, CLASS;`
/// with literal numbers. `Part` implements [`InterruptSource`], `handler`
/// is a `fn(&mut Part)`, and the macro defines the token type `Token`, while
/// `Interrupts::take` hands out every token once. `CLASS` is a [`Class`]:
/// `Hardware`, `Executor` or `Exempt(VIOLATION, "reason")`, where
/// `VIOLATION` names the one hardware rule the entry breaks
/// (`ExecutorLevel` or `WakerCall`); an exempt entry's reason is exported as
/// `__OER_INTERRUPT_EXEMPT_` followed by the handler's symbol for the image
/// gate's report.
///
/// The table fails to compile when a line lies outside
/// `lines.first..lines.count`, a level outside `1..=lines.levels` or a hart
/// outside `0..lines.harts`; when two entries share a line and hart or a
/// source; when two `Executor` entries share a level and hart, a `Hardware`
/// or `Exempt(WakerCall, ..)` entry sits on the level of its hart's
/// `Executor`, or an `Exempt(ExecutorLevel, ..)` entry does not. An entry's
/// `#[cfg(...)]` attributes, before its doc comment, leave all of it out:
/// token, handler symbol, reason and table entry.
///
/// ```compile_fail,E0080
/// # oer_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// oer_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     radio: RadioToken = RadioIrq => on_radio, line 17, level 2, hart 0, Executor;
///     // A hardware handler would wait for the executor's whole poll.
///     timer: TimerToken = TimerIrq => on_timer, line 16, level 2, hart 0, Hardware;
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// oer_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     // One executor per level and hart.
///     radio: RadioToken = RadioIrq => on_radio, line 17, level 2, hart 0, Executor;
///     timer: TimerToken = TimerIrq => on_timer, line 16, level 2, hart 0, Executor;
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// oer_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     // Nothing to exempt: no executor shares the level.
///     timer: TimerToken = TimerIrq => on_timer, line 16, level 2, hart 0, Exempt(ExecutorLevel, "no executor here");
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// oer_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     radio: RadioToken = RadioIrq => on_radio, line 17, level 2, hart 0, Executor;
///     // A waker exemption does not exempt the executor's level too.
///     timer: TimerToken = TimerIrq => on_timer, line 16, level 2, hart 0, Exempt(WakerCall, "its driver");
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// oer_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     // One line, one source: a line's vector calls one handler.
///     radio: RadioToken = RadioIrq => on_radio, line 16, level 3, hart 0, Hardware;
///     timer: TimerToken = TimerIrq => on_timer, line 16, level 3, hart 0, Hardware;
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_again(_: &mut TimerIrq) {}
/// oer_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     // One source, one binding: the same part type binds once.
///     timer: TimerToken = TimerIrq => on_timer, line 16, level 3, hart 0, Hardware;
///     again: AgainToken = TimerIrq => on_again, line 17, level 3, hart 0, Hardware;
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// oer_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     // Lines below the first external one are the core's own.
///     timer: TimerToken = TimerIrq => on_timer, line 7, level 3, hart 0, Hardware;
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// oer_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     // Three level bits hold eight levels.
///     timer: TimerToken = TimerIrq => on_timer, line 16, level 9, hart 0, Hardware;
/// }
/// ```
///
/// ```compile_fail,E0080
/// # oer_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// oer_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     // The fake has two harts.
///     timer: TimerToken = TimerIrq => on_timer, line 16, level 3, hart 2, Hardware;
/// }
/// ```
///
/// ```compile_fail,E0308
/// # oer_interrupt_table::__fake_parts!();
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// oer_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     // A handler gets exactly its entry's part.
///     timer: TimerToken = TimerIrq => on_radio, line 16, level 3, hart 0, Hardware;
/// }
/// ```
///
/// The same entries on other harts, lines and levels, or an exemption
/// beside the executor, compile:
///
/// ```
/// # oer_interrupt_table::__fake_parts!();
/// # pub fn on_timer(_: &mut TimerIrq) {}
/// # pub fn on_radio(_: &mut RadioIrq) {}
/// # pub fn on_absent(_: &mut AbsentIrq) {}
/// oer_interrupt_table::interrupt_table! {
///     lines: FAKE_LINES,
///     handler_attributes: [];
///     radio: RadioToken = RadioIrq => on_radio, line 16, level 2, hart 0, Executor;
///     timer: TimerToken = TimerIrq => on_timer, line 16, level 2, hart 1, Hardware;
///     absent: AbsentToken = AbsentIrq => on_absent, line 47, level 2, hart 0, Exempt(ExecutorLevel, "until its port");
/// }
/// ```
#[macro_export]
macro_rules! interrupt_table {
    (
        lines: $lines:expr,
        handler_attributes: $handler_attributes:tt;
        $(
            $(#[cfg($cfg:meta)])*
            $(#[doc = $doc:literal])*
            $field:ident: $token:ident = $part:ty => $handler:path,
                line $line:literal, level $level:literal, hart $hart:literal,
                $class:ident $(($violation:ident, $reason:literal))?;
        )*
    ) => {
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
                const LEVEL: u8 = $level;
                const HART: u8 = $hart;
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

        const _: () = $crate::__private::check(
            $lines,
            &[
                $($crate::__private::Declared {
                    source: {
                        #[cfg(all($($cfg),*))]
                        let source = <$part as $crate::InterruptSource>::SOURCE;
                        #[cfg(not(all($($cfg),*)))]
                        let source = 0;
                        source
                    },
                    line: $line,
                    level: $level,
                    hart: $hart,
                    class: $crate::__class!($class $($violation)?),
                    present: cfg!(all($($cfg),*)),
                },)*
            ],
        );

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

        /// The image's interrupt table. `#[used]` keeps the exported slice
        /// in the image: reads of an immutable static are folded into their
        /// uses, which would leave the tools no symbol to find it by.
        #[used]
        #[unsafe(export_name = "__OER_INTERRUPT_TABLE")]
        pub static INTERRUPT_TABLE: &[$crate::Binding] = &[
            $({
                #[cfg(all($($cfg),*))]
                let binding = $crate::Binding {
                    source: <$part as $crate::InterruptSource>::SOURCE,
                    line: $line,
                    level: $level,
                    class: $crate::__class!($class $($violation)?),
                    hart: $hart,
                    handler: ::core::option::Option::Some({
                        unsafe extern "C" {
                            #[link_name = concat!("__oer_interrupt_h", $hart, "_l", $line)]
                            fn handler();
                        }
                        handler as unsafe extern "C" fn()
                    }),
                };
                #[cfg(not(all($($cfg),*)))]
                let binding = $crate::Binding {
                    source: 0,
                    line: $line,
                    level: $level,
                    class: $crate::__class!($class $($violation)?),
                    hart: $hart,
                    handler: ::core::option::Option::None,
                };
                binding
            },)*
        ];
    };
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
    use crate::{Class, Lines};

    pub use oer_memory::HandlerCell;
    pub use static_cell::StaticCell;

    /// One entry as the table's rules see it.
    pub struct Declared {
        pub source: u16,
        pub line: u8,
        pub level: u8,
        pub hart: u8,
        pub class: Class,
        /// A `cfg` keeps the entry in this build.
        pub present: bool,
    }

    /// The rules of a table, at compile time: each present entry's line,
    /// level and hart in range, one entry per line and hart and per source,
    /// one `Executor` per level and hart, no `Hardware` or waker exemption
    /// on its level, every level exemption on it.
    pub const fn check(lines: Lines, entries: &[Declared]) {
        let mut rest = entries;
        let mut position = 0;
        while let [entry, tail @ ..] = rest {
            rest = tail;
            position += 1;
            if !entry.present {
                continue;
            }
            assert!(
                entry.line >= lines.first && entry.line < lines.count,
                "an entry's line is no line a peripheral source can be routed to"
            );
            assert!(
                entry.level >= 1 && entry.level <= lines.levels,
                "an entry's level is no level a line can hold"
            );
            assert!(
                entry.hart < lines.harts,
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
                        !(other.line == entry.line && other.hart == entry.hart),
                        "two entries share a line and hart"
                    );
                    assert!(other.source != entry.source, "two entries bind one source");
                }
                if other.level == entry.level
                    && other.hart == entry.hart
                    && matches!(other.class, Class::Executor)
                {
                    executors += 1;
                }
            }
            match entry.class {
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

/// Three fake interrupt parts and the fake's [`Lines`] (`FAKE_LINES`: lines
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

        $crate::interrupt_table! {
            lines: FAKE_LINES,
            handler_attributes: [];
            timer: TimerToken = TimerIrq => on_timer, line 16, level 1, hart 0, Hardware;
            radio: RadioToken = RadioIrq => on_radio, line 17, level 2, hart 1, Executor;
            #[cfg(any())]
            /// Left out of every build; on its own it would break the line
            /// rule, which only the build's entries obey.
            absent: AbsentToken = AbsentIrq => on_absent, line 16, level 1, hart 0, Hardware;
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
