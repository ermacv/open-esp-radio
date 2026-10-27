#![cfg_attr(not(test), no_std)]
#![cfg_attr(
    not(target_arch = "riscv32"),
    allow(
        dead_code,
        reason = "the host builds the chip-independent parts for their tests"
    )
)]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing
)]
//! Stage-two entry, relocation, traps and interrupt-stack ownership of the
//! Espressif staged boot.
//!
//! The bootstrap copies stage two into PSRAM and jumps to `_runtime_start`
//! ([`entry`](crate::entry)), which initializes the SRAM sections, switches to
//! the boot hart PSRAM task stack, installs the runtime's vectors and enters the
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
//! image's `critical-section` (`critical`), the panic record ([`panic`]) and
//! the fatal paths (`fatal`: the end of a hart and the chip's reset, which
//! lend the chip HAL's fatal-path proof); the interrupt table's router writes
//! the routes and the CLIC lines through the chip's controller.
#[allow(unsafe_code, reason = "the critical-section implementation contract")]
mod critical;
#[cfg(target_arch = "riscv32")]
#[allow(
    unsafe_code,
    reason = "the default trap handlers export linker-visible symbols"
)]
mod default_handlers;
pub mod diagnostics;
#[cfg(feature = "esp32s31")]
pub mod fatal;
/// Shared retained/reset path selected only by staged Wi-Fi images.
#[cfg(feature = "wifi-fault")]
pub mod wifi_fault;
#[cfg(all(target_arch = "riscv32", feature = "esp32s31"))]
pub use fatal::{fatal_halt, reset, reset_recording};
#[cfg(target_arch = "riscv32")]
#[allow(unsafe_code, reason = "the stage-two reset entry is assembly")]
mod entry;
pub mod interrupts;
/// The chip's CPU lines, harts and vector-table words, which the build
/// generates from its facts.
#[cfg(feature = "esp32s31")]
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
#[allow(unsafe_code, reason = "volatile reads of the installed vector table")]
pub mod trap;

#[doc(hidden)]
pub use oer_interrupt_table as __interrupt_table;

/// The image's interrupt table as [`interrupt_table!`] declares it.
pub type InterruptTable = oer_interrupt_table::Table;

pub use oer_espressif_staged_layout::handoff::AdoptsBootstrapMapping;

#[doc(hidden)]
pub use oer_memory as __memory;

#[doc(hidden)]
pub use oer_espressif_staged_layout::handoff::StageTwoHandoff as __StageTwoHandoff;

/// A refused stage-two interrupt installation or enable. The image reports
/// this through its existing boot-failure path, with interrupts disabled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootError {
    /// The controller belongs to another hart.
    WrongHart { actual: usize, expected: usize },
    /// The current hart has no runtime interrupt stack.
    NoInterruptStack { hart: usize },
    /// The linker-provided stack guard cannot be encoded as an aligned NAPOT region.
    InvalidStackGuard { bottom: usize, bytes: usize },
    /// Vector CSRs no longer name the runtime's checked entries.
    ForeignVectors,
    /// Controller geometry, configuration, routes or lines failed verification.
    InterruptController(oer_interrupt_table::Error),
}

impl BootError {
    /// The boot-failure reason carried by the image's diagnostic record.
    pub const fn name(self) -> &'static str {
        match self {
            Self::WrongHart { .. } => "interrupt-hart",
            Self::NoInterruptStack { .. } => "interrupt-stack",
            Self::InvalidStackGuard { .. } => "interrupt-stack-guard",
            Self::ForeignVectors => "interrupt-vectors",
            Self::InterruptController(oer_interrupt_table::Error::WrongControlBits { .. }) => {
                "interrupt-control-bits"
            }
            Self::InterruptController(oer_interrupt_table::Error::WrongInterruptCount {
                ..
            }) => "interrupt-count",
            Self::InterruptController(oer_interrupt_table::Error::WrongLevelBits { .. }) => {
                "interrupt-level-bits"
            }
            Self::InterruptController(_) => "interrupt-table",
        }
    }
}

impl From<oer_interrupt_table::Error> for BootError {
    fn from(error: oer_interrupt_table::Error) -> Self {
        Self::InterruptController(error)
    }
}

/// The image's control at the runtime's entry: stage two runs on the boot hart right
/// after `_runtime_start`, with interrupts disabled and the bootstrap's
/// mapping intact. [`entry!`] gives it once, to the image's `main`.
#[must_use = "the handoff adopts the bootstrap's mapping"]
pub struct Handoff {
    proof: oer_espressif_staged_layout::handoff::StageTwoHandoff,
}

