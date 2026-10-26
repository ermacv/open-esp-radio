//! Chip access for the executor: owned HAL blocks, source identities from
//! the HAL's executor contract and architectural CSR instructions.

use crate::hardware::ExecutorPlatform;
use oer_esp32s31_hal::executor::Platform as Chip;
pub(crate) use oer_esp32s31_hal::platform;
use oer_memory::zeroed::ZeroedOnce;

use crate::software_line::{SoftwareLine, serve};
use platform::system_timer::counter;

/// The HAL's executor contract: its source identities and its pending and
/// debugger reads.
impl ExecutorPlatform for Chip {
    const SOFTWARE_SOURCES: [u16; 4] = Chip::SOFTWARE_SOURCES;
    const ALARM_SOURCES: [u16; 3] = Chip::ALARM_SOURCES;

    fn debugger_connected() -> bool {
        Chip::debugger_connected()
    }

    fn is_pending(source: u16) -> bool {
        Chip::is_pending(source)
    }
}

/// The software interrupts an executor can own.
pub(crate) const SOFTWARE_INTERRUPT_COUNT: usize = crate::task::EXECUTORS as usize;

/// The system timer's ticks per microsecond, the HAL's.
pub(crate) const TICKS_PER_MICROSECOND: u64 = platform::system_timer::TICKS_PER_MICROSECOND;

/// The core running the caller.
#[inline(always)]
pub(crate) fn current_core() -> usize {
    oer_riscv::hart_id()
}

/// The interrupt level the caller runs at: the CLIC's `mintstatus.mil`,
/// which the hart raises on a trap's entry to the taken interrupt's level
/// and restores on its return; 0 in thread mode. The core takes interrupts
/// through a CLIC.
#[inline(always)]
pub(crate) fn interrupt_level() -> u8 {
    let status = oer_riscv::read_mintstatus();
    // `mil` is the top byte: the truncation keeps exactly the field.
    (status >> oer_riscv::clic::MINTSTATUS_MIL_SHIFT) as u8
}

/// The shared counter latch every context reads, published once by the time
/// driver's handoff.
static COUNTER: ZeroedOnce<platform::SystimerCounter> = ZeroedOnce::new();

/// Publish the counter latch, from the system timer's handoff, once its
/// latch has settled within its bound ([`counter::check`]).
///
/// # Panics
/// When it is published already or its latch never settles (a fault of the
/// system timer): the boundary at which the clock's reads become infallible.
pub(crate) fn install_counter(counter: platform::SystimerCounter) {
    assert!(COUNTER.get().is_none(), "the counter is published once");
    assert!(
        counter::check(&counter).is_ok(),
        "the system timer's counter latch settles"
    );
    let _ = COUNTER.get_or_init(|| counter);
}

/// The system timer's counter, in microseconds: a verified read, or the
/// floor of a read that preempting strobes kept from verifying, which is no
/// smaller than any earlier reading and no larger than any later one.
/// Before the time driver's [`init`](crate::init) publishes the counter
/// the clock reads 0, which every later read exceeds. The read never panics.
// Share the same clock read with cooperative admission and poll stamping.
#[inline(never)]
pub(crate) fn now() -> u64 {
    COUNTER.get().map_or(0, |counter| {
        counter::read(counter).unwrap_or_else(|unverified| unverified.floor) / TICKS_PER_MICROSECOND
    })
}

/// Whether `source` is pending at the current core's interrupt matrix.
#[inline(always)]
pub(crate) fn is_pending(source: u16) -> bool {
    Chip::is_pending(source)
}

pub(crate) const fn software_interrupt_source(swi: u8) -> u16 {
    Chip::SOFTWARE_SOURCES[swi as usize]
}

pub(crate) const fn alarm_source(comparator: usize) -> u16 {
    Chip::ALARM_SOURCES[comparator]
}

pub(crate) fn wait_for_interrupt() {
    if !Chip::debugger_connected() {
        oer_riscv::wait_for_interrupt();
    }
}

/// One software interrupt line: its typed block, lent to every context.
#[derive(Clone, Copy)]
pub(crate) enum Line {
    Zero(&'static platform::FromCpu0),
    One(&'static platform::FromCpu1),
    Two(&'static platform::FromCpu2),
    Three(&'static platform::FromCpu3),
}

impl SoftwareLine for Line {
    #[inline(always)]
    fn raise(&self) {
        match self {
            Self::Zero(line) => line.raise(),
            Self::One(line) => line.raise(),
            Self::Two(line) => line.raise(),
            Self::Three(line) => line.raise(),
        }
    }
    #[inline(always)]
    fn lower(&self) {
        match self {
            Self::Zero(line) => line.lower(),
            Self::One(line) => line.lower(),
            Self::Two(line) => line.lower(),
            Self::Three(line) => line.lower(),
        }
    }
}

macro_rules! own_line {
    ($variant:ident, $block:ident, $take:ident, $storage:ident) => {
        static $storage: ZeroedOnce<platform::$block> = ZeroedOnce::new();

        /// Publish a line moved from the single reset root, once.
        pub(crate) fn $take(block: platform::$block) -> Line {
            assert!($storage.get().is_none(), "the line owner is published once");
            Line::$variant($storage.get_or_init(|| block))
        }
    };
}

own_line!(Zero, FromCpu0, own_line_0, LINE0);
own_line!(One, FromCpu1, own_line_1, LINE1);
own_line!(Two, FromCpu2, own_line_2, LINE2);
own_line!(Three, FromCpu3, own_line_3, LINE3);

// Each executor's line, published once for wakers; only the line's handler
// lowers it. Boot lowers the line before it is published.
static LINES: [ZeroedOnce<Line>; SOFTWARE_INTERRUPT_COUNT] =
    [const { ZeroedOnce::new() }; SOFTWARE_INTERRUPT_COUNT];

/// Install software interrupt `swi`'s line for its executor.
///
/// # Panics
/// When the line is installed already.
pub(crate) fn install_line(swi: usize, line: Line) {
    line.lower();
    let Some(slot) = LINES.get(swi) else {
        panic!("no such software interrupt");
    };
    assert!(
        slot.get().is_none(),
        "software interrupt owner is already installed"
    );
    let _ = slot.get_or_init(|| line);
}

/// Software interrupt `swi`'s installed line: none before its executor or
/// doorbell took it, when there is nothing it could wake.
fn line(swi: u8) -> Option<&'static Line> {
    LINES.get(usize::from(swi)).and_then(ZeroedOnce::get)
}

/// Raise software interrupt `swi`; a line no executor took yet has no
/// handler to run, so nothing is raised. Never panics.
#[inline(always)]
pub(crate) fn raise_interrupt(swi: u8) {
    if let Some(line) = line(swi) {
        line.raise();
    }
}

/// One handler round of software interrupt `swi`: lower its line, then take
/// the work (`drain`), so a raise during the work runs another round. Only
/// an installed line's handler runs, since only an installed line is raised
/// and routed. Never panics.
#[inline(always)]
pub(crate) fn serve_interrupt<R>(swi: u8, drain: impl FnOnce() -> R) -> R {
    match line(swi) {
        Some(line) => serve(line, drain),
        None => drain(),
    }
}
