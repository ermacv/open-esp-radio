//! What the executor takes from the chip, in one place. Ours, on the typed
//! platform blocks: the software interrupt lines that pend and wake
//! executors (`FROM_CPU_0..3`), the system timer's counter
//! (`SYSTIMER_COUNTER`) and its comparators ([`crate::systimer`]), the
//! pending sources (the interrupt table's matrix) and the current interrupt
//! level ([`interrupt_level`]). Still esp-hal's: the wait for an interrupt
//! (which skips `wfi` while a debugger is attached), the current core and the
//! interrupt table's source, level and core types.

pub(crate) use esp_hal::{
    interrupt::{Priority, software::SoftwareInterrupt, wait_for_interrupt},
    peripherals::{Interrupt, SYSTIMER},
    system::Cpu,
};
#[cfg(feature = "esp32c5")]
pub(crate) use oer_esp32c5_hal::platform;
#[cfg(feature = "esp32s31")]
pub(crate) use oer_esp32s31_hal::platform;
use oer_memory::zeroed::ZeroedOnce;

use crate::{
    counter::CounterLatch,
    software_line::{SoftwareLine, serve},
};

/// The software interrupts an executor can own.
pub(crate) const SOFTWARE_INTERRUPT_COUNT: usize = crate::task::EXECUTORS as usize;

/// The system timer's ticks per microsecond: it counts at 16 MHz on both
/// chips.
///
/// SOURCE: ESP-IDF 4d59230d `esp_hw_support/port/esp32s31/systimer.c` and
/// `esp_hw_support/port/esp32c5/systimer.c` (`systimer_ticks_to_us`:
/// `ticks / 16`).
pub(crate) const TICKS_PER_MICROSECOND: u64 = 16;

/// The core running the caller.
#[inline(always)]
pub(crate) fn current_core() -> usize {
    Cpu::current() as usize
}

/// The interrupt level the caller runs at: the CLIC's `mintstatus.mil`,
/// which the hart raises on a trap's entry to the taken interrupt's level
/// and restores on its return; 0 in thread mode. Both chips' cores take
/// interrupts through a CLIC.
#[inline(always)]
pub(crate) fn interrupt_level() -> u8 {
    let status: usize;
    #[allow(unsafe_code, reason = "a CSR read is an instruction")]
    // SAFETY: `csrr` of `mintstatus`, a read-only CSR, writes only the output
    // register; it touches no memory and no other state.
    unsafe {
        core::arch::asm!(
            "csrr {status}, {mintstatus}",
            status = out(reg) status,
            mintstatus = const oer_riscv::clic::MINTSTATUS,
            options(nomem, nostack, preserves_flags),
        );
    }
    // `mil` is the top byte: the truncation keeps exactly the field.
    (status >> oer_riscv::clic::MINTSTATUS_MIL_SHIFT) as u8
}

impl CounterLatch for platform::SystimerCounter {
    #[inline(always)]
    fn update(&self) {
        self.trigger_unit0_op_update();
    }
    #[inline(always)]
    fn valid(&self) -> bool {
        self.read_unit0_op().value_valid()
    }
    #[inline(always)]
    fn high(&self) -> u32 {
        self.read_unit0_value_hi().value_hi().get()
    }
    #[inline(always)]
    fn low(&self) -> u32 {
        self.read_unit0_value_lo().value_lo()
    }
}

/// The shared counter latch every context reads, published once by the time
/// driver's handoff.
static COUNTER: ZeroedOnce<platform::SystimerCounter> = ZeroedOnce::new();

/// Publish the counter latch, from the system timer's handoff.
///
/// # Panics
/// When it is published already.
pub(crate) fn install_counter(counter: platform::SystimerCounter) {
    assert!(COUNTER.get().is_none(), "the counter is published once");
    let _ = COUNTER.get_or_init(|| counter);
}

/// The system timer's counter, in microseconds.
///
/// # Panics
/// Before the time driver's [`init`](crate::init), or when the latch never
/// settles (a fault of the system timer).
// Share the same clock read with cooperative admission and poll stamping.
#[inline(never)]
pub(crate) fn now() -> u64 {
    let counter = COUNTER
        .get()
        .unwrap_or_else(|| panic!("the time driver publishes the counter before time is read"));
    let ticks = crate::counter::read(counter)
        .unwrap_or_else(|_| panic!("the system timer's counter latch settles"));
    ticks / TICKS_PER_MICROSECOND
}

