#![cfg_attr(not(test), no_std)]
#![cfg_attr(
    not(target_arch = "riscv32"),
    allow(
        dead_code,
        reason = "the host builds the chip-independent parts for their tests"
    )
)]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
//! Stage-two entry, relocation, traps and interrupt-stack ownership of the
//! Espressif staged boot.
//!
//! The bootstrap copies stage two into PSRAM and jumps to `_runtime_start`
//! ([`entry`](crate::entry)), which initializes the SRAM sections, switches to
//! the CPU0 PSRAM task stack, installs the runtime's vectors and enters the
//! image's `runtime_main` that [`entry!`] defines. The image's `main` adopts
//! the bootstrap's PSRAM mapping through its board and takes its hart's
//! routes and lines ([`Handoff::adopt`]), binds its interrupts through the
//! [`Router`](oer_interrupt_table::Router) it gets back and enables
//! interrupts ([`Adopted::enable_interrupts`]). A chip feature selects the
//! chip, and the build generates the vector tables, every hart's line
//! entries, each line's default handler and the interrupt table's lines
//! from its facts (`oer-espressif-facts`): its harts and CLIC lines.
//!
//! The runtime owns the traps ([`stacks`]: the vector tables, every entry
//! and the checked exit; the common entry and exit are `vectors.s`), the
//! image's `critical-section` (`critical`) and
//! the panic record ([`panic`]); the interrupt table's router writes the
//! routes and the CLIC lines through the chip's controller.
#[allow(unsafe_code, reason = "the CLIC threshold and mstatus.MIE")]
mod critical;
#[cfg(target_arch = "riscv32")]
#[allow(
    unsafe_code,
    reason = "exported default trap handlers and the abort loop"
)]
mod default_handlers;
pub mod diagnostics;
#[cfg(target_arch = "riscv32")]
#[allow(unsafe_code, reason = "the stage-two reset entry is assembly")]
mod entry;
pub mod interrupts;
/// The chip's CPU lines, harts and vector-table words, which the build
/// generates from its facts.
#[cfg(any(feature = "esp32s31", feature = "esp32c5"))]
mod vectors {
    include!(concat!(env!("OUT_DIR"), "/vectors.rs"));
}
#[cfg(target_arch = "riscv32")]
pub mod layout;
#[allow(
    unsafe_code,
    reason = "the panic record lives in a fixed SRAM section the next boot reads"
)]
pub mod panic;
pub mod placement;
#[allow(
    unsafe_code,
    reason = "per-hart interrupt stacks and the trap entries switching onto them"
)]
pub mod stacks;
#[allow(unsafe_code, reason = "trap entry and machine CSRs")]
pub mod trap;

#[doc(hidden)]
pub use oer_interrupt_table as __interrupt_table;

/// The image's interrupt table as [`interrupt_table!`] declares it.
pub type InterruptTable = oer_interrupt_table::Table;

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
    /// stage-two interrupt context of CPU0: its SRAM interrupt stack and
    /// vectors, the task-stack watchpoint, and the hart's routes and lines,
    /// which `controller` writes. Interrupts stay disabled until
    /// [`Adopted::enable_interrupts`].
    ///
    /// `interrupt_table` is the image's `INTERRUPT_TABLE`
    /// ([`interrupt_table!`]). Installing it silences every source and
    /// disables every line of the hart, so from here a source is routed only
    /// by binding its entry.
    ///
    /// # Panics
    ///
    /// When `controller` belongs to another hart or does not take the
    /// silencing writes: a typed boot failure (`oer_interrupt_table::Error`).
    #[cfg(target_arch = "riscv32")]
    #[allow(
        unsafe_code,
        reason = "installs the interrupt stack, the vectors and the stack watchpoint"
    )]
    pub fn adopt<B: AdoptsBootstrapMapping, C: oer_interrupt_table::Controller>(
        self,
        interrupt_table: &'static InterruptTable,
        controller: C,
        board: B,
    ) -> (B::Mapping, Adopted<C>) {
        let mapping = board.adopt(&self.proof);
        assert!(
            usize::from(controller.hart()) == oer_espressif_staged_layout::boot::BOOT_HART,
            "the stage-two handoff runs on the boot hart"
        );
        // SAFETY: the handoff proves interrupts are disabled on the only
        // running hart, which has not admitted one.
        unsafe { stacks::install_current_hart_interrupt_stack() };
        let router = match oer_interrupt_table::Router::install(controller, interrupt_table) {
            Ok(router) => router,
            Err(error) => {
                panic!("the hart's routes and lines disagree with the image's table: {error:?}")
            }
        };
        unsafe extern "C" {
            static _stack_end: u8;
        }
        // SAFETY: interrupts are disabled and trigger 0 is the runtime's;
        // `_stack_end` is the word-aligned bottom of the CPU0 task stack the
        // runtime linker script places.
        unsafe { stacks::watch_task_stack(core::ptr::addr_of!(_stack_end) as usize) };
        (mapping, Adopted { router })
    }
}

