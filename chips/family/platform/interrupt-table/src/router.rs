//! The one writer of a hart's routes and CPU lines: [`Router`] over the
//! chip's [`Controller`].
//!
//! [`Router::install`] silences every source and disables every line of the
//! hart; [`Router::bind`] places an entry's interrupt part where only its
//! handler reaches it, configures and enables the entry's line, and only
//! then routes the source to it; [`Router::verify`] checks the hart's
//! routes and lines against the table before interrupts are enabled;
//! [`Router::silent`] proves a source no entry binds is routed nowhere, for
//! a driver that polls its events.
//!
//! Each hart has its own router over its own controller (its matrix bank
//! and its own CLIC), which the hart's handoff makes; `install` takes the
//! controller by value, so there is one router per hart. A source no entry
//! binds is routed on no bank: every bank is silenced before its hart first
//! unmasks (the secondary launch disconnects a released hart's bank, and
//! every router silences its own bank at `install`), and each router routes
//! only its own hart's entries of the one table.

use oer_portable_mmio::{InterruptSource, InterruptSources};

use crate::{__private::Declared, BindFault, BindFaultKind, Binding, Entry, Quarantined, Table};

use oer_portable_riscv::clic::{Control, LevelBits};

/// A hart's interrupt matrix bank and CLIC, as the table writes them. The
/// chip implements it on its typed blocks; each method is a few word reads
/// or writes, never a wait. A write of a source or line outside the
/// controller writes nothing and returns [`Error::NoSuchSource`] or
/// [`Error::NoSuchLine`].
pub trait Controller {
    /// The sources of the chip's interrupt matrix: `0..SOURCES`.
    const SOURCES: u16;
    /// The CPU lines of the hart's CLIC: `0..LINES`, at most 64.
    const LINES: u8;
    /// The level bits of a line's control byte (`CLIC_INT_CONFIG.MNLBITS`).
    const LEVEL_BITS: LevelBits;

    /// The hart whose bank and CLIC this controller writes.
    fn hart(&self) -> u8;
    /// Check the controller's implemented geometry and selected configuration
    /// against its chip facts, before installation and before enabling IRQs.
    fn verify_configuration(&self) -> Result<(), Error>;
    /// Route `source` to `line` on this hart.
    ///
    /// # Errors
    ///
    /// [`Error::NoSuchSource`] or [`Error::NoSuchLine`]; nothing is written.
    fn route(&mut self, source: u16, line: u8) -> Result<(), Error>;
    /// Route `source` nowhere on this hart.
    ///
    /// # Errors
    ///
    /// [`Error::NoSuchSource`]; nothing is written.
    fn silence(&mut self, source: u16) -> Result<(), Error>;
    /// The line `source` is routed to on this hart; `None` when silent.
    fn routed(&self, source: u16) -> Option<u8>;
    /// Bind `line` at the level of `control`: enabled, hardware-vectored,
    /// with the trigger the chip's lines have, keeping its pending bit. The
    /// line is never enabled with another attribute or control.
    ///
    /// # Errors
    ///
    /// [`Error::NoSuchLine`]; nothing is written.
    fn bind(&mut self, line: u8, control: Control) -> Result<(), Error>;
    /// Disable `line`, keeping its pending bit.
    ///
    /// # Errors
    ///
    /// [`Error::NoSuchLine`]; nothing is written.
    fn disable(&mut self, line: u8) -> Result<(), Error>;
    /// Check `line` against what the table left in it: a disabled line is
    /// not enabled, a bound one holds exactly its binding.
    ///
    /// # Errors
    ///
    /// [`Error::WrongLine`] with the line's observation, or
    /// [`Error::NoSuchLine`].
    fn check(&self, line: u8, expected: LineExpectation) -> Result<(), Error>;
}

/// What the table leaves in a line: the family's own value, which names no
/// trigger, enable or vectoring of a chip.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineExpectation {
    /// The line is not enabled.
    Disabled,
    /// The line is bound at this control byte's level.
    Bound(Control),
}

impl LineExpectation {
    /// The expectation's word in a record, encoded here alone: 0 disabled,
    /// bit 8 and the control byte bound.
    pub const fn record(self) -> u32 {
        match self {
            Self::Disabled => 0,
            Self::Bound(control) => 0x100 | control.bits() as u32,
        }
    }
}

/// A line as its chip reads it: the line's own register word, which the
/// family carries into a record and does not look into.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LineObservation(u32);

impl LineObservation {
    /// The observation of a line whose register reads `word`.
    pub const fn of_word(word: u32) -> Self {
        Self(word)
    }

