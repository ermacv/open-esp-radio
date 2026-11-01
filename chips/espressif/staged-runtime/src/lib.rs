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
//! image's `runtime_main` that [`entry!`] defines. During boot, before any
//! executor, an image that declares a secondary hart starts it with
//! [`secondary::launch`] into the hart's generated entry, which calls the
//! hart's root that [`secondary_entry!`] declares on its own stacks; the
//! hart then makes its own interrupt context and executor
//! ([`SecondaryHandoff::adopt`]). The image's `main` adopts
//! the bootstrap's PSRAM mapping through its board and takes its hart's
//! routes and lines ([`adoption::run`]), binds its interrupts through the
//! [`Router`](oer_espressif_interrupt_table::Router) it gets back and enables
//! interrupts ([`Adopted::enable_interrupts`]). The chip HAL selects the
//! chip through the platform port (`oer-espressif-platform-port`), whose
//! metadata names it to the build; the build generates the vector tables,
//! every hart's line entries, each line's default handler and the interrupt
//! table's lines from its facts (`oer-espressif-facts`): its harts and CLIC
//! lines. The hart geometry is the port's, and the image names its platform
//! (the HAL's port type) once, in [`entry!`].
//!
//! The runtime owns the traps ([`stacks`]: the vector tables, every entry
//! and the checked exit; the common entry and exit are `vectors.s`), the
//! image's `critical-section` (`critical`), the panic record ([`panic`]) and
//! the fatal paths (`fatal`: the end of a hart and the chip's reset, over the
//! port's [`Platform`](oer_espressif_platform_port::Platform), which lends
//! the chip HAL's fatal-path proof); the interrupt table's router writes the
//! routes and the CLIC lines through the chip's controller.
pub mod adoption;
#[allow(unsafe_code, reason = "the critical-section implementation contract")]
mod critical;
#[cfg(target_arch = "riscv32")]
#[allow(
    unsafe_code,
    reason = "the default trap handlers export linker-visible symbols"
)]
mod default_handlers;
pub mod diagnostics;
#[cfg(chip_vectors)]
pub mod fatal;
#[allow(
    unsafe_code,
    reason = "terminal election is placed in boot-initialized SRAM"
)]
pub mod retained_fault;
pub mod trace;
#[cfg(all(target_arch = "riscv32", chip_vectors))]
pub use fatal::{fatal_halt, reset, reset_recording};
#[cfg(target_arch = "riscv32")]
#[allow(unsafe_code, reason = "the stage-two reset entry is assembly")]
mod entry;
#[cfg(chip_vectors)]
mod hart;
pub mod interrupts;
#[cfg(chip_vectors)]
pub use hart::{Hart, PreAdmission};
/// The boot hart, as the parameter of its [`Hart`] token: the port's
/// [`BOOT_HART`](oer_espressif_platform_port::BOOT_HART), from the chip facts.
#[cfg(chip_vectors)]
pub const BOOT_HART: u8 = oer_espressif_platform_port::BOOT_HART as u8;
/// The chip's CPU lines and vector-table words, which the build generates
/// from its facts.
#[cfg(chip_vectors)]
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
pub mod secondary;
#[cfg(all(target_arch = "riscv32", chip_vectors))]
pub use secondary::launch;
pub use secondary::{EarlyFault, LaunchFailure, LaunchWaits, Mailbox};
#[cfg(chip_vectors)]
pub use secondary::{Running, SecondaryHandoff};
#[allow(
    unsafe_code,
    reason = "per-hart interrupt stacks and the trap entries switching onto them"
)]
pub mod stacks;
#[allow(unsafe_code, reason = "volatile reads of the installed vector table")]
pub mod trap;

#[doc(hidden)]
pub use oer_espressif_interrupt_table as __interrupt_table;

#[doc(hidden)]
pub use oer_espressif_platform_port as __port;

/// The image's interrupt table as [`interrupt_table!`] declares it.
pub type InterruptTable = oer_espressif_interrupt_table::Table;

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
    InterruptController(oer_espressif_interrupt_table::Error),
}

