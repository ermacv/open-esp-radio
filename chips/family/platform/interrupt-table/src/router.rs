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

use crate::{
    __private::Declared, BindFault, BindFaultKind, Binding, Entry, Quarantined, Table, TaskBinding,
};

use oer_portable_riscv::clic::{Control, LevelBits};

/// A hart's interrupt matrix bank and CLIC, as the table writes them. The
/// chip implements it on its typed blocks; each method is a few word reads
/// or writes, never a wait.
///
/// A source and a line are the controller's own values ([`Self::Source`],
/// [`Self::Line`]), made once, in a constant, for each source and line it
/// has: the router takes an entry's by its number at compile time, so a
/// write names only what the controller has and cannot be refused.
pub trait Controller {
    /// A source of the chip's interrupt matrix.
    type Source: Copy + 'static;
    /// A CPU line of the hart's CLIC.
    type Line: Copy + 'static;
    /// Every source of the chip's interrupt matrix, source `n` at `n`.
    const SOURCES: &'static [Self::Source];
    /// Every CPU line of the hart's CLIC, line `n` at `n`: at most 64.
    const LINES: &'static [Self::Line];
    /// The level bits of a line's control byte (`CLIC_INT_CONFIG.MNLBITS`).
    const LEVEL_BITS: LevelBits;

    /// The hart whose bank and CLIC this controller writes.
    fn hart(&self) -> u8;
    /// Check the controller's implemented geometry and selected configuration
    /// against its chip facts, before installation and before enabling IRQs.
    fn verify_configuration(&self) -> Result<(), Error>;
    /// Route `source` to `line` on this hart.
    fn route(&mut self, source: Self::Source, line: Self::Line);
    /// Route `source` nowhere on this hart.
    fn silence(&mut self, source: Self::Source);
    /// The number of the line `source` is routed to on this hart, as the
    /// hardware reads it; `None` when silent.
    fn routed(&self, source: Self::Source) -> Option<u8>;
    /// Bind `line` at the level of `control`: enabled, hardware-vectored,
    /// with the trigger the chip's lines have, keeping its pending bit. The
    /// line is never enabled with another attribute or control.
    fn bind(&mut self, line: Self::Line, control: Control);
    /// Bind `line` as a level task's at the level of `control`: enabled,
    /// hardware-vectored and positive-edge, since software pends a line only
    /// on an edge trigger; its pending bit cleared before the enable, with
    /// the trigger already an edge. The line is never enabled with another
    /// attribute or control.
    fn bind_task(&mut self, line: Self::Line, control: Control);
    /// Disable `line`, keeping its pending bit.
    fn disable(&mut self, line: Self::Line);
    /// Check `line` against what the table left in it: a disabled line is
    /// not enabled, a bound one holds exactly its binding.
    ///
    /// # Errors
    ///
    /// The line's observation when it holds something else.
    fn check(&self, line: Self::Line, expected: LineExpectation) -> Result<(), LineObservation>;
}

/// What the table leaves in a line: the family's own value, which names no
/// trigger, enable or vectoring of a chip.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineExpectation {
    /// The line is not enabled.
    Disabled,
    /// The line is bound at this control byte's level.
    Bound(Control),
    /// The line is a level task's, bound at this control byte's level.
    Task(Control),
}

