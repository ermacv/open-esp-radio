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

use oer_mmio::{InterruptSource, InterruptSources};

use crate::{BindError, BindFault, BindFaultKind, Binding, Entry, Quarantined, Table};

use oer_riscv::clic::{LevelBits, LineState};

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
    /// Write `line`'s enable, attributes and control, keeping its pending
    /// bit. The line is never enabled with another attribute or control
    /// than `state`'s.
    ///
    /// # Errors
    ///
    /// [`Error::NoSuchLine`]; nothing is written.
    fn set_line(&mut self, line: u8, state: LineState) -> Result<(), Error>;
    /// Read `line`'s enable, attributes and control.
    fn line(&self, line: u8) -> LineState;
}

/// How a hart's routes and lines disagree with the table, or a binding the
/// table refuses: a typed boot failure.
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
    /// The table holds no entry of the token's source, line, level and hart.
    NotInTable { source: u16 },
    /// The token and table assign different handler classes to this source.
    WrongClass {
        source: u16,
        table: crate::Class,
        entry: crate::Class,
    },
    /// The table's vector and token do not identify the same handler cell.
    HandlerCellMismatch { source: u16 },
    /// `source` is routed to `line`, which no entry of this hart binds it to.
    ForeignRoute { source: u16, line: u8 },
    /// `line` holds `state`, not what the table's entries leave in it.
    WrongLine {
        line: u8,
        state: LineState,
        expected: LineState,
    },
    /// `source` is no source of the controller's matrix (`0..SOURCES`).
    NoSuchSource { source: u16 },
    /// `line` is no line of the controller's CLIC (`0..LINES`).
    NoSuchLine { line: u8 },
    /// `level` is no level of the controller's lines
    /// (`1..=1 << LEVEL_BITS`).
    NoSuchLevel { level: u8 },
    /// An entry of the table binds `source`, so no silence of it holds.
    Bindable { source: u16 },
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

/// The one writer of hart `H`'s routes and lines, by the image's table. A
/// binding of an entry of another hart fails to compile.
pub struct Router<C: Controller, const H: u8> {
    pub(crate) controller: C,
    table: &'static Table,
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

/// The entries of `table` the build holds, on `hart`.
fn present(table: &Table, hart: u8) -> impl Iterator<Item = &Binding> {
    table
        .iter()
        .filter(move |binding| binding.handler.is_some() && binding.hart == hart)
}

impl<C: Controller, const H: u8> Router<C, H> {
    /// Take `controller`'s hart for `table`, the image's
    /// `INTERRUPT_TABLE`: silence every source and disable every line of the
    /// hart, then [`verify`](Self::verify). Interrupts must stay disabled on
    /// the hart until it returns.
    ///
    /// # Errors
    ///
    /// [`Error::WrongHart`] when `controller` is another hart's than `H`,
    /// before any write; [`Error::NoSuchSource`], [`Error::NoSuchLine`] or
    /// [`Error::NoSuchLevel`] when an entry of the hart lies outside the
    /// controller, before any write; otherwise as
    /// [`verify`](Self::verify): the controller did not take a write.
    pub fn install(mut controller: C, table: &'static Table) -> Result<Self, Error> {
        const { assert!(C::LINES <= 64, "a controller has at most 64 lines") };
        let current = controller.hart();
        if current != H {
            return Err(Error::WrongHart { hart: H, current });
        }
        for binding in present(table, H) {
            Self::inside(binding.source, binding.line, binding.level)?;
        }
        controller.verify_configuration()?;
        for source in 0..C::SOURCES {
            controller.silence(source)?;
        }
        for line in 0..C::LINES {
            controller.set_line(line, LineState::DISABLED)?;
        }
        let router = Self { controller, table };
        router.verify()?;
        Ok(router)
    }