    /// The word, for a record.
    pub const fn record(self) -> u32 {
        self.0
    }
}

/// How a hart's routes and lines disagree with the table, or a write its
/// controller refuses: a typed boot failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// Implemented CLICINTCTL bits differ from the chip facts.
    WrongControlBits { actual: u8, expected: u8 },
    /// Implemented interrupt input count differs from the chip facts.
    WrongInterruptCount { actual: u16, expected: u16 },
    /// Selected MNLBITS differ from the chip facts.
    WrongLevelBits { actual: u8, expected: u8 },
    /// A router of `hart` was installed over the controller of `current`.
    WrongHart { hart: u8, current: u8 },
    /// `source` is routed to `line`, which no entry of this hart binds it to.
    ForeignRoute { source: u16, line: u8 },
    /// `line` is observed as `observed`, not what the table's entries leave
    /// in it.
    WrongLine {
        line: u8,
        observed: LineObservation,
        expected: LineExpectation,
    },
    /// `source` is no source of the controller's matrix (`0..SOURCES`).
    NoSuchSource { source: u16 },
    /// `line` is no line of the controller's CLIC (`0..LINES`).
    NoSuchLine { line: u8 },
    /// `level` is no level of the controller's lines
    /// (`1..=1 << LEVEL_BITS`).
    NoSuchLevel { level: u8 },
}

/// The proof that an entry's handler holds its interrupt part and its line
/// is enabled and routed: the only way to enable the peripheral's interrupt.
/// Neither `Copy` nor `Clone`; only [`Router::bind`] makes one.
#[must_use = "the bound part's interrupt is enabled only with this proof"]
pub struct Bound<E: Entry> {
    _part: core::marker::PhantomData<fn() -> E>,
}

impl<E: Entry> core::fmt::Debug for Bound<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Bound")
    }
}

impl<E: Entry> Bound<E> {
    pub const LEVEL: u8 = E::LEVEL;
    pub const HART: u8 = E::HART;
    pub const CONTEXT: crate::Context = E::CONTEXT;
}

impl<E: Entry> Bound<E>
where
    E::Part: InterruptSource,
{
    /// The bound source.
    pub const SOURCE: u16 = <E::Part as InterruptSource>::SOURCE;
}

/// The proof that every source of `P` is routed nowhere on the router's
/// hart and that no entry of the table binds one, so the router never routes it:
/// a driver that polls `P`'s events may enable them without a handler.
/// Neither `Copy` nor `Clone`; only [`Router::silent`] makes one.
///
/// On a chip of several harts each hart's router silences its own bank at
/// its install, before that hart first unmasks, and routes only entries of
/// the table; the proof checks the table's entries of every hart and this
/// router's own bank, so the table binds the source on no hart and no router
/// ever routes it.
#[must_use = "the polled part's events are enabled only with this proof"]
pub struct Silent<P: InterruptSources> {
    _part: core::marker::PhantomData<fn() -> P>,
}

impl<P: InterruptSources> core::fmt::Debug for Silent<P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Silent")
    }
}

/// What a driver takes before it enables the interrupt events of a part of
/// sources `P::SOURCES`: [`Bound<E>`] (a handler holds the part and its line
/// is routed) or [`Silent<P>`] (no line receives the source). Sealed: only
/// the router makes either.
pub trait EventAuthority<P: InterruptSources>: sealed::Sealed {}

impl<E: Entry> EventAuthority<E::Part> for Bound<E> {}
impl<P: InterruptSources> EventAuthority<P> for Silent<P> {}

mod sealed {
    pub trait Sealed {}
    impl<E: super::Entry> Sealed for super::Bound<E> {}
    impl<P: super::InterruptSources> Sealed for super::Silent<P> {}
}

/// The one writer of hart `H`'s routes and lines, by the image's table `T`.
/// A binding of an entry of another table or another hart fails to compile.
/// It keeps the controller and its table; an entry's storage and handler
/// cell are the entry's own.
pub struct Router<C: Controller, const H: u8, T: Table> {
    pub(crate) controller: C,
    table: T,
}

/// Whether `sources` holds a source, and each lies inside a matrix of
/// `count` sources: a part's sources at compile time.
const fn inside_matrix(sources: &[u16], count: u16) -> bool {
    if sources.is_empty() {
        return false;
    }
    let mut rest = sources;
    while let [source, tail @ ..] = rest {
        rest = tail;
        if *source >= count {
            return false;
        }
    }
    true
}

