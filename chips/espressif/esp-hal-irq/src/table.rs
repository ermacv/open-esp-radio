use esp_hal::{
    interrupt::{CoreInterruptRoutes, InterruptRoutes, Priority},
    peripherals::Interrupt,
    system::Cpu,
};
use oer_interrupt_table::{Adopted, Binding, Entry, Matrix, MatrixError, Table};
use oer_memory::zeroed::ZeroedOnce;
use oer_mmio::Index;

#[cfg(feature = "esp32c5")]
use oer_esp32c5_hal::platform;
#[cfg(feature = "esp32s31")]
use oer_esp32s31_hal::platform;
use platform::{
    Clic, InterruptCore0, InterruptCore0Status, InterruptMatrixHandoff, InterruptMatrixPeripherals,
};
#[cfg(feature = "esp32s31")]
use platform::{InterruptCore1, InterruptCore1Status};

use crate::lines::{self, LineError, LineState};

/// One core's route words: the interrupt-matrix block of that core.
enum Routes {
    Core0(InterruptCore0),
    #[cfg(feature = "esp32s31")]
    Core1(InterruptCore1),
}

/// What a source's route word holds.
enum RouteWord {
    /// The CPU line it routes to, zero for none.
    Line(u8),
    /// The word belongs to another block: the IEEE 802.15.4 route view owns
    /// the `ZB_MAC` source's word.
    Elsewhere,
}

/// The route word of `source` in `block`, a typed interrupt-matrix block of
/// module `$module`: two arrays around the `ZB_MAC` source.
macro_rules! route_word {
    ($block:expr, $module:ident, $source:expr) => {{
        let source = $source;
        const LOW: usize = platform::$module::map_low::COUNT;
        const HIGH: usize = platform::$module::map_high::COUNT;
        if let Ok(index) = Index::<LOW>::new(source) {
            Some(RouteWord::Line(
                $block.read_map_low(index).map().get().to_le_bytes()[0],
            ))
        } else if source == LOW {
            Some(RouteWord::Elsewhere)
        } else if let Some(index) = source
            .checked_sub(LOW + 1)
            .and_then(|high| Index::<HIGH>::new(high).ok())
        {
            Some(RouteWord::Line(
                $block.read_map_high(index).map().get().to_le_bytes()[0],
            ))
        } else {
            None
        }
    }};
}

impl Routes {
    fn word(&self, source: Interrupt) -> Option<RouteWord> {
        let source = source as usize;
        match self {
            Self::Core0(block) => route_word!(block, interrupt_core0, source),
            #[cfg(feature = "esp32s31")]
            Self::Core1(block) => route_word!(block, interrupt_core1, source),
        }
    }
}

/// The one writer of one core's interrupt-matrix bank: esp-hal's routes of
/// that core, which still store the route words, beside the typed blocks
/// that read them back. It is created only by consuming the esp-hal owner.
pub struct EspHalMatrix {
    routes: CoreInterruptRoutes,
    words: Routes,
    /// The CLIC of the hart that runs this core's matrix owner.
    clic: Option<Clic>,
}

impl EspHalMatrix {
    /// The line state of `line` in this hart's CLIC.
    fn line(&self, line: u8) -> Option<LineState> {
        let clic = self.clic.as_ref()?;
        let word = clic.read_ctrl(Index::new(usize::from(line)).ok()?);
        Some(LineState {
            enabled: word.ie(),
            vectored: word.attr_shv(),
            trigger: word.attr_trig().get().to_le_bytes()[0],
            control: word.ctl(),
        })
    }

    /// Check every present entry of the current core against the lines:
    /// its level's line, and its route when the matrix block declares it.
    fn check_lines(&self, table: &Table<Self>) -> Result<(), VerifyError> {
        let current = self.current_core();
        for binding in table
            .iter()
            .filter(|binding| binding.handler.is_some() && binding.core == current)
        {
            let level = binding.level as u8;
            let line = |error| VerifyError::Line {
                source: binding.source,
                error,
            };
            if level == 0 || level > lines::LEVELS {
                return Err(line(LineError::NoSuchLevel { level }));
            }
            let state = self
                .line(lines::line_of(level))
                .ok_or(VerifyError::NoLineController { core: current })?;
            lines::check_line(level, state).map_err(line)?;
            match self.words.word(binding.source) {
                Some(RouteWord::Line(routed)) => lines::check_route(level, routed).map_err(line)?,
                // Its route is the radio view's word, which the generic
                // check reads through esp-hal.
                Some(RouteWord::Elsewhere) => {}
                None => return Err(line(LineError::NoRouteWord)),
            }
        }
        Ok(())
    }
}