impl LineExpectation {
    /// The expectation's word in a record, encoded here alone: 0 disabled,
    /// bit 8 and the control byte bound, bit 9 and the control byte a level
    /// task's.
    pub const fn record(self) -> u32 {
        let (kind, control) = match self {
            Self::Disabled => return 0,
            Self::Bound(control) => (0x100, control),
            Self::Task(control) => (0x200, control),
        };
        kind | control.bits() as u32
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

    /// The observation of a line laid out as four byte registers, as they
    /// read: byte `n` of the record is the line's byte register `n`.
    pub const fn of_bytes(bytes: [u8; 4]) -> Self {
        Self(u32::from_le_bytes(bytes))
    }

    /// The word, for a record.
    pub const fn record(self) -> u32 {
        self.0
    }
}

/// How a hart's controller or its routes and lines disagree with the chip
/// facts and the table: a typed boot failure.
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
const fn inside_matrix(sources: &[u16], count: usize) -> bool {
    if sources.is_empty() {
        return false;
    }
    let mut rest = sources;
    while let [source, tail @ ..] = rest {
        rest = tail;
        if *source as usize >= count {
            return false;
        }
    }
    true
}

/// Whether every entry `table` declares that the build holds lies inside a
/// controller of `sources`, `lines` and `levels`: the table at compile time.
const fn table_inside(table: &[Declared], sources: usize, lines: usize, levels: u16) -> bool {
    let mut rest = table;
    while let [entry, tail @ ..] = rest {
        rest = tail;
        let binding = &entry.binding;
        if entry.present
            && (!inside_matrix(entry.sources, sources)
                || binding.line as usize >= lines
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
/// The control byte of each line of the controller that a present level task
/// of hart `hart` holds, line `n` at `n`; the first task outside the
/// controller's lines or levels refuses.
const fn task_controls(
    tasks: &[TaskBinding],
    hart: u8,
    lines: usize,
    bits: LevelBits,
) -> Result<[Option<Control>; 64], &'static str> {
    let mut controls = [None; 64];
    let mut rest = tasks;
    while let [task, tail @ ..] = rest {
        rest = tail;
        if task.handler.is_none() || task.hart != hart {
            continue;
        }
        if task.line as usize >= lines {
            return Err("a level task's line is no line of the controller");
        }
        let Ok(control) = bits.control(task.level as u16) else {
            return Err("a level task's level is no level of the controller's lines");
        };
        let mut slots: &mut [Option<Control>] = &mut controls;
        let mut at = 0;
        while let [slot, more @ ..] = slots {
            if at == task.line as usize {
                *slot = Some(control);
            }
            slots = more;
            at += 1;
        }
    }
    Ok(controls)
}

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

/// The `n`th of `all`; `None` past its end: a controller's line or source
/// by its number, at compile time.
const fn nth<V: Copy>(all: &[V], n: usize) -> Option<V> {
    let mut rest = all;
    let mut at = 0;
    while let [first, tail @ ..] = rest {
        if at == n {
            return Some(*first);
        }
        rest = tail;
        at += 1;
    }
    None
}

/// Each of the controller's sources whose number `numbers` holds, with its
/// number, in the order of the numbers.
fn sources_of<C: Controller>(numbers: &'static [u16]) -> impl Iterator<Item = (C::Source, u16)> {
    C::SOURCES
        .iter()
        .copied()
        .zip(0u16..)
        .filter(move |(_, number)| numbers.contains(number))
}

/// The entries of `table` the build holds, on `hart`.
fn present(table: &[Binding], hart: u8) -> impl Iterator<Item = &Binding> {
    table
        .iter()
        .filter(move |binding| binding.handler.is_some() && binding.hart == hart)
}

impl<C: Controller, const H: u8, T: Table> Router<C, H, T> {
    /// The control byte of each line a level task of the hart holds, line
    /// `n` at `n`: made at compile time from the table, so neither
    /// [`install`](Self::install) nor [`verify`](Self::verify) looks a task
    /// up or refuses one.
    const TASKS: [Option<Control>; 64] =
        match task_controls(T::TASKS, H, C::LINES.len(), C::LEVEL_BITS) {
            Ok(controls) => controls,
            Err(why) => panic!("{}", why),
        };

    /// Take `controller`'s hart for `table`, the image's `InterruptTable`:
    /// silence every source and disable every line of the hart, bind the
    /// line of each level task of the hart ([`Controller::bind_task`]), then
    /// [`verify`](Self::verify). Interrupts must stay disabled on the hart
    /// until it returns. A task's line holds no source and nothing pends it
    /// before its raisers exist, so it is bound here, for good, with nothing
    /// to place first.
    ///
    /// A table whose entry or level task names a source, a line or a level
    /// the controller has not fails to compile, on the task's hart:
    ///
    /// ```compile_fail,E0080
    /// # oer_chips_family_platform_interrupt_table::__fake_parts!();
    /// # oer_chips_family_platform_interrupt_table::__fake_controller!();
    /// # pub fn on_far() {}
    /// // The table's lines reach past the controller's 48.
    /// const WIDE: oer_chips_family_platform_interrupt_table::Lines =
    ///     oer_chips_family_platform_interrupt_table::Lines { first: 16, count: 64, levels: 8, harts: 1, tie_break: oer_chips_family_platform_interrupt_table::TieBreak::HighestLine };
    /// oer_chips_family_platform_interrupt_table::interrupt_table! {
    ///     lines: WIDE,
    ///     handler_attributes: [];
    ///     level_tasks { FarTask => on_far, line 60, context keeper, hart 0; }
    ///     contexts { keeper: 3; }
    /// }
    /// let _ = oer_chips_family_platform_interrupt_table::Router::<_, 0, _>::install(FakeController::new(0), InterruptTable);
    /// ```
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
            if C::LINES.len() > 64 {
                panic!("a controller has at most 64 lines")
            }
            if !table_inside(
                T::DECLARED,
                C::SOURCES.len(),
                C::LINES.len(),
                C::LEVEL_BITS.levels(),
            ) {
                panic!("an entry of the table lies outside the controller")
            }
        };
        let current = controller.hart();
        if current != H {
            return Err(Error::WrongHart { hart: H, current });
        }
        controller.verify_configuration()?;
        for &source in C::SOURCES {
            controller.silence(source);
        }
        for &line in C::LINES {
            controller.disable(line);
        }
        for (&line, task) in C::LINES.iter().zip(&Self::TASKS) {
            if let Some(control) = *task {
                controller.bind_task(line, control);
            }
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
    /// A binding that cannot claim storage or install its handler returns
    /// its [`Quarantined`] owners: the image must record the fault and
    /// reset. Nothing else refuses it: the controller's writes name only its
    /// own line and sources, at compile time.
    pub fn bind<E: Entry<Table = T>>(
        &mut self,
        token: E,
        part: E::Part,
    ) -> Result<Bound<E>, Quarantined<E>> {
        const {
            if !inside_matrix(<E::Part as InterruptSources>::SOURCES, C::SOURCES.len()) {
                panic!("the entry's source is no source of the controller")
            }
            if E::LEVEL < 1 || E::LEVEL as u16 > C::LEVEL_BITS.levels() {
                panic!("the entry's level is no level of the controller's lines")
            }
            if E::HART != H {
                panic!("the entry belongs to the router's hart")
            }
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
        let line = const {
            match nth(C::LINES, E::LINE as usize) {
                Some(line) => line,
                None => panic!("the entry's line is no line of the controller"),
            }
        };
        self.controller.bind(
            line,
            const {
                match C::LEVEL_BITS.control(E::LEVEL as u16) {
                    Ok(control) => control,
                    Err(_) => panic!("the entry's level is no level of the controller"),
                }
            },
        );
        for (source, _) in sources_of::<C>(sources) {
            self.controller.route(source, line);
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
            if !inside_matrix(P::SOURCES, C::SOURCES.len()) {
                panic!("the part's source is no source of the controller")
            }
            if table_binds(T::DECLARED, P::SOURCES) {
                panic!("an entry of the table binds the part's source")
            }
        };
        for (source, number) in sources_of::<C>(P::SOURCES) {
            if let Some(line) = self.controller.routed(source) {
                return Err(Error::ForeignRoute {
                    source: number,
                    line,
                });
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
        for (typed, source) in C::SOURCES.iter().copied().zip(0u16..) {
            let Some(line) = self.controller.routed(typed) else {
                continue;
            };
            if !present(self.table.bindings(), hart)
                .any(|binding| binding.source == source && binding.line == line)
            {
                return Err(Error::ForeignRoute { source, line });
            }
            routed_lines |= 1u64.checked_shl(u32::from(line)).unwrap_or(0);
        }
        for ((typed, line), &task) in C::LINES.iter().copied().zip(0u8..).zip(&Self::TASKS) {
            let routed = present(self.table.bindings(), hart)
                .find(|binding| binding.line == line)
                .filter(|_| routed_lines >> line & 1 == 1);
            let expected = match (task, routed) {
                (Some(control), _) => LineExpectation::Task(control),
                (None, Some(binding)) => LineExpectation::Bound(
                    C::LEVEL_BITS
                        .control(u16::from(binding.level))
                        .map_err(|_| Error::NoSuchLevel {
                            level: binding.level,
                        })?,
                ),
                (None, None) => LineExpectation::Disabled,
            };
            self.controller
                .check(typed, expected)
                .map_err(|observed| Error::WrongLine {
                    line,
                    observed,
                    expected,
                })?;
        }
        Ok(())
    }

    /// The hart this router writes.
    pub const fn hart(&self) -> u8 {
        H
    }
}