/// Whether every entry `table` declares that the build holds lies inside a
/// controller of `sources`, `lines` and `levels`: the table at compile time.
const fn table_inside(table: &[Declared], sources: u16, lines: u8, levels: u16) -> bool {
    let mut rest = table;
    while let [entry, tail @ ..] = rest {
        rest = tail;
        let binding = &entry.binding;
        if entry.present
            && (!inside_matrix(entry.sources, sources)
                || binding.line >= lines
                || binding.level == 0
                || binding.level as u16 > levels)
        {
            return false;
        }
    }
    true
}

/// Whether an entry `table` declares that the build holds binds one of
/// `sources`, on any hart: the table at compile time.
const fn table_binds(table: &[Declared], sources: &[u16]) -> bool {
    let mut rest = table;
    while let [entry, tail @ ..] = rest {
        rest = tail;
        if !entry.present {
            continue;
        }
        let mut wanted = sources;
        while let [source, more @ ..] = wanted {
            wanted = more;
            let mut held = entry.sources;
            while let [bound, others @ ..] = held {
                held = others;
                if *bound == *source {
                    return true;
                }
            }
        }
    }
    false
}

/// The entries of `table` the build holds, on `hart`.
fn present(table: &[Binding], hart: u8) -> impl Iterator<Item = &Binding> {
    table
        .iter()
        .filter(move |binding| binding.handler.is_some() && binding.hart == hart)
}

