//! Each image's static table of its peripheral interrupt sources: the handler,
//! level and core of every source, declared once with [`interrupt_table!`].
//!
//! The table says *where* a source may be routed; the source's owner decides
//! *when*. Every source starts silent ([`install`]); [`enable`] routes it to
//! the table's level on the table's core, [`disable`] silences it again. Both
//! take the entry's token, which the table's macro creates once per entry and
//! hands out through its `take`: a source outside the table has no token, and
//! no token carries another level or core. [`verify`] checks, before
//! interrupts are enabled, that the matrix still agrees with the table.
//!
//! ```compile_fail,E0277
//! # oer_interrupt_table::__fake_matrix!();
//! # let mut matrix = FakeMatrix::new(Core::Zero);
//! // Not a token of the table: no source to enable.
//! struct Elsewhere;
//! let _ = oer_interrupt_table::enable(&mut matrix, INTERRUPT_TABLE, &Elsewhere);
//! ```
//!
//! ```compile_fail,E0271
//! # oer_interrupt_table::__fake_matrix!();
//! # let mut matrix = FakeMatrix::new(Core::Zero);
//! # let tokens = Interrupts::take().unwrap();
//! // A token of another chip's table names another matrix's source.
//! struct OtherMatrix;
//! # impl oer_interrupt_table::Matrix for OtherMatrix {
//! #     type Source = u8; type Level = u8; type Core = u8;
//! #     fn current_core(&self) -> u8 { 0 }
//! #     fn route(&mut self, _: u8, _: u8) {}
//! #     fn silence(&mut self, _: u8, _: u8) {}
//! #     fn routed(&self, _: u8, _: u8) -> Option<u8> { None }
//! #     fn slot(&self, _: u8) -> usize { 0 }
//! # }
//! let _ = oer_interrupt_table::enable(&mut OtherMatrix, &[], &tokens.timer);
//! ```
//!
//! ```compile_fail,E0382
//! # oer_interrupt_table::__fake_matrix!();
//! // A token is unique: it is neither `Copy` nor `Clone`.
//! let tokens = Interrupts::take().unwrap();
//! let first = tokens.timer;
//! let second = tokens.timer;
//! ```
//!
//! The same fake matrix enables a token of its own table:
//!
//! ```
//! # oer_interrupt_table::__fake_matrix!();
//! # let mut matrix = FakeMatrix::new(Core::Zero);
//! let tokens = Interrupts::take().unwrap();
//! oer_interrupt_table::enable(&mut matrix, INTERRUPT_TABLE, &tokens.timer).unwrap();
//! ```
#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

use core::fmt::Debug;

mod adopted;
pub use adopted::{Adopted, AlreadyAdopted};

/// A chip's interrupt matrix, as an image's interrupt table drives it.
pub trait Matrix {
    /// A peripheral interrupt source.
    type Source: Copy + Eq + Debug;
    /// A level the matrix routes a source to.
    type Level: Copy + Eq + Debug;
    /// A core.
    type Core: Copy + Eq + Debug;

    /// The core that runs the caller.
    fn current_core(&self) -> Self::Core;
    /// Route `source` to `level` on the current core.
    fn route(&mut self, source: Self::Source, level: Self::Level);
    /// Route `source` nowhere on `core`.
    fn silence(&mut self, core: Self::Core, source: Self::Source);
    /// The level `source` is routed to on `core`, if any.
    fn routed(&self, core: Self::Core, source: Self::Source) -> Option<Self::Level>;
    /// The address of the handler `source`'s vector slot holds.
    fn slot(&self, source: Self::Source) -> usize;
}

/// One entry of an image's interrupt table, laid out for the tools that read
/// it from the image.
#[derive(Debug)]
#[repr(C)]
pub struct Binding<S, L, C> {
    pub source: S,
    pub level: L,
    pub core: C,
    /// The handler the vector slot of `source` holds from the link on;
    /// `None` (a zero word) for an entry a `cfg` leaves out of the image.
    pub handler: Option<unsafe extern "C" fn()>,
}

/// The bindings of a matrix.
pub type Table<M> = [Binding<<M as Matrix>::Source, <M as Matrix>::Level, <M as Matrix>::Core>];