// Boot and task-side routing borrow each bank's one owner. Interrupt
// handlers never route sources and never reach these cells. Each hart has
// its own bank, so simultaneous startup on two cores cannot contend.
static MATRICES: [oer_memory::HandlerCell<EspHalMatrix>; 2] =
    [const { oer_memory::HandlerCell::new() }; 2];
static STORAGE: [static_cell::StaticCell<EspHalMatrix>; 2] =
    [const { static_cell::StaticCell::new() }; 2];

/// The pending bits of the 32 sources from `32 * $word` in `$status`, a
/// typed pending block; zero past the last source.
macro_rules! pending_word {
    ($status:expr, $word:expr) => {{
        let status = $status;
        #[cfg(feature = "esp32s31")]
        let word = match $word {
            0 => status.read_int_status_0().sources(),
            1 => status.read_int_status_1().sources(),
            2 => status.read_int_status_2().sources(),
            3 => status.read_int_status_3().sources(),
            4 => status.read_int_status_4().sources(),
            5 => status.read_int_status_5().sources().get(),
            _ => 0,
        };
        #[cfg(feature = "esp32c5")]
        let word = match $word {
            0 => status.read_int_status_0().sources(),
            1 => status.read_int_status_1().sources(),
            2 => status.read_int_status_2().sources().get(),
            _ => 0,
        };
        word
    }};
}

/// Each core's pending sources, which every context polls.
static PENDING0: ZeroedOnce<InterruptCore0Status> = ZeroedOnce::new();
#[cfg(feature = "esp32s31")]
static PENDING1: ZeroedOnce<InterruptCore1Status> = ZeroedOnce::new();

fn with_matrix<R>(f: impl FnOnce(&mut EspHalMatrix) -> R) -> R {
    let matrix = match Cpu::current() {
        Cpu::ProCpu => &MATRICES[0],
        #[cfg(feature = "esp32s31")]
        Cpu::AppCpu => &MATRICES[1],
    };
    #[expect(
        clippy::expect_used,
        reason = "routing requires one-time adoption and cannot reenter its own core's owner"
    )]
    matrix
        .with(f)
        .expect("the core's matrix owner is absent or already borrowed")
}

/// The levels esp-hal's `Priority` holds, by level.
const PRIORITIES: [Priority; lines::LEVELS as usize] = [
    Priority::Priority1,
    Priority::Priority2,
    Priority::Priority3,
    Priority::Priority4,
    Priority::Priority5,
    Priority::Priority6,
    Priority::Priority7,
    Priority::Priority8,
];

impl Matrix for EspHalMatrix {
    type Source = Interrupt;
    type Level = Priority;
    type Core = Cpu;

    fn current_core(&self) -> Cpu {
        Cpu::current()
    }

    fn route(&mut self, source: Interrupt, level: Priority) {
        self.routes.enable(source, level);
    }

    fn silence(&mut self, core: Cpu, source: Interrupt) {
        assert_eq!(
            core,
            self.routes.core(),
            "the matrix owner belongs to another core"
        );
        self.routes.disable(source);
    }

    /// The level of the line the source is routed to, from the typed route
    /// word and the line's control byte; `None` for a silent source and
    /// for a line that holds no level, which the line check refuses.
    fn routed(&self, core: Cpu, source: Interrupt) -> Option<Priority> {
        assert_eq!(
            core,
            self.routes.core(),
            "the matrix owner belongs to another core"
        );
        match self.words.word(source)? {
            RouteWord::Line(0) => None,
            RouteWord::Line(line) => {
                let level = lines::level_of_control(self.line(line)?.control)?;
                PRIORITIES.get(usize::from(level).checked_sub(1)?).copied()
            }
            // The radio view owns the word; esp-hal reads it until the table
            // writes the routes.
            RouteWord::Elsewhere => {
                esp_hal::interrupt::mapped_to(core, source).map(|line| line.priority())
            }
        }
    }