impl<C: Controller, const H: u8, T: Table> Router<C, H, T> {
    /// Take `controller`'s hart for `table`, the image's `InterruptTable`:
    /// silence every source and disable every line of the hart, then
    /// [`verify`](Self::verify). Interrupts must stay disabled on the hart
    /// until it returns.
    ///
    /// A table whose entry names a source, a line or a level the controller
    /// has not fails to compile, on any hart:
    ///
    /// ```compile_fail,E0080
    /// # oer_chips_family_platform_interrupt_table::__fake_parts!();
    /// # oer_chips_family_platform_interrupt_table::__fake_controller!();
    /// # pub fn on_timer(_: &mut TimerIrq) {}
    /// // The table's lines reach past the controller's 48.
    /// const WIDE: oer_chips_family_platform_interrupt_table::Lines =
    ///     oer_chips_family_platform_interrupt_table::Lines { first: 16, count: 64, levels: 8, harts: 1, tie_break: oer_chips_family_platform_interrupt_table::TieBreak::HighestLine };
    /// oer_chips_family_platform_interrupt_table::interrupt_table! {
    ///     lines: WIDE,
    ///     handler_attributes: [];
    ///     contexts { radio: 3; }
    ///     timer: TimerToken = TimerIrq => on_timer, line 60, context radio, hart 0, Hardware;
    /// }
    /// let _ = oer_chips_family_platform_interrupt_table::Router::<_, 0, _>::install(FakeController::new(0), InterruptTable);
    /// ```
    ///
    /// ```compile_fail,E0080
    /// # oer_chips_family_platform_interrupt_table::__fake_parts!();
    /// # oer_chips_family_platform_interrupt_table::__fake_controller!();
    /// /// A source past the fake controller's eight.
    /// pub struct FarIrq;
    /// impl oer_chips_family_platform_interrupt_table::InterruptSource for FarIrq {
    ///     const SOURCE: u16 = 84;
    /// }
    /// # pub fn on_far(_: &mut FarIrq) {}
    /// oer_chips_family_platform_interrupt_table::interrupt_table! {
    ///     lines: FAKE_LINES,
    ///     handler_attributes: [];
    ///     contexts { radio: 3; }
    ///     far: FarToken = FarIrq => on_far, line 20, context radio, hart 1, Hardware;
    /// }
    /// let _ = oer_chips_family_platform_interrupt_table::Router::<_, 0, _>::install(FakeController::new(0), InterruptTable);
    /// ```
    ///
    /// ```compile_fail,E0080
    /// # oer_chips_family_platform_interrupt_table::__fake_parts!();
    /// # oer_chips_family_platform_interrupt_table::__fake_controller!();
    /// # pub fn on_timer(_: &mut TimerIrq) {}
    /// // Levels the controller's three level bits do not hold.
    /// const DEEP: oer_chips_family_platform_interrupt_table::Lines =
    ///     oer_chips_family_platform_interrupt_table::Lines { first: 16, count: 32, levels: 15, harts: 1, tie_break: oer_chips_family_platform_interrupt_table::TieBreak::HighestLine };
    /// oer_chips_family_platform_interrupt_table::interrupt_table! {
    ///     lines: DEEP,
    ///     handler_attributes: [];
    ///     contexts { deep: 12; }
    ///     timer: TimerToken = TimerIrq => on_timer, line 20, context deep, hart 0, Hardware;
    /// }
    /// let _ = oer_chips_family_platform_interrupt_table::Router::<_, 0, _>::install(FakeController::new(0), InterruptTable);
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::WrongHart`] when `controller` is another hart's than `H`,
    /// before any write; otherwise as [`verify`](Self::verify): the
    /// controller did not take a write.
    pub fn install(mut controller: C, table: T) -> Result<Self, Error> {
        const {
            assert!(C::LINES <= 64, "a controller has at most 64 lines");
            assert!(
                table_inside(T::DECLARED, C::SOURCES, C::LINES, C::LEVEL_BITS.levels()),
                "an entry of the table lies outside the controller"
            );
        };
        let current = controller.hart();
        if current != H {
            return Err(Error::WrongHart { hart: H, current });
        }
        controller.verify_configuration()?;
        for source in 0..C::SOURCES {
            controller.silence(source)?;
        }
        for line in 0..C::LINES {
            controller.disable(line)?;
        }
        let router = Self { controller, table };
        router.verify()?;
        Ok(router)
    }

    /// Bind `token`'s entry: move `part` where only the entry's handler
    /// reaches it, configure and enable the entry's line, then route each
    /// of the part's sources ([`InterruptSources`]) to it.
    ///
    /// The entry is the router's table's by type, at the line, level, class
    /// and handler its one declaration gives both; an entry of another table
    /// fails to compile:
    ///
    /// ```compile_fail,E0271
    /// # oer_chips_family_platform_interrupt_table::__fake_parts!();
    /// # oer_chips_family_platform_interrupt_table::__fake_controller!();
    /// # pub fn on_timer(_: &mut TimerIrq) {}
    /// oer_chips_family_platform_interrupt_table::interrupt_table! {
    ///     lines: FAKE_LINES,
    ///     handler_attributes: [];
    ///     contexts { radio: 3; }
    ///     timer: TimerToken = TimerIrq => on_timer, line 20, context radio, hart 0, Hardware;
    /// }
    /// // A router of the table of no entry.
    /// fn bind(router: &mut oer_chips_family_platform_interrupt_table::Router<FakeController, 0, oer_chips_family_platform_interrupt_table::NoBindings>) {
    ///     let tokens = Interrupts::take().unwrap();
    ///     let _ = router.bind(tokens.timer, TimerIrq::default());
    /// }
    /// ```
    ///
    /// An entry of another hart than the router's fails to compile too:
    ///
    /// ```compile_fail,E0080
    /// # oer_chips_family_platform_interrupt_table::__fake_parts!();
    /// # oer_chips_family_platform_interrupt_table::__fake_controller!();
    /// # pub fn on_timer(_: &mut TimerIrq) {}
    /// oer_chips_family_platform_interrupt_table::interrupt_table! {
    ///     lines: FAKE_LINES,
    ///     handler_attributes: [];
    ///     contexts { radio: 3; }
    ///     timer: TimerToken = TimerIrq => on_timer, line 20, context radio, hart 1, Hardware;
    /// }
    /// // Hart zero's router.
    /// fn bind(router: &mut oer_chips_family_platform_interrupt_table::Router<FakeController, 0, InterruptTable>) {
    ///     let tokens = Interrupts::take().unwrap();
    ///     let _ = router.bind(tokens.timer, TimerIrq::default());
    /// }
    /// # bind(&mut oer_chips_family_platform_interrupt_table::Router::install(FakeController::new(0), InterruptTable).unwrap());
    /// ```
    ///
    /// # Errors
    ///
    /// A binding that cannot claim storage, install its handler or write its
    /// controller returns its [`Quarantined`] owners: the image must record
    /// the fault and reset. Nothing else refuses it.
    pub fn bind<E: Entry<Table = T>>(
        &mut self,
        token: E,
        part: E::Part,
    ) -> Result<Bound<E>, Quarantined<E>> {
        const {
            assert!(
                inside_matrix(<E::Part as InterruptSources>::SOURCES, C::SOURCES),
                "the entry's source is no source of the controller"
            );
            assert!(
                E::LINE < C::LINES,
                "the entry's line is no line of the controller"
            );
            assert!(
                E::LEVEL >= 1 && E::LEVEL as u16 <= C::LEVEL_BITS.levels(),
                "the entry's level is no level of the controller's lines"
            );
            assert!(E::HART == H, "the entry belongs to the router's hart");
        };
        let sources = <E::Part as InterruptSources>::SOURCES;
        // A part has a source (checked at compile time).
        let source = match sources {
            [first, ..] => *first,
            [] => 0,
        };
        let proof = E::PROOF;
        let fault = |source, kind| BindFault {
            source,
            line: E::LINE,
            hart: H,
            kind,
        };
        let Some(storage) = E::storage().try_uninit() else {
            return Err(Quarantined::owned(
                fault(source, BindFaultKind::StorageClaimed),
                token,
                part,
            ));
        };
        let place = storage.write(part);
        if let Err(place) = proof.cell().install(place) {
            return Err(Quarantined::placed(
                fault(source, BindFaultKind::HandlerOccupied),
                token,
                place,
            ));
        }
        if let Err(error) = self.controller.bind(
            E::LINE,
            const {
                match C::LEVEL_BITS.control(E::LEVEL as u16) {
                    Ok(control) => control,
                    Err(_) => panic!("the entry's level is no level of the controller"),
                }
            },
        ) {
            return Err(Quarantined::installed(
                fault(source, BindFaultKind::LineRefused(error)),
                token,
            ));
        }
        for &source in sources {
            if let Err(error) = self.controller.route(source, E::LINE) {
                return Err(Quarantined::installed(
                    fault(source, BindFaultKind::RouteRefused(error)),
                    token,
                ));
            }
        }
        let _consumed = token;
        Ok(Bound {
            _part: core::marker::PhantomData,
        })
    }

