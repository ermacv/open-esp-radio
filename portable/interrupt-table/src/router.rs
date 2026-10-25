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

use oer_mmio::{InterruptSource, InterruptSources};

use crate::{Binding, Entry, Table, line_control};

/// The trigger of a CPU line (the CLIC's `clicintattr.trig`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Trigger {
    /// Positive level: the line follows its source. The only trigger the
    /// table binds, so that a write of the line's word that keeps its
    /// pending bit neither loses nor invents an interrupt.
    PositiveLevel,
    PositiveEdge,
    NegativeLevel,
    NegativeEdge,
}

/// One CPU line as its controller holds it (the CLIC's `clicintie`,
/// `clicintattr.shv`, `clicintattr.trig` and `clicintctl`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LineState {
    /// The line's interrupt enable.
    pub enabled: bool,
    /// Hardware vectoring: the hart enters through the line's vector.
    pub vectored: bool,
    pub trigger: Trigger,
    /// The level and priority control byte.
    pub control: u8,
}

impl LineState {
    /// A disabled line.
    pub const DISABLED: Self = Self {
        enabled: false,
        vectored: false,
        trigger: Trigger::PositiveLevel,
        control: 0,
    };

    /// The line of a bound entry at `level`, of a controller with
    /// `level_bits` level bits: enabled, hardware-vectored,
    /// positive-level-triggered, holding the level.
    #[must_use]
    pub const fn bound(level: u8, level_bits: u8) -> Self {
        Self {
            enabled: true,
            vectored: true,
            trigger: Trigger::PositiveLevel,
            control: line_control(level, level_bits),
        }
    }
}

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
    const LEVEL_BITS: u8;

    /// The hart whose bank and CLIC this controller writes.
    fn hart(&self) -> u8;
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
    /// The entry of `source` belongs to `hart`; the router writes `current`.
    WrongHart { source: u16, hart: u8, current: u8 },
    /// The table holds no entry of the token's source, line, level and hart.
    NotInTable { source: u16 },
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
pub struct Bound<P: InterruptSources> {
    _part: core::marker::PhantomData<fn() -> P>,
}

impl<P: InterruptSources> core::fmt::Debug for Bound<P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Bound")
    }
}

impl<P: InterruptSource> Bound<P> {
    /// The bound source.
    pub const SOURCE: u16 = P::SOURCE;
}

/// The proof that every source of `P` is routed nowhere on the router's
/// hart and that no entry of the table binds one, so the router never routes it:
/// a driver that polls `P`'s events may enable them without a handler.
/// Neither `Copy` nor `Clone`; only [`Router::silent`] makes one.
///
/// On a chip of several harts each hart's router silences its own bank at
/// its install; the table binds the source on none of them.
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
/// sources `P::SOURCES`: [`Bound<P>`] (a handler holds the part and its line
/// is routed) or [`Silent<P>`] (no line receives the source). Sealed: only
/// the router makes either.
pub trait EventAuthority<P: InterruptSources>: sealed::Sealed {}

impl<P: InterruptSources> EventAuthority<P> for Bound<P> {}
impl<P: InterruptSources> EventAuthority<P> for Silent<P> {}

mod sealed {
    pub trait Sealed {}
    impl<P: super::InterruptSources> Sealed for super::Bound<P> {}
    impl<P: super::InterruptSources> Sealed for super::Silent<P> {}
}

/// What [`Router::bind`] refused, with the token and the part it gives back.
pub struct Refused<E: Entry> {
    pub error: Error,
    pub token: E,
    pub part: E::Part,
}

impl<E: Entry> core::fmt::Debug for Refused<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Refused")
            .field("error", &self.error)
            .finish()
    }
}