/// The entries of `table` the image holds, with their handlers.
fn present<S, L, C>(
    table: &[Binding<S, L, C>],
) -> impl Iterator<Item = (&Binding<S, L, C>, unsafe extern "C" fn())> {
    table
        .iter()
        .filter_map(|binding| binding.handler.map(|handler| (binding, handler)))
}

/// The token of one table entry: the only way to enable or disable its source.
///
/// # Safety
///
/// Only [`interrupt_table!`] implements it: the image's table lists `SOURCE`
/// with `LEVEL` and `CORE`, and the type has one value. [`enable`] still
/// refuses a source the table does not list so.
#[allow(
    unsafe_code,
    reason = "a token is the unique capability to route its source; forging one needs unsafe"
)]
pub unsafe trait Entry {
    type Source;
    type Level;
    type Core;
    const SOURCE: Self::Source;
    const LEVEL: Self::Level;
    const CORE: Self::Core;
}

/// A matrix that disagrees with the table, or a source enabled on another
/// core than the table's.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error<S, L, C> {
    /// The source's entry routes it to `core`, the caller runs on `current`.
    WrongCore { source: S, core: C, current: C },
    /// The source's vector slot holds another handler than the table's.
    ForeignHandler { source: S },
    /// The source is routed to another level than the table's.
    WrongLevel { source: S, level: L },
    /// The table lists the source twice.
    Duplicate { source: S },
    /// The table does not list the source with the token's level and core,
    /// or does not list a source a driver requires.
    NotInTable { source: S },
    /// A source a driver requires is the current core's but not routed.
    NotRouted { source: S },
}

/// The [`Error`] of a matrix.
pub type MatrixError<M> = Error<<M as Matrix>::Source, <M as Matrix>::Level, <M as Matrix>::Core>;

/// Route the token's source to its table level on its table core.
///
/// # Errors
///
/// [`Error::WrongCore`] when the caller runs on another core, and
/// [`Error::NotInTable`] when `table` does not list the token's source with
/// its level and core; nothing changes.
pub fn enable<M, E>(matrix: &mut M, table: &Table<M>, _token: &E) -> Result<(), MatrixError<M>>
where
    M: Matrix,
    E: Entry<Source = M::Source, Level = M::Level, Core = M::Core>,
{
    route(matrix, table, E::SOURCE, E::LEVEL, E::CORE)
}

/// Silence the token's source on its table core.
pub fn disable<M, E>(matrix: &mut M, _token: &E)
where
    M: Matrix,
    E: Entry<Source = M::Source, Level = M::Level, Core = M::Core>,
{
    matrix.silence(E::CORE, E::SOURCE);
}

/// A token whose type is erased: an owner that keeps several sources, or
/// keeps them behind a concrete type, holds their routes. Only consuming a
/// token creates one, and a route is neither `Copy` nor `Clone`.
#[derive(Debug)]
#[must_use = "a dropped route can never enable or disable its source again"]
pub struct Route<S, L, C> {
    source: S,
    level: L,
    core: C,
}

impl<S: Copy, L: Copy, C: Copy> Route<S, L, C> {
    /// The route of `token`'s source.
    pub fn new<E: Entry<Source = S, Level = L, Core = C>>(token: E) -> Self {
        let _ = token;
        Self {
            source: E::SOURCE,
            level: E::LEVEL,
            core: E::CORE,
        }
    }

    /// The route's source.
    pub fn source(&self) -> S {
        self.source
    }
}

/// The [`Route`] of a matrix.
pub type MatrixRoute<M> = Route<<M as Matrix>::Source, <M as Matrix>::Level, <M as Matrix>::Core>;

/// [`enable`] by a route.
///
/// # Errors
///
/// As [`enable`].
pub fn enable_route<M: Matrix>(
    matrix: &mut M,
    table: &Table<M>,
    route: &MatrixRoute<M>,
) -> Result<(), MatrixError<M>> {
    self::route(matrix, table, route.source, route.level, route.core)
}

/// [`disable`] by a route.
pub fn disable_route<M: Matrix>(matrix: &mut M, route: &MatrixRoute<M>) {
    matrix.silence(route.core, route.source);
}