/// Whether `source` is pending at the current core's interrupt matrix.
#[inline(always)]
pub(crate) fn is_pending(source: Interrupt) -> bool {
    oer_espressif_esp_hal_irq::is_pending(source)
}

/// The peripheral interrupt source of software interrupt `SWI`.
pub(crate) const fn software_interrupt_source(swi: u8) -> Interrupt {
    match swi {
        0 => Interrupt::FROM_CPU_INTR0,
        1 => Interrupt::FROM_CPU_INTR1,
        2 => Interrupt::FROM_CPU_INTR2,
        _ => Interrupt::FROM_CPU_INTR3,
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

/// Store `$value` into the line block `$line`; a raise on the ESP32-S31
/// reads the line back, so the raise has reached the interrupt matrix when
/// it returns.
///
/// SOURCE: esp-hal 1f646c5 `interrupt/software.rs` (`raise`: the S31 store
/// and its read back; the C5 store needs neither a critical section nor a
/// wait, as ESP-IDF 4d59230d `hal/esp32c5/include/hal/crosscore_int_ll.h`
/// stores alone).
macro_rules! store_line {
    ($line:expr, $value:expr) => {{
        let line = $line;
        line.store_line($value);
        #[cfg(feature = "esp32s31")]
        if $value {
            let _ = line.read_line();
        }
    }};
}

impl SoftwareLine for Line {
    #[inline(always)]
    fn raise(&self) {
        match self {
            Self::Zero(line) => store_line!(line, true),
            Self::One(line) => store_line!(line, true),
            Self::Two(line) => store_line!(line, true),
            Self::Three(line) => store_line!(line, true),
        }
    }
    #[inline(always)]
    fn lower(&self) {
        match self {
            Self::Zero(line) => store_line!(line, false),
            Self::One(line) => store_line!(line, false),
            Self::Two(line) => store_line!(line, false),
            Self::Three(line) => store_line!(line, false),
        }
    }
}

macro_rules! take_line {
    ($number:literal, $variant:ident, $block:ident, $take:ident, $handoff:ident, $partition:ident, $member:ident, $storage:ident) => {
        static $storage: ZeroedOnce<platform::$block> = ZeroedOnce::new();

        /// Consume esp-hal's software interrupt and claim its line's block.
        pub(crate) fn $take(interrupt: SoftwareInterrupt<'static, $number>) -> Line {
            let _consumed = interrupt;
            #[allow(
                unsafe_code,
                reason = "the consumed esp-hal line proves the line handoff"
            )]
            // SAFETY: esp-hal makes `SoftwareInterrupt<N>` once and its owner
            // consumed it here for good, so no esp-hal raise or reset of the
            // line remains; this is the line's only handoff.
            let handoff = unsafe { platform::$handoff::assume_exclusive() };
            let block = platform::$partition::from_handoff(handoff).$member;
            Line::$variant($storage.get_or_init(|| block))
        }
    };
}

take_line!(
    0,
    Zero,
    FromCpu0,
    take_line_0,
    SoftwareInterrupt0Handoff,
    SoftwareInterrupt0Peripherals,
    from_cpu_0,
    LINE0
);
take_line!(
    1,
    One,
    FromCpu1,
    take_line_1,
    SoftwareInterrupt1Handoff,
    SoftwareInterrupt1Peripherals,
    from_cpu_1,
    LINE1
);
take_line!(
    2,
    Two,
    FromCpu2,
    take_line_2,
    SoftwareInterrupt2Handoff,
    SoftwareInterrupt2Peripherals,
    from_cpu_2,
    LINE2
);
take_line!(
    3,
    Three,
    FromCpu3,
    take_line_3,
    SoftwareInterrupt3Handoff,
    SoftwareInterrupt3Peripherals,
    from_cpu_3,
    LINE3
);

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

fn line(swi: u8) -> &'static Line {
    LINES
        .get(usize::from(swi))
        .and_then(ZeroedOnce::get)
        .unwrap_or_else(|| panic!("software interrupt line is installed before use"))
}

/// Raise software interrupt `swi`.
#[inline(always)]
pub(crate) fn raise_interrupt(swi: u8) {
    line(swi).raise();
}

/// One handler round of software interrupt `swi`: lower its line, then take
/// the work (`drain`), so a raise during the work runs another round.
#[inline(always)]
pub(crate) fn serve_interrupt<R>(swi: u8, drain: impl FnOnce() -> R) -> R {
    serve(line(swi), drain)
}