    /// Prove that each of `P`'s sources is routed nowhere on this hart and
    /// that no entry of the table binds one: the router, their one writer,
    /// never routes them then. A driver that polls `P`'s events takes the
    /// proof before it enables them.
    ///
    /// A part a table's entry binds, on any hart, has no silence: it fails
    /// to compile.
    ///
    /// ```compile_fail,E0080
    /// # oer_chips_family_platform_interrupt_table::__fake_parts!();
    /// # oer_chips_family_platform_interrupt_table::__fake_controller!();
    /// # pub fn on_timer(_: &mut TimerIrq) {}
    /// oer_chips_family_platform_interrupt_table::interrupt_table! {
    ///     lines: FAKE_LINES,
    ///     handler_attributes: [];
    ///     contexts { radio: 3; }
    ///     timer: TimerToken = TimerIrq => on_timer, line 20, context radio, hart 1, Hardware;
    /// }
    /// fn silent(router: &oer_chips_family_platform_interrupt_table::Router<FakeController, 0, InterruptTable>) {
    ///     let _ = router.silent::<TimerIrq>();
    /// }
    /// # silent(&oer_chips_family_platform_interrupt_table::Router::install(FakeController::new(0), InterruptTable).unwrap());
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::ForeignRoute`] when a source is routed.
    pub fn silent<P: InterruptSources>(&self) -> Result<Silent<P>, Error> {
        const {
            assert!(
                inside_matrix(P::SOURCES, C::SOURCES),
                "the part's source is no source of the controller"
            );
            assert!(
                !table_binds(T::DECLARED, P::SOURCES),
                "an entry of the table binds the part's source"
            );
        };
        for &source in P::SOURCES {
            if let Some(line) = self.controller.routed(source) {
                return Err(Error::ForeignRoute { source, line });
            }
        }
        Ok(Silent {
            _part: core::marker::PhantomData,
        })
    }

    /// Check the hart's routes and lines against the table: every routed
    /// source is an entry of this hart routed to its line, every line a
    /// routed entry holds is bound at its level, and every other line is
    /// disabled.
    ///
    /// # Errors
    ///
    /// The first disagreement.
    pub fn verify(&self) -> Result<(), Error> {
        self.controller.verify_configuration()?;
        let hart = H;
        // Bit `line` of the lines a routed source holds; a line past 63
        // is no line of a controller (`LINES` is at most 64).
        let mut routed_lines = 0u64;
        for source in 0..C::SOURCES {
            let Some(line) = self.controller.routed(source) else {
                continue;
            };
            let Some(binding) = present(self.table.bindings(), hart)
                .find(|binding| binding.source == source && binding.line == line)
            else {
                return Err(Error::ForeignRoute { source, line });
            };
            routed_lines |= 1u64.checked_shl(u32::from(line)).unwrap_or(0);
            let control = C::LEVEL_BITS
                .control(u16::from(binding.level))
                .map_err(|_| Error::NoSuchLevel {
                    level: binding.level,
                })?;
            self.controller
                .check(line, LineExpectation::Bound(control))?;
        }
        for line in (0..C::LINES).filter(|&line| routed_lines >> line & 1 == 0) {
            self.controller.check(line, LineExpectation::Disabled)?;
        }
        Ok(())
    }

    /// The hart this router writes.
    pub const fn hart(&self) -> u8 {
        H
    }
}
