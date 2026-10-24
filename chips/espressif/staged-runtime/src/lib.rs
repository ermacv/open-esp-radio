#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
//! Stage-two entry, relocation and interrupt-stack ownership of the
//! Espressif staged boot.
//!
//! The bootstrap copies stage two into PSRAM and jumps to `_runtime_start`
//! ([`entry`](crate::entry)), which initializes the SRAM sections, switches to
//! the CPU0 PSRAM task stack, installs the vectors and enters the image's
//! `runtime_main` that [`entry!`] defines. The image's `main` adopts the
//! bootstrap's PSRAM mapping through its board ([`Handoff::adopt`]) and
//! enables interrupts ([`Adopted::enable_interrupts`]). A chip feature selects the chip of esp-hal; `two-harts` adds the
//! second hart's interrupt stack.
#[allow(
    unsafe_code,
    reason = "exported default trap handlers and the abort loop"
)]
mod default_handlers;
pub mod diagnostics;
#[allow(unsafe_code, reason = "the stage-two reset entry is assembly")]
mod entry;
pub mod interrupts;
pub mod layout;
#[allow(
    unsafe_code,
    reason = "the panic record lives in a fixed SRAM section the next boot reads"
)]
pub mod panic;
pub mod placement;
#[allow(
    unsafe_code,
    reason = "per-hart interrupt stacks and the vector table switching onto them"
)]
pub mod stacks;
#[allow(unsafe_code, reason = "trap entry and machine CSRs")]
pub mod trap;

#[doc(hidden)]
pub use oer_interrupt_table as __interrupt_table;

/// The image's interrupt table as [`interrupt_table!`] declares it.
pub type InterruptTable = oer_interrupt_table::Table<oer_espressif_esp_hal_irq::EspHalMatrix>;

pub use oer_espressif_staged_layout::handoff::AdoptsBootstrapMapping;

#[doc(hidden)]
pub use oer_memory as __memory;

/// The image's control at the runtime's entry: stage two runs on CPU0 right
/// after `_runtime_start`, with interrupts disabled and the bootstrap's
/// mapping intact. [`entry!`] gives it once, to the image's `main`.
#[must_use = "the handoff adopts the bootstrap's mapping"]
pub struct Handoff {
    proof: oer_espressif_staged_layout::handoff::StageTwoHandoff,
}

impl Handoff {
    #[doc(hidden)]
    /// # Safety
    /// Only [`entry!`]'s `runtime_main` calls it, once, before anything else.
    #[allow(unsafe_code, reason = "forwards the stage-two handoff proof")]
    pub const unsafe fn __new() -> Self {
        Self {
            // SAFETY: forwarded unchanged from this function's contract,
            // which is the proof's.
            proof: unsafe { oer_espressif_staged_layout::handoff::StageTwoHandoff::new() },
        }
    }

    /// Adopt the bootstrap's PSRAM mapping through `board` and install the
    /// stage-two interrupt context: the image's interrupt table, esp-hal's
    /// vectoring and the current hart's SRAM interrupt stack, and the
    /// task-stack watchpoint. Interrupts stay disabled until
    /// [`Adopted::enable_interrupts`].
    ///
    /// `interrupt_table` is the image's `INTERRUPT_TABLE` ([`interrupt_table!`]):
    /// every hart installs and checks it. Installing it silences every source
    /// it lists, so from here a source is routed only by its token's owner.
    #[allow(
        unsafe_code,
        reason = "installs vectoring, the interrupt stack and the stack watchpoint"
    )]
    pub fn adopt<B: AdoptsBootstrapMapping>(
        self,
        interrupt_table: &'static InterruptTable,
        board: B,
    ) -> (B::Mapping, Adopted) {
        oer_espressif_esp_hal_irq::adopt(interrupt_table);
        let mapping = board.adopt(&self.proof);
        // SAFETY: the handoff proves interrupts are disabled on the only
        // running hart, and no driver relies on the bootloader's interrupt
        // mappings yet.
        unsafe { esp_hal::interrupt::reinitialize_vectoring_after_handoff() };
        // SAFETY: interrupts are disabled and this hart has not admitted one.
        unsafe { stacks::install_current_hart_interrupt_stack() };
        unsafe extern "C" {
            static _stack_end: u8;
        }
        // SAFETY: `_stack_end` is the word-aligned bottom of the CPU0 task
        // stack the runtime linker script places.
        unsafe {
            esp_hal::debugger::set_stack_watchpoint(core::ptr::addr_of!(_stack_end) as usize)
        };
        (mapping, Adopted { _once: () })
    }
}

/// The adopted stage-two interrupt context of CPU0, before interrupts are
/// enabled.
#[must_use = "interrupts stay disabled until `enable_interrupts`"]
pub struct Adopted {
    _once: (),
}

impl Adopted {
    /// Hand global interrupt enable (`mstatus.MIE`) to this hart's executor.
    ///
    /// Safe at any point after [`Handoff::adopt`]: adoption silenced every
    /// source of the image's table and esp-hal's vectoring reset unrouted
    /// every peripheral source, and a source is routed again only through its
    /// table token, by the owner that binds its handler first (the image
    /// check refuses any other writer of the interrupt matrix).
    ///
    /// # Panics
    /// When a hardware-vector slot of the hart's active MTVT is not the
    /// runtime's stack-switching entry, or the interrupt matrix disagrees with
    /// the image's interrupt table ([`interrupts`]).
    #[allow(unsafe_code, reason = "sets mstatus.MIE")]
    pub fn enable_interrupts(self) {
        stacks::verify_current_hart_vectors();
        oer_espressif_esp_hal_irq::verify_current_hart();
        // SAFETY: as above, no source is routed but by its bound owner;
        // setting MIE touches no memory and leaves the stack unchanged.
        unsafe {
            core::arch::asm!("csrsi mstatus, {mie}", mie = const oer_riscv::machine::status::MIE, options(nomem, nostack))
        };
    }
}

/// Wait for interrupts forever: the rest of an image that has nothing left
/// to do on this hart.
#[allow(unsafe_code, reason = "the wfi instruction")]
pub fn idle() -> ! {
    loop {
        // SAFETY: `wfi` touches no memory and only parks the hart until an
        // interrupt; with interrupts disabled it parks it for good.
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}

/// The image's entry: `runtime_main`, which `_runtime_start` calls once on
/// CPU0, gives `$main` the one [`Handoff`].
///
/// ```ignore
/// oer_espressif_staged_runtime::entry!(main);
/// fn main(handoff: oer_espressif_staged_runtime::Handoff) -> ! { … }
/// ```
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        #[unsafe(no_mangle)]
        extern "C" fn runtime_main() -> ! {
            // SAFETY: `_runtime_start` calls `runtime_main` once on CPU0,
            // after initializing the SRAM sections, with interrupts disabled
            // and the bootstrap's mapping intact; this is its first statement.
            let handoff = unsafe { $crate::Handoff::__new() };
            $main(handoff)
        }
    };
}

/// The stack-protector guard that `esp_hal::init` arms its watchpoint on.
/// esp-hal's `linkall.x` defines it; stage two links its own scripts, so the
/// runtime owns it, in SRAM critical data.
#[allow(
    unsafe_code,
    reason = "the stack-protector guard symbol esp-hal links against"
)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".critical.data.stack_guard")]
static mut __stack_chk_guard: u32 = 0xDEED_BAAD;