impl BootError {
    /// The boot-failure reason carried by the image's diagnostic record.
    pub const fn name(self) -> oer_diagnostics::Name<32> {
        match self {
            Self::WrongHart { .. } => oer_diagnostics::name!("interrupt-hart"; 32),
            Self::NoInterruptStack { .. } => oer_diagnostics::name!("interrupt-stack"; 32),
            Self::InvalidStackGuard { .. } => oer_diagnostics::name!("interrupt-stack-guard"; 32),
            Self::ForeignVectors => oer_diagnostics::name!("interrupt-vectors"; 32),
            Self::InterruptController(oer_espressif_interrupt_table::Error::WrongControlBits {
                ..
            }) => oer_diagnostics::name!("interrupt-control-bits"; 32),
            Self::InterruptController(
                oer_espressif_interrupt_table::Error::WrongInterruptCount { .. },
            ) => oer_diagnostics::name!("interrupt-count"; 32),
            Self::InterruptController(oer_espressif_interrupt_table::Error::WrongLevelBits {
                ..
            }) => oer_diagnostics::name!("interrupt-level-bits"; 32),
            Self::InterruptController(_) => oer_diagnostics::name!("interrupt-table"; 32),
        }
    }
}

impl From<oer_espressif_interrupt_table::Error> for BootError {
    fn from(error: oer_espressif_interrupt_table::Error) -> Self {
        Self::InterruptController(error)
    }
}

impl From<oer_espressif_platform_port::ForeignHart> for BootError {
    fn from(error: oer_espressif_platform_port::ForeignHart) -> Self {
        Self::WrongHart {
            actual: error.actual,
            expected: error.expected,
        }
    }
}

/// The image's control at the runtime's entry: stage two runs on the boot hart right
/// after `_runtime_start`, with interrupts disabled and the bootstrap's
/// mapping intact. [`entry!`] gives it once, to the image's `main`, with the
/// image's platform `P` (the chip HAL's port type).
#[must_use = "the handoff adopts the bootstrap's mapping"]
pub struct Handoff<P> {
    proof: oer_espressif_staged_layout::handoff::StageTwoHandoff,
    #[cfg(chip_vectors)]
    admission: PreAdmission<BOOT_HART>,
    _platform: core::marker::PhantomData<fn() -> P>,
}

impl<P> Handoff<P> {
    #[doc(hidden)]
    /// Wrap the proof minted by [`entry!`].
    pub const fn from_proof(proof: oer_espressif_staged_layout::handoff::StageTwoHandoff) -> Self {
        Self {
            proof,
            #[cfg(chip_vectors)]
            admission: PreAdmission::new(),
            _platform: core::marker::PhantomData,
        }
    }

    /// Borrow the boot hart's stack-install authority before adoption.
    #[cfg(chip_vectors)]
    pub fn pre_admission(&mut self) -> &mut PreAdmission<BOOT_HART> {
        &mut self.admission
    }
}

impl<P: oer_espressif_platform_port::EarlyText> Handoff<P> {
    /// Adopt the bootstrap's PSRAM mapping through `board` and install the
    /// stage-two interrupt context of the boot hart: its SRAM interrupt stack and
    /// vectors, the task-stack watchpoint, and the hart's routes and lines,
    /// which the controller `controller` makes from the boot hart's grant
    /// `interrupts` writes. Interrupts stay disabled until
    /// [`Adopted::enable_interrupts`].
    ///
    /// `interrupt_table` is the image's `INTERRUPT_TABLE`
    /// ([`interrupt_table!`]). Installing it silences every source of the
    /// boot hart's bank and disables every line of its CLIC, so from here a
    /// source is routed only by binding its entry.
    ///
    /// Before the mapping is adopted, the previous boot's named fault and
    /// invalid-hart records are reported through `P`'s early writer, then
    /// `previous_records` reports the image's own retained records (the chip
    /// composition's, e.g. the PHY transaction record).
    ///
    /// # Errors
    ///
    /// A wrong hart, missing interrupt stack, invalid stack guard, or controller
    /// disagreement with its chip facts or the image's table returns [`BootError`].
    #[cfg(target_arch = "riscv32")]
    pub(crate) fn adopt<
        B: AdoptsBootstrapMapping,
        I,
        C: oer_espressif_interrupt_table::Controller,
    >(
        mut self,
        interrupt_table: &'static InterruptTable,
        interrupts: I,
        controller: impl FnOnce(I) -> Result<C, oer_espressif_platform_port::ForeignHart>,
        board: B,
        previous_records: impl FnOnce(),
    ) -> Result<(B::Mapping, Adopted<C>), BootError> {
        panic::report_previous_fault::<P>();
        previous_records();
        let mapping = oer_espressif_boot::adoption::adopt_mapping(board, &self.proof);
        let controller = controller(interrupts).map_err(BootError::from)?;
        let actual = usize::from(controller.hart());
        let expected = oer_espressif_platform_port::BOOT_HART.get();
        if actual != expected {
            return Err(BootError::WrongHart { actual, expected });
        }
        stacks::install_interrupt_stack(&mut self.admission)?;
        let router = oer_espressif_interrupt_table::Router::install(controller, interrupt_table)?;
        // Interrupts are disabled and trigger 0 is the runtime's;
        // the generated table names this checked hart's word-aligned task bottom.
        let hart = oer_espressif_platform_port::BOOT_HART;
        stacks::watch_task_stack(hart.at(&layout::stack_symbols()).task.start)?;
        Ok((
            mapping,
            Adopted {
                router,
                admission: self.admission,
            },
        ))
    }
}