    /// Bind `token`'s entry: move `part` where only the entry's handler
    /// reaches it, configure and enable the entry's line, then route each
    /// of the part's sources ([`InterruptSources`]) to it.
    ///
    /// # Errors
    ///
    /// [`Error::NotInTable`] when the router's table lacks the entry;
    /// nothing changes and `BindError::Refused` returns the token and part.
    /// An admitted binding that cannot claim storage, install its handler or
    /// write its controller returns `BindError::Fault`: its owners stay in
    /// quarantine and the image must record the fault and reset. An entry whose
    /// source, line or level lies outside the controller fails to compile:
    ///
    /// ```compile_fail,E0080
    /// # oer_espressif_interrupt_table::__fake_parts!();
    /// # oer_espressif_interrupt_table::__fake_controller!();
    /// # pub fn on_timer(_: &mut TimerIrq) {}
    /// // The table's lines reach past the controller's 48.
    /// const WIDE: oer_espressif_interrupt_table::Lines =
    ///     oer_espressif_interrupt_table::Lines { first: 16, count: 64, levels: 8, harts: 1, tie_break: oer_espressif_interrupt_table::TieBreak::HighestLine };
    /// oer_espressif_interrupt_table::interrupt_table! {
    ///     lines: WIDE,
    ///     handler_attributes: [];
    ///     contexts { radio: 3; }
    ///     timer: TimerToken = TimerIrq => on_timer, line 60, context radio, hart 0, Hardware;
    /// }
    /// fn bind(router: &mut oer_espressif_interrupt_table::Router<FakeController, 0>) {
    ///     let tokens = Interrupts::take().unwrap();
    ///     let _ = router.bind(tokens.timer, TimerIrq::default());
    /// }
    /// # bind(&mut oer_espressif_interrupt_table::Router::install(FakeController::new(0), INTERRUPT_TABLE).unwrap());
    /// ```
    ///
    /// ```compile_fail,E0080
    /// # oer_espressif_interrupt_table::__fake_parts!();
    /// # oer_espressif_interrupt_table::__fake_controller!();
    /// /// A source past the fake controller's eight.
    /// pub struct FarIrq;
    /// impl oer_espressif_interrupt_table::InterruptSource for FarIrq {
    ///     const SOURCE: u16 = 84;
    /// }
    /// # pub fn on_far(_: &mut FarIrq) {}
    /// oer_espressif_interrupt_table::interrupt_table! {
    ///     lines: FAKE_LINES,
    ///     handler_attributes: [];
    ///     contexts { radio: 3; }
    ///     far: FarToken = FarIrq => on_far, line 20, context radio, hart 0, Hardware;
    /// }
    /// fn bind(router: &mut oer_espressif_interrupt_table::Router<FakeController, 0>) {
    ///     let tokens = Interrupts::take().unwrap();
    ///     let _ = router.bind(tokens.far, FarIrq);
    /// }
    /// # bind(&mut oer_espressif_interrupt_table::Router::install(FakeController::new(0), INTERRUPT_TABLE).unwrap());
    /// ```
    ///
    /// An entry of another hart than the router's fails to compile too:
    ///
    /// ```compile_fail,E0080
    /// # oer_espressif_interrupt_table::__fake_parts!();
    /// # oer_espressif_interrupt_table::__fake_controller!();
    /// # pub fn on_timer(_: &mut TimerIrq) {}
    /// oer_espressif_interrupt_table::interrupt_table! {
    ///     lines: FAKE_LINES,
    ///     handler_attributes: [];
    ///     contexts { radio: 3; }
    ///     timer: TimerToken = TimerIrq => on_timer, line 20, context radio, hart 1, Hardware;
    /// }
    /// // Hart zero's router.
    /// fn bind(router: &mut oer_espressif_interrupt_table::Router<FakeController, 0>) {
    ///     let tokens = Interrupts::take().unwrap();
    ///     let _ = router.bind(tokens.timer, TimerIrq::default());
    /// }
    /// # bind(&mut oer_espressif_interrupt_table::Router::install(FakeController::new(0), INTERRUPT_TABLE).unwrap());
    /// ```
    pub fn bind<E: Entry>(&mut self, token: E, part: E::Part) -> Result<Bound<E>, BindError<E>> {
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
        let refuse = |error, token, part| Err(BindError::Refused { error, token, part });
        let proof = E::PROOF;
        for &source in sources {
            let Some(binding) = present(self.table, H).find(|binding| {
                binding.source == source && binding.line == E::LINE && binding.level == E::LEVEL
            }) else {
                return refuse(Error::NotInTable { source }, token, part);
            };
            if binding.class != E::CLASS {
                return refuse(
                    Error::WrongClass {
                        source,
                        table: binding.class,
                        entry: E::CLASS,
                    },
                    token,
                    part,
                );
            }
            if !binding
                .handler
                .is_some_and(|handler| core::ptr::fn_addr_eq(handler, proof.handler()))
                || !core::ptr::eq(E::cell(), proof.cell())
            {
                return refuse(Error::HandlerCellMismatch { source }, token, part);
            }
        }
        let fault = |source, kind| BindFault {
            source,
            line: E::LINE,
            hart: H,
            kind,
        };
        let Some(storage) = E::storage().try_uninit() else {
            return Err(BindError::Fault(Quarantined::owned(
                fault(source, BindFaultKind::StorageClaimed),
                token,
                part,
            )));
        };
        let place = storage.write(part);
        if let Err(place) = proof.cell().install(place) {
            return Err(BindError::Fault(Quarantined::placed(
                fault(source, BindFaultKind::HandlerOccupied),
                token,
                place,
            )));
        }
        if let Err(error) = self.controller.set_line(
            E::LINE,
            LineState::vectored(
                const {
                    match C::LEVEL_BITS.control(E::LEVEL as u16) {
                        Ok(control) => control,
                        Err(_) => panic!("the entry's level is no level of the controller"),
                    }
                },
            ),
        ) {
            return Err(BindError::Fault(Quarantined::installed(
                fault(source, BindFaultKind::LineRefused(error)),
                token,
            )));
        }
        for &source in sources {
            if let Err(error) = self.controller.route(source, E::LINE) {
                return Err(BindError::Fault(Quarantined::installed(
                    fault(source, BindFaultKind::RouteRefused(error)),
                    token,
                )));
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
    /// # Errors
    ///
    /// [`Error::Bindable`] when an entry of the table, on any hart, binds a
    /// source; [`Error::ForeignRoute`] when a source is routed.
    pub fn silent<P: InterruptSources>(&self) -> Result<Silent<P>, Error> {
        const {
            assert!(
                inside_matrix(P::SOURCES, C::SOURCES),
                "the part's source is no source of the controller"
            );
        };
        for &source in P::SOURCES {
            if self
                .table
                .iter()
                .any(|binding| binding.handler.is_some() && binding.source == source)
            {
                return Err(Error::Bindable { source });
            }
            if let Some(line) = self.controller.routed(source) {
                return Err(Error::ForeignRoute { source, line });
            }
        }
        Ok(Silent {
            _part: core::marker::PhantomData,
        })
    }

    /// Check that an entry's source, line and level lie inside the
    /// controller.
    fn inside(source: u16, line: u8, level: u8) -> Result<(), Error> {
        if source >= C::SOURCES {
            return Err(Error::NoSuchSource { source });
        }
        if line >= C::LINES {
            return Err(Error::NoSuchLine { line });
        }
        if level == 0 || u16::from(level) > C::LEVEL_BITS.levels() {
            return Err(Error::NoSuchLevel { level });
        }
        Ok(())
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
            let Some(binding) = present(self.table, hart)
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
            let expected = LineState::vectored(control);
            let state = self.controller.line(line);
            if state != expected {
                return Err(Error::WrongLine {
                    line,
                    state,
                    expected,
                });
            }
        }
        for line in (0..C::LINES).filter(|&line| routed_lines >> line & 1 == 0) {
            let state = self.controller.line(line);
            if state.enabled {
                return Err(Error::WrongLine {
                    line,
                    state,
                    expected: LineState::DISABLED,
                });
            }
        }
        Ok(())
    }

    /// The hart this router writes.
    pub const fn hart(&self) -> u8 {
        H
    }
}