/// Route `source` to `level` on the current core, which must be `core`, when
/// the table lists it so.
fn route<M: Matrix>(
    matrix: &mut M,
    table: &Table<M>,
    source: M::Source,
    level: M::Level,
    core: M::Core,
) -> Result<(), MatrixError<M>> {
    if !present(table).any(|(binding, _)| {
        binding.source == source && binding.level == level && binding.core == core
    }) {
        return Err(Error::NotInTable { source });
    }
    let current = matrix.current_core();
    if current != core {
        return Err(Error::WrongCore {
            source,
            core,
            current,
        });
    }
    matrix.route(source, level);
    Ok(())
}

/// Silence every source of the current core's entries, then [`verify`].
///
/// # Errors
///
/// As [`verify`].
pub fn install<M: Matrix>(matrix: &mut M, table: &Table<M>) -> Result<(), MatrixError<M>> {
    let current = matrix.current_core();
    for (binding, _) in present(table).filter(|(binding, _)| binding.core == current) {
        matrix.silence(binding.core, binding.source);
    }
    verify(matrix, table)
}

/// Check the current core's entries against the matrix: each source listed
/// once, its vector slot holding its handler, and the source silent or routed
/// to its level.
///
/// # Errors
///
/// The first disagreement.
pub fn verify<M: Matrix>(matrix: &M, table: &Table<M>) -> Result<(), MatrixError<M>> {
    for (index, (binding, _)) in present(table).enumerate() {
        if present(table)
            .take(index)
            .any(|(other, _)| other.source == binding.source)
        {
            return Err(Error::Duplicate {
                source: binding.source,
            });
        }
    }
    let current = matrix.current_core();
    for (binding, handler) in present(table).filter(|(binding, _)| binding.core == current) {
        if matrix.slot(binding.source) != handler as usize {
            return Err(Error::ForeignHandler {
                source: binding.source,
            });
        }
        match matrix.routed(binding.core, binding.source) {
            Some(level) if level != binding.level => {
                return Err(Error::WrongLevel {
                    source: binding.source,
                    level,
                });
            }
            _ => {}
        }
    }
    Ok(())
}

/// Check that each `required` source, one a driver waits on, has an entry in
/// `table` and, when that entry is the current core's, is routed: a driver
/// whose interrupt nobody routes would wait forever.
///
/// # Errors
///
/// [`Error::NotInTable`] or [`Error::NotRouted`] for the first such source.
pub fn verify_required<M: Matrix>(
    matrix: &M,
    table: &Table<M>,
    required: impl IntoIterator<Item = M::Source>,
) -> Result<(), MatrixError<M>> {
    let current = matrix.current_core();
    for source in required {
        let Some((binding, _)) = present(table).find(|(binding, _)| binding.source == source)
        else {
            return Err(Error::NotInTable { source });
        };
        if binding.core == current && matrix.routed(current, source).is_none() {
            return Err(Error::NotRouted { source });
        }
    }
    Ok(())
}

