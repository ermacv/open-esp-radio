use esp_hal::{
    interrupt::{InterruptRoutes, Priority},
    peripherals::Interrupt,
    system::Cpu,
};
use oer_interrupt_table::{Adopted, Binding, Entry, Matrix, MatrixError, Table};

/// The chip's interrupt matrix, as esp-hal drives it.
pub struct EspHalMatrix;

/// esp-hal's routing capability, which [`adopt`] takes once.
static ROUTES: Adopted<InterruptRoutes> = Adopted::new();

fn routes() -> &'static InterruptRoutes {
    match ROUTES.get() {
        Some([routes]) => routes,
        _ => panic!("the image's interrupt table is not adopted"),
    }
}

impl Matrix for EspHalMatrix {
    type Source = Interrupt;
    type Level = Priority;
    type Core = Cpu;

    fn current_core(&self) -> Cpu {
        Cpu::current()
    }

    fn route(&mut self, source: Interrupt, level: Priority) {
        routes().enable(Cpu::current(), source, level);
    }

    fn silence(&mut self, core: Cpu, source: Interrupt) {
        routes().disable(core, source);
    }

    fn routed(&self, core: Cpu, source: Interrupt) -> Option<Priority> {
        esp_hal::interrupt::mapped_to(core, source).map(|line| line.priority())
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
    assert!(offset_of!(Entry, core) == 4);
    assert!(offset_of!(Entry, handler) == 8);
    assert!(size_of::<Entry>() == 12 && align_of::<Entry>() == 4);
    assert!(Cpu::ProCpu as u32 == 0);
    #[cfg(feature = "esp32s31")]
    assert!(Cpu::AppCpu as u32 == 1);
    assert!(Priority::Priority1 as u8 == 1);
};

/// A table error of the chip's matrix.
pub type Error = MatrixError<EspHalMatrix>;

/// The image's table, which [`adopt`] stores once.
static TABLE: Adopted<Binding<Interrupt, Priority, Cpu>> = Adopted::new();

/// Keep the image's `INTERRUPT_TABLE` for every hart; the staged runtime
/// calls this once, before any hart installs its interrupt stack.
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
    // The table is adopted once, so its routes are too.
    let _ = ROUTES.adopt(core::slice::from_ref(routes));
}

/// The image's interrupt table.
///
/// # Panics
///
/// Before [`adopt`]: no core reaches its sources without the table.
pub fn table() -> &'static Table<EspHalMatrix> {
    TABLE
        .get()
        .unwrap_or_else(|| panic!("the image's interrupt table is not adopted"))
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
    oer_interrupt_table::enable(&mut EspHalMatrix, table(), token)
}

/// Silence the token's source on its table core.
pub fn disable<E>(token: &E)
where
    E: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
{
    oer_interrupt_table::disable(&mut EspHalMatrix, token);
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
    oer_interrupt_table::enable_route(&mut EspHalMatrix, table(), route)
}

/// [`disable`] by a route.
pub fn disable_route(route: &Route) {
    oer_interrupt_table::disable_route(&mut EspHalMatrix, route);
}

/// Silence the current hart's sources and check every vector slot.
///
/// # Panics
///
/// Before [`adopt`], or when the matrix disagrees with the table.
pub fn install_current_hart() {
    if let Err(error) = oer_interrupt_table::install(&mut EspHalMatrix, table()) {
        panic!("the interrupt matrix disagrees with the image's table: {error:?}");
    }
}

/// Check the current hart's sources and every vector slot against the table,
/// and that each source an esp-hal driver required so far is routed.
///
/// # Panics
///
/// Before [`adopt`], when the matrix disagrees with the table, or when a
/// driver's required source has no entry or, on its table core, no route.
pub fn verify_current_hart() {
    if let Err(error) = oer_interrupt_table::verify(&EspHalMatrix, table()) {
        panic!("the interrupt matrix disagrees with the image's table: {error:?}");
    }
    let required = esp_hal::interrupt::required_routes().map(|number| {
        u8::try_from(number)
            .ok()
            .and_then(|number| Interrupt::try_from(number).ok())
            .unwrap_or_else(|| panic!("esp-hal required source {number}, which the PAC lacks"))
    });
    if let Err(error) = oer_interrupt_table::verify_required(&EspHalMatrix, table(), required) {
        panic!("an esp-hal driver's source is not routed by the image's table: {error:?}");
    }
}