/// A hart's adopted interrupt context, before interrupts are enabled: the
/// hart's router, through which the image binds its entries. Hart `H` is the
/// boot hart's (from [`Handoff`]) or a secondary hart's (from its
/// [`SecondaryHandoff`]); each hart has one, over its own controller.
#[must_use = "interrupts stay disabled until `enable_interrupts`"]
pub struct Adopted<C: oer_espressif_interrupt_table::Controller, const H: u8 = DEFAULT_HART> {
    router: oer_espressif_interrupt_table::Router<C, H>,
    #[cfg(chip_vectors)]
    admission: PreAdmission<H>,
}

/// The hart of an [`Adopted`] context that names none: the boot hart.
#[cfg(chip_vectors)]
const DEFAULT_HART: u8 = BOOT_HART;
#[cfg(not(chip_vectors))]
const DEFAULT_HART: u8 = 0;

impl<C: oer_espressif_interrupt_table::Controller, const H: u8> Adopted<C, H> {
    /// Borrow the local execution identity and router together before admission.
    #[cfg(chip_vectors)]
    pub fn parts(&mut self) -> (&Hart<H>, &mut oer_espressif_interrupt_table::Router<C, H>) {
        (self.admission.hart(), &mut self.router)
    }

    /// The hart's router: bind entries before interrupts are enabled.
    pub fn router(&mut self) -> &mut oer_espressif_interrupt_table::Router<C, H> {
        &mut self.router
    }