impl Handoff {
    #[doc(hidden)]
    /// Wrap the proof minted by [`entry!`].
    pub const fn from_proof(proof: oer_espressif_staged_layout::handoff::StageTwoHandoff) -> Self {
        Self { proof }
    }

    /// Adopt the bootstrap's PSRAM mapping through `board` and install the
    /// stage-two interrupt context of the boot hart: its SRAM interrupt stack and
    /// vectors, the task-stack watchpoint, and the hart's routes and lines,
    /// which `controller` writes. Interrupts stay disabled until
    /// [`Adopted::enable_interrupts`].
    ///
    /// `interrupt_table` is the image's `INTERRUPT_TABLE`
    /// ([`interrupt_table!`]). Installing it silences every source and
    /// disables every line of the hart, so from here a source is routed only
    /// by binding its entry.
    ///
    /// # Errors
    ///
    /// A wrong hart, missing interrupt stack, invalid stack guard, or controller
    /// disagreement with its chip facts or the image's table returns [`BootError`].
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
    ) -> Result<(B::Mapping, Adopted<C>), BootError> {
        panic::report_previous_fault();
        let mapping = board.adopt(&self.proof);
        let actual = usize::from(controller.hart());
        let expected = oer_esp32s31_hal::platform::BOOT_HART.get();
        if actual != expected {
            return Err(BootError::WrongHart { actual, expected });
        }
        // SAFETY: the handoff proves interrupts are disabled on the only
        // running hart, which has not admitted one.
        unsafe { stacks::install_current_hart_interrupt_stack() }?;
        let router = oer_interrupt_table::Router::install(controller, interrupt_table)?;
        // Interrupts are disabled and trigger 0 is the runtime's;
        // the generated table names this checked hart's word-aligned task bottom.
        let hart = oer_esp32s31_hal::platform::BOOT_HART;
        stacks::watch_task_stack(hart.at(&layout::stack_symbols()).task.start)?;
        Ok((mapping, Adopted { router }))
    }
}

/// The adopted stage-two interrupt context of the boot hart, before interrupts are
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
    /// # Errors
    /// [`BootError`] when vector CSRs, controller geometry, configuration,
    /// routes or lines disagree. Global interrupt enable stays disabled.
    #[cfg(target_arch = "riscv32")]
    pub fn enable_interrupts(self) -> Result<oer_interrupt_table::Router<C>, BootError> {
        stacks::verify_current_hart_vectors()?;
        self.router.verify()?;
        oer_riscv::enable_interrupts();
        Ok(self.router)
    }
}

/// Wait for interrupts forever: the rest of an image that has nothing left
/// to do on this hart.
#[cfg(target_arch = "riscv32")]
pub fn idle() -> ! {
    loop {
        oer_riscv::wait_for_interrupt();
    }
}

/// Mask this hart's interrupts (`mstatus.MIE`) and wait for good: the end
/// of a fatal path. It takes no lock and touches no other hart, which keeps
/// running.
#[cfg(target_arch = "riscv32")]
pub fn halt() -> ! {
    oer_riscv::disable_interrupts();
    idle()
}

/// The image's entry: `runtime_main`, which `_runtime_start` calls once on
/// the boot hart, gives `$main` the one [`Handoff`].
///
/// ```ignore
/// oer_espressif_staged_runtime::entry!(main);
/// fn main(handoff: oer_espressif_staged_runtime::Handoff) -> ! { … }
/// ```
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        // SAFETY: `_runtime_start` calls this one image entry with the C ABI;
        // defining a second `runtime_main` fails the link.
        #[unsafe(no_mangle)]
        extern "C" fn runtime_main() -> ! {
            // SAFETY: `_runtime_start` calls `runtime_main` once on the boot hart,
            // after initializing the SRAM sections, with interrupts disabled
            // and the bootstrap's mapping intact; this is its first statement.
            let proof = unsafe { $crate::__StageTwoHandoff::new() };
            let handoff = $crate::Handoff::from_proof(proof);
            $main(handoff)
        }
    };
}

/// The stack-protector ABI's read-only guard, in SRAM critical data.
#[allow(
    unsafe_code,
    reason = "the runtime defines the stack-protector guard symbol"
)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".critical.data.stack_guard")]
static __stack_chk_guard: u32 = 0xDEED_BAAD;