    fn slot(&self, source: Interrupt) -> usize {
        esp_hal::interrupt::bound_handler(source).map_or(0, |handler| handler.address())
    }
}

// The stack analysis reads the table from the image by these offsets: a
// change of esp-hal's or the PAC's representation of a field must fail here.
const _: () = {
    use core::mem::{align_of, offset_of, size_of};
    type Entry = Binding<Interrupt, Priority, Cpu>;
    assert!(size_of::<Interrupt>() == 2 && size_of::<Priority>() == 1);
    assert!(size_of::<Cpu>() == 4);
    assert!(offset_of!(Entry, source) == 0);
    assert!(offset_of!(Entry, level) == 2);
    assert!(offset_of!(Entry, class) == 3 && size_of::<oer_interrupt_table::Class>() == 1);
    assert!(offset_of!(Entry, core) == 4);
    assert!(offset_of!(Entry, handler) == 8);
    assert!(size_of::<Entry>() == 12 && align_of::<Entry>() == 4);
    assert!(Cpu::ProCpu as u32 == 0);
    #[cfg(feature = "esp32s31")]
    assert!(Cpu::AppCpu as u32 == 1);
    let mut level = 0;
    while level < PRIORITIES.len() {
        assert!(PRIORITIES[level] as usize == level + 1);
        level += 1;
    }
};

/// A table error of the chip's matrix.
pub type Error = MatrixError<EspHalMatrix>;

/// How the matrix, the CPU lines or esp-hal's drivers disagree with the
/// image's table at the start: a typed boot failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerifyError {
    /// The matrix disagrees with the table.
    Table(Error),
    /// A bound source's line or route disagrees with its entry.
    Line { source: Interrupt, error: LineError },
    /// The core has entries but no owner of its hart's CLIC.
    NoLineController { core: Cpu },
    /// An esp-hal driver's source has no entry or, on its table core, no
    /// route.
    Required(Error),
    /// esp-hal required a source number its PAC lacks.
    UnknownSource { number: u16 },
}

/// The image's table, which [`adopt`] stores once.
static TABLE: Adopted<Binding<Interrupt, Priority, Cpu>> = Adopted::new();

/// Keep the image's `INTERRUPT_TABLE` for every hart and claim the
/// interrupt matrix and the CLIC; the staged runtime calls this once, after
/// esp-hal's vectoring and before any hart installs its interrupt stack.
///
/// # Panics
///
/// On a second call: the image has one table, which alone holds esp-hal's
/// routing capability.
pub fn adopt(table: &'static Table<EspHalMatrix>) {
    if TABLE.adopt(table).is_err() {
        panic!("the image's interrupt table is adopted twice");
    }
    let routes = InterruptRoutes::take()
        .unwrap_or_else(|| panic!("esp-hal's interrupt routes are taken outside the table"));
    #[allow(
        unsafe_code,
        reason = "the taken routes prove the interrupt matrix handoff"
    )]
    // SAFETY: `InterruptRoutes::take` returned esp-hal's one routing
    // capability, which no other code now holds: each core's half moves
    // into that core's matrix owner below, beside the typed blocks. The
    // staged runtime calls `adopt` once, after esp-hal's vectoring of this
    // boot, so esp-hal writes the route and line words only as
    // `registers/shared-words.toml` reviews.
    let handoff = unsafe { InterruptMatrixHandoff::assume_exclusive() };
    let blocks = InterruptMatrixPeripherals::from_handoff(handoff);
    let _ = PENDING0.get_or_init(|| blocks.interrupt_core0_status);
    #[cfg(feature = "esp32s31")]
    let _ = PENDING1.get_or_init(|| blocks.interrupt_core1_status);
    let (core0, core1) = routes.split();
    let core0 = EspHalMatrix {
        routes: core0,
        words: Routes::Core0(blocks.interrupt_core0),
        clic: Some(blocks.clic),
    };
    #[cfg(feature = "esp32s31")]
    let core1 = core1.map(|routes| EspHalMatrix {
        routes,
        words: Routes::Core1(blocks.interrupt_core1),
        // The CLIC block is the accessing hart's own; the second hart runs
        // no entries yet and owns none.
        clic: None,
    });
    #[cfg(not(feature = "esp32s31"))]
    let core1: Option<EspHalMatrix> =
        core1.map(|_| unreachable!("a single-core chip has no second bank"));
    for ((storage, matrix), owner) in STORAGE.iter().zip(&MATRICES).zip([Some(core0), core1]) {
        if let Some(owner) = owner {
            assert!(
                matrix.install(storage.init(owner)).is_ok(),
                "the core's interrupt matrix was adopted twice"
            );
        }
    }
}