/// Declare an image's interrupt table: once per image, since it defines
/// `INTERRUPT_TABLE` (exported as `__OER_INTERRUPT_TABLE` for the tools that
/// read the image) and, for each source, the strong symbol of the source's
/// name that its vector slot holds from the link on. The image hands
/// `INTERRUPT_TABLE` to its platform's runtime.
///
/// A chip's platform wraps it with its types; each entry reads
/// `field: Token = SOURCE => handler, LEVEL, CORE;` and defines the token
/// type `Token`, while `Interrupts::take` hands out every token once. An
/// entry's `#[cfg(...)]` attributes, before its doc comment, leave all of it
/// out: token, handler symbol and table entry.
#[macro_export]
macro_rules! interrupt_table {
    (
        source: $source_ty:ty = $($sources:ident)::+,
        level: $level_ty:ty = $($levels:ident)::+,
        core: $core_ty:ty = $($cores:ident)::+,
        handler_attributes: $handler_attributes:tt;
        $(
            $(#[cfg($cfg:meta)])*
            $(#[doc = $doc:literal])*
            $field:ident: $token:ident = $source:ident => $handler:path, $level:ident, $core:ident;
        )*
    ) => {
        #[allow(unused_imports)]
        use $($sources)::+ as __OerInterruptSources;
        #[allow(unused_imports)]
        use $($levels)::+ as __OerInterruptLevels;
        #[allow(unused_imports)]
        use $($cores)::+ as __OerInterruptCores;

        $(
            $(#[cfg($cfg)])*
            $(#[doc = $doc])*
            #[must_use = "a dropped token can never enable or disable its source again"]
            pub struct $token($crate::__private::Unique);

            $(#[cfg($cfg)])*
            // SAFETY: the table below lists this source with this level and
            // core, and `Interrupts::take` creates this token's one value.
            unsafe impl $crate::Entry for $token {
                type Source = $source_ty;
                type Level = $level_ty;
                type Core = $core_ty;
                const SOURCE: $source_ty = __OerInterruptSources::$source;
                const LEVEL: $level_ty = __OerInterruptLevels::$level;
                const CORE: $core_ty = __OerInterruptCores::$core;
            }

            $(#[cfg($cfg)])*
            $crate::__interrupt_handler!($handler_attributes, $source, $handler);
        )*

        /// Every source token of the image's interrupt table.
        #[allow(dead_code, reason = "an image without sources takes no token")]
        pub struct Interrupts {
            $($(#[cfg($cfg)])* pub $field: $token,)*
        }

        #[allow(dead_code, reason = "an image without sources takes no token")]
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
        pub static INTERRUPT_TABLE: &[$crate::Binding<$source_ty, $level_ty, $core_ty>] = &[
            $($crate::Binding {
                source: __OerInterruptSources::$source,
                level: __OerInterruptLevels::$level,
                core: __OerInterruptCores::$core,
                handler: {
                    #[cfg(all($($cfg),*))]
                    let handler = ::core::option::Option::Some(
                        $source as unsafe extern "C" fn(),
                    );
                    #[cfg(not(all($($cfg),*)))]
                    let handler = ::core::option::Option::None;
                    handler
                },
            },)*
        ];
    };
}

/// The strong symbol of one source, whose name its vector slot links to.
#[doc(hidden)]
#[macro_export]
macro_rules! __interrupt_handler {
    ([$(#[$attribute:meta])*], $source:ident, $handler:path) => {
        $(#[$attribute])*
        #[allow(non_snake_case)]
        #[unsafe(no_mangle)]
        extern "C" fn $source() {
            $handler()
        }
    };
}

#[doc(hidden)]
pub mod __private {
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

/// A host model of a two-core matrix with two sources and its interrupt
/// table, for this crate's tests and examples; once per binary, as
/// [`interrupt_table!`].
#[doc(hidden)]
#[macro_export]
macro_rules! __fake_matrix {
    () => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum Source {
            Timer,
            Radio,
            Absent,
        }
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum Level {
            One,
            Two,
        }
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum Core {
            Zero,
            One,
        }

        /// Routes by core and source, and the vector slots by source.
        pub struct FakeMatrix {
            pub current: Core,
            pub routes: [[Option<Level>; 3]; 2],
            pub slots: [usize; 3],
        }

        impl FakeMatrix {
            /// Every source routed to level two on both cores, every slot
            /// holding what the link put there.
            pub fn new(current: Core) -> Self {
                let mut slots = [0; 3];
                for binding in INTERRUPT_TABLE {
                    if let Some(handler) = binding.handler {
                        slots[binding.source as usize] = handler as usize;
                    }
                }
                Self {
                    current,
                    routes: [[Some(Level::Two); 3]; 2],
                    slots,
                }
            }
        }

        impl $crate::Matrix for FakeMatrix {
            type Source = Source;
            type Level = Level;
            type Core = Core;
            fn current_core(&self) -> Core {
                self.current
            }
            fn route(&mut self, source: Source, level: Level) {
                self.routes[self.current as usize][source as usize] = Some(level);
            }
            fn silence(&mut self, core: Core, source: Source) {
                self.routes[core as usize][source as usize] = None;
            }
            fn routed(&self, core: Core, source: Source) -> Option<Level> {
                self.routes[core as usize][source as usize]
            }
            fn slot(&self, source: Source) -> usize {
                self.slots[source as usize]
            }
        }

        pub fn on_timer() {}
        pub fn on_radio() {}

        $crate::interrupt_table! {
            source: Source = Source,
            level: Level = Level,
            core: Core = Core,
            handler_attributes: [];
            timer: TimerToken = Timer => on_timer, One, Zero;
            radio: RadioToken = Radio => on_radio, Two, One;
            #[cfg(any())]
            /// Left out of every build.
            absent: AbsentToken = Absent => on_absent, One, Zero;
        }
    };
}

#[cfg(test)]
#[allow(
    unsafe_code,
    reason = "the tests' fake matrix declares an interrupt table"
)]
mod tests;