    /// Check the hart's vectors, routes and lines and hand global interrupt
    /// enable (`mstatus.MIE`) to this hart's executor; the router stays the
    /// image's, for bindings made later. Stack-install authority ends, while
    /// the returned hart token authorizes local executor startup.
    ///
    /// Safe at any point after [`adoption::run`]: adoption silenced every
    /// source and disabled every line, and a source is routed again only by
    /// binding its entry, after its handler holds its part.
    ///
    /// # Errors
    /// [`BootError`] when vector CSRs, controller geometry, configuration,
    /// routes or lines disagree. Global interrupt enable stays disabled.
    #[cfg(target_arch = "riscv32")]
    pub fn enable_interrupts(
        self,
    ) -> Result<(oer_espressif_interrupt_table::Router<C, H>, Hart<H>), BootError> {
        stacks::verify_current_hart_vectors()?;
        self.router.verify()?;
        oer_riscv::enable_interrupts();
        Ok((self.router, self.admission.into_hart()))
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

/// The image's entry and its one panic entry, for the image's platform
/// `$platform` (the chip HAL's port type, a
/// [`Platform`](oer_espressif_platform_port::Platform)): `runtime_main`,
/// which `_runtime_start` calls once on the boot hart, gives `$main` the one
/// [`Handoff`], and the panic handler records the panic and resets
/// ([`panic`]). The same entry binds the common CPU-exception handler.
/// `exception = hook` adds supplemental observations after its minimal record
/// and peer-stop attempt; it cannot replace capture or the reset policy.
///
/// With `hook = $hook`, the panic entry calls the image's hook after its
/// record, in the fatal path with the hart masked and the other hart stopped
/// (or why it is not), before it resets the chip: `$hook` records more of the
/// image's state, formats nothing and returns. It is exported as
/// `oer_platform_panic_hook`, which the image linker places with its strict
/// callees in SRAM and the image checks analyse as a panic path.
///
/// ```ignore
/// // `Port`: the chip HAL's implementation of the platform port.
/// oer_espressif_staged_runtime::entry!(main, Port);
/// fn main(handoff: oer_espressif_staged_runtime::Handoff<Port>) -> ! { … }
/// ```
#[macro_export]
macro_rules! entry {
    (@runtime_main $main:path, $platform:ty) => {
        // SAFETY: `_runtime_start` calls this one image entry with the C ABI;
        // defining a second `runtime_main` fails the link.
        #[unsafe(no_mangle)]
        extern "C" fn runtime_main() -> ! {
            // SAFETY: `_runtime_start` calls `runtime_main` once on the boot hart,
            // after initializing the SRAM sections, with interrupts disabled
            // and the bootstrap's mapping intact; this is its first statement.
            let proof = unsafe { $crate::__StageTwoHandoff::new() };
            let handoff = $crate::Handoff::<$platform>::from_proof(proof);
            $main(handoff)
        }
    };
    ($main:path, $platform:ty) => {
        $crate::entry!($main, $platform, exception = |_claim, _frame, _stop| ());
    };
    ($main:path, $platform:ty, exception = $exception:expr) => {
        $crate::entry!(@runtime_main $main, $platform);
        $crate::__exception_handler!($platform, $exception);
        #[cfg(target_arch = "riscv32")]
        #[panic_handler]
        fn __oer_panic(info: &::core::panic::PanicInfo<'_>) -> ! {
            $crate::panic::handle::<$platform>(info, |_claim, _details, _stop| ())
        }
    };
    ($main:path, $platform:ty, hook = $hook:path) => {
        $crate::entry!($main, $platform, hook = $hook, exception = |_claim, _frame, _stop| ());
    };
    ($main:path, $platform:ty, hook = $hook:path, exception = $exception:expr) => {
        $crate::entry!(@runtime_main $main, $platform);
        $crate::__exception_handler!($platform, $exception);
        #[cfg(target_arch = "riscv32")]
        #[unsafe(no_mangle)]
        fn oer_platform_panic_hook(
            claim: &$crate::retained_fault::Claim<'_>,
            details: &$crate::panic::PanicDetails<'_>,
            stop: ::core::result::Result<
                &<$platform as $crate::__port::Platform>::Stop<'_>,
                <$platform as $crate::__port::Platform>::StopError,
            >,
        ) {
            $hook(claim, details, stop)
        }
        #[cfg(target_arch = "riscv32")]
        #[panic_handler]
        fn __oer_panic(info: &::core::panic::PanicInfo<'_>) -> ! {
            $crate::panic::handle::<$platform>(info, |claim, details, stop| {
                oer_platform_panic_hook(claim, details, stop)
            })
        }
    };
}

/// Secondary hart `$hart`'s root and launch mailbox, for an image that
/// declares the hart (`[package.metadata.open-radio] harts`): the static
/// `$mailbox` (a [`Mailbox`] of platform `$platform`'s interrupt grant and
/// the image's parts `$parts`, in internal SRAM) that the boot hart passes to
/// [`launch`](crate::secondary::launch), and the hart's root
/// `runtime_secondary_main_h<hart>`, which its generated entry tail-calls on
/// the hart's task stack. The root takes the mailbox, installs the hart's
/// interrupt stack and exception path, reports ready and calls `$main` with
/// the hart's one [`SecondaryHandoff`] and the parts; `$main` never returns.
///
/// ```ignore
/// oer_espressif_staged_runtime::secondary_entry!(1, HART1: Port, Hart1Parts => hart1);
/// fn hart1(handoff: SecondaryHandoff<Port, 1>, parts: Hart1Parts) -> ! { … }
/// ```
#[macro_export]
macro_rules! secondary_entry {
    ($hart:literal, $mailbox:ident: $platform:ty, $parts:ty => $main:path) => {
        /// The launch mailbox of the hart's owners, in internal SRAM.
        #[unsafe(link_section = concat!(".critical.data.secondary_mailbox_h", $hart))]
        static $mailbox: $crate::secondary::Mailbox<
            <$platform as $crate::__port::SecondaryStart>::Interrupts,
            $parts,
        > = $crate::secondary::Mailbox::new();
        const _: () = {
            // SAFETY: the hart's generated entry tail-calls this one root
            // with the C ABI on the hart's own task stack; a second
            // definition fails the link.
            #[unsafe(export_name = concat!("runtime_secondary_main_h", $hart))]
            extern "C" fn root() -> ! {
                $crate::secondary::__root::<$platform, $hart, $parts, _>(&$mailbox, $main)
            }
        };
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