/// The image's table.
///
/// # Panics
///
/// Before [`adopt`]: no core reaches its sources without the table.
pub fn table() -> &'static Table<EspHalMatrix> {
    TABLE
        .get()
        .unwrap_or_else(|| panic!("the image's interrupt table is not adopted"))
}

/// Whether `source` is pending at the current core's interrupt matrix.
/// Before [`adopt`] no source is routed, so none is reported pending. Never
/// panics.
#[inline(always)]
pub fn is_pending(source: Interrupt) -> bool {
    let source = source as usize;
    let (word, bit) = (source / 32, source % 32);
    let sources = match Cpu::current() {
        Cpu::ProCpu => PENDING0
            .get()
            .map_or(0, |status| pending_word!(status, word)),
        #[cfg(feature = "esp32s31")]
        Cpu::AppCpu => PENDING1
            .get()
            .map_or(0, |status| pending_word!(status, word)),
    };
    sources
        .checked_shr(bit as u32)
        .is_some_and(|sources| sources & 1 != 0)
}

/// Route the token's source to its table level; on its table core only.
///
/// # Errors
///
/// [`oer_interrupt_table::Error::WrongCore`] on another core.
///
/// # Panics
///
/// Before [`adopt`].
pub fn enable<E>(token: &E) -> Result<(), Error>
where
    E: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
{
    with_matrix(|matrix| oer_interrupt_table::enable(matrix, table(), token))
}

/// Silence the token's source on its table core.
pub fn disable<E>(token: &E)
where
    E: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
{
    with_matrix(|matrix| oer_interrupt_table::disable(matrix, token));
}

/// A source's token with its type erased ([`oer_interrupt_table::Route`]).
pub type Route = oer_interrupt_table::MatrixRoute<EspHalMatrix>;

/// [`enable`] by a route.
///
/// # Errors
///
/// As [`enable`].
///
/// # Panics
///
/// Before [`adopt`].
pub fn enable_route(route: &Route) -> Result<(), Error> {
    with_matrix(|matrix| oer_interrupt_table::enable_route(matrix, table(), route))
}

/// [`disable`] by a route.
pub fn disable_route(route: &Route) {
    with_matrix(|matrix| oer_interrupt_table::disable_route(matrix, route));
}

/// Silence the current hart's sources and check every vector slot.
///
/// # Panics
///
/// Before [`adopt`], or when the matrix disagrees with the table.
pub fn install_current_hart() {
    if let Err(error) = with_matrix(|matrix| oer_interrupt_table::install(matrix, table())) {
        panic!("the interrupt matrix disagrees with the image's table: {error:?}");
    }
}

/// Check the current hart's sources, its CPU lines and every vector slot
/// against the table, and that each source an esp-hal driver required so
/// far is routed: every bound level's line is enabled, vectored,
/// level-triggered and holds that level, and every bound source is silent
/// or routed to its level's line.
///
/// # Errors
///
/// The first disagreement, a typed boot failure.
///
/// # Panics
///
/// Before [`adopt`].
pub fn verify_current_hart() -> Result<(), VerifyError> {
    with_matrix(|matrix| {
        oer_interrupt_table::verify(matrix, table()).map_err(VerifyError::Table)?;
        matrix.check_lines(table())
    })?;
    for number in esp_hal::interrupt::required_routes() {
        let source = u8::try_from(number)
            .ok()
            .and_then(|number| Interrupt::try_from(number).ok())
            .ok_or(VerifyError::UnknownSource { number })?;
        with_matrix(|matrix| oer_interrupt_table::verify_required(matrix, table(), [source]))
            .map_err(VerifyError::Required)?;
    }
    Ok(())
}