/// The adopted stage-two interrupt context of CPU0, before interrupts are
/// enabled: the hart's router, through which the image binds its entries.
#[must_use = "interrupts stay disabled until `enable_interrupts`"]
pub struct Adopted<C: oer_interrupt_table::Controller> {
    router: oer_interrupt_table::Router<C>,
}

impl<C: oer_interrupt_table::Controller> Adopted<C> {
    /// The hart's router: bind entries before interrupts are enabled.
    pub fn router(&mut self) -> &mut oer_interrupt_table::Router<C> {
        &mut self.router
    }

    /// Check the hart's vectors, routes and lines and hand global interrupt
    /// enable (`mstatus.MIE`) to this hart's executor; the router stays the
    /// image's, for bindings made later.
    ///
    /// Safe at any point after [`Handoff::adopt`]: adoption silenced every
    /// source and disabled every line, and a source is routed again only by
    /// binding its entry, after its handler holds its part.
    ///
    /// # Panics
    /// When the hart dispatches through other vectors than the runtime's, or
    /// a route or line disagrees with the image's table: the typed boot
    /// failure `oer_interrupt_table::Error`.
    #[cfg(target_arch = "riscv32")]
    #[allow(unsafe_code, reason = "sets mstatus.MIE")]
    pub fn enable_interrupts(self) -> oer_interrupt_table::Router<C> {
        stacks::verify_current_hart_vectors();
        if let Err(error) = self.router.verify() {
            panic!("the hart's routes and lines disagree with the image's table: {error:?}");
        }
        // SAFETY: as above, no source is routed but by its binding; setting
        // MIE touches no memory and leaves the stack unchanged.
        unsafe {
            core::arch::asm!("csrsi mstatus, {mie}", mie = const oer_riscv::machine::status::MIE, options(nomem, nostack))
        };
        self.router
    }
}

/// Wait for interrupts forever: the rest of an image that has nothing left
/// to do on this hart.
#[cfg(target_arch = "riscv32")]
#[allow(unsafe_code, reason = "the wfi instruction")]
pub fn idle() -> ! {
    loop {
        // SAFETY: `wfi` touches no memory and only parks the hart until an
        // interrupt; with interrupts disabled it parks it for good.
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}

/// Mask this hart's interrupts (`mstatus.MIE`) and wait for good: the end
/// of a fatal path. It takes no lock and touches no other hart, which keeps
/// running.
#[cfg(target_arch = "riscv32")]
#[allow(unsafe_code, reason = "the clear of mstatus.MIE")]
pub fn halt() -> ! {
    // SAFETY: clearing MIE touches no memory and leaves the stack unchanged;
    // afterwards no handler runs on this hart.
    unsafe {
        core::arch::asm!("csrci mstatus, {mie}", mie = const oer_riscv::machine::status::MIE, options(nomem, nostack))
    };
    idle()
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