/// The one writer of a hart's routes and lines, by the image's table.
pub struct Router<C: Controller> {
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

impl<C: Controller> Router<C> {
    /// Take `controller`'s hart for `table`, the image's
    /// `INTERRUPT_TABLE`: silence every source and disable every line of the
    /// hart, then [`verify`](Self::verify). Interrupts must stay disabled on
    /// the hart until it returns.
    ///
    /// # Errors
    ///
    /// [`Error::NoSuchSource`], [`Error::NoSuchLine`] or
    /// [`Error::NoSuchLevel`] when an entry of the hart lies outside the
    /// controller, before any write; otherwise as
    /// [`verify`](Self::verify): the controller did not take a write.
    pub fn install(mut controller: C, table: &'static Table) -> Result<Self, Error> {
        const { assert!(C::LINES <= 64, "a controller has at most 64 lines") };
        for binding in present(table, controller.hart()) {
            Self::inside(binding.source, binding.line, binding.level)?;
        }
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
    /// [`Error::WrongHart`] when the entry belongs to another hart and
    /// [`Error::NotInTable`] when the router's table lacks the entry;
    /// nothing changes and the token and the part come back. An entry whose
    /// source, line or level lies outside the controller fails to compile:
    ///
    /// ```compile_fail,E0080
    /// # oer_interrupt_table::__fake_parts!();
    /// # oer_interrupt_table::__fake_controller!();
    /// # pub fn on_timer(_: &mut TimerIrq) {}
    /// // The table's lines reach past the controller's 48.
    /// const WIDE: oer_interrupt_table::Lines =
    ///     oer_interrupt_table::Lines { first: 16, count: 64, levels: 8, harts: 1 };
    /// oer_interrupt_table::interrupt_table! {
    ///     lines: WIDE,
    ///     handler_attributes: [];
    ///     timer: TimerToken = TimerIrq => on_timer, line 60, level 3, hart 0, Hardware;
    /// }
    /// fn bind(router: &mut oer_interrupt_table::Router<FakeController>) {
    ///     let tokens = Interrupts::take().unwrap();
    ///     let _ = router.bind(tokens.timer, TimerIrq::default());
    /// }
    /// # bind(&mut oer_interrupt_table::Router::install(FakeController::new(0), INTERRUPT_TABLE).unwrap());
    /// ```
    ///
    /// ```compile_fail,E0080
    /// # oer_interrupt_table::__fake_parts!();
    /// # oer_interrupt_table::__fake_controller!();
    /// /// A source past the fake controller's eight.
    /// pub struct FarIrq;
    /// impl oer_interrupt_table::InterruptSource for FarIrq {
    ///     const SOURCE: u16 = 84;
    /// }
    /// # pub fn on_far(_: &mut FarIrq) {}
    /// oer_interrupt_table::interrupt_table! {
    ///     lines: FAKE_LINES,
    ///     handler_attributes: [];
    ///     far: FarToken = FarIrq => on_far, line 20, level 3, hart 0, Hardware;
    /// }
    /// fn bind(router: &mut oer_interrupt_table::Router<FakeController>) {
    ///     let tokens = Interrupts::take().unwrap();
    ///     let _ = router.bind(tokens.far, FarIrq);
    /// }
    /// # bind(&mut oer_interrupt_table::Router::install(FakeController::new(0), INTERRUPT_TABLE).unwrap());
    /// ```
    pub fn bind<E: Entry>(
        &mut self,
        token: E,
        part: E::Part,
    ) -> Result<Bound<E::Part>, Refused<E>> {
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
                E::LEVEL >= 1 && E::LEVEL as u32 <= 1u32 << C::LEVEL_BITS,
                "the entry's level is no level of the controller's lines"
            );
        };
        let sources = <E::Part as InterruptSources>::SOURCES;
        // A part has a source (checked at compile time).
        let source = match sources {
            [first, ..] => *first,
            [] => 0,
        };
        let current = self.controller.hart();
        let refuse = |error, token, part| Err(Refused { error, token, part });
        if E::HART != current {
            return refuse(
                Error::WrongHart {
                    source,
                    hart: E::HART,
                    current,
                },
                token,
                part,
            );
        }
        if let Some(&source) = sources.iter().find(|&&source| {
            !present(self.table, current).any(|binding| {
                binding.source == source && binding.line == E::LINE && binding.level == E::LEVEL
            })
        }) {
            return refuse(Error::NotInTable { source }, token, part);
        }
        let _consumed = token;
        let place = E::storage().init(part);
        #[expect(
            clippy::expect_used,
            reason = "the token is unique, so its cell is empty until this install"
        )]
        let _installed = E::cell()
            .install(place)
            .ok()
            .expect("an entry's part is installed once");
        #[expect(
            clippy::expect_used,
            reason = "the entry's source, line and level lie inside the controller (checked at compile time)"
        )]
        {
            self.controller
                .set_line(E::LINE, LineState::bound(E::LEVEL, C::LEVEL_BITS))
                .expect("the entry's line is a line of the controller");
            for &source in sources {
                self.controller
                    .route(source, E::LINE)
                    .expect("the entry's source and line are the controller's");
            }
        }
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
        if level == 0 || u32::from(level) > 1u32 << C::LEVEL_BITS {
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
        let hart = self.controller.hart();
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
            let expected = LineState::bound(binding.level, C::LEVEL_BITS);
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
    pub fn hart(&self) -> u8 {
        self.controller.hart()
    }
}
