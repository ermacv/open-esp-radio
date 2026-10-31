//! A secondary hart's launch during boot, before any executor exists, and its
//! idle on its own stacks.
//!
//! On the boot hart, [`launch`] checks that the hart's stacks are linked and
//! the handshake is fresh, releases the hart into its generated entry through
//! the platform ([`SecondaryStart`]), waits (`secondary-entered`, a named boot
//! wait) for the entry to report itself, retires the ROM boot word, fences and
//! acknowledges, then waits (`secondary-ready`) for the hart's Rust root to
//! report its interrupt stack and exception path installed, and checks the
//! stack pointer it reports against the hart's task stack.
//!
//! On the secondary hart, the generated entry (`entry.rs`) masks interrupts
//! first, installs a stack-free fault stub, stores `ENTERED`, waits for `ACK`,
//! checks PMP, programs its PMA chain from the chip facts and installs `gp`,
//! its task stack and the FPU before it tail-calls its generated
//! `runtime_secondary_main_h<N>`, which mints the hart's
//! [`SecondaryHandoff`]. [`SecondaryHandoff::idle`] installs the hart's
//! interrupt stack and the common exception path, reports `READY` and idles
//! in a masked wait. A fault before `READY` stores its kind and observations
//! and `FAULT`; the boot hart then records and resets.
//!
//! Every failure after the release ends in record and reset
//! ([`LaunchFailure::reset`]): the released hart cannot be confirmed held again.

use core::sync::atomic::{AtomicU32, Ordering};

#[cfg(all(target_arch = "riscv32", chip_vectors))]
use oer_espressif_boot::wait::BootDeadline;
use oer_espressif_boot::wait::{BootWait, WaitFailure};
use oer_time::Clock;

/// The entry reached the handshake: its interrupts are masked and its fault
/// stub installed.
pub(crate) const ENTERED: u32 = 0x5ec0_e001;
/// The boot hart retired the boot word: the entry may continue.
pub(crate) const ACK: u32 = 0x5ec0_ac02;
/// The hart's interrupt stack and exception path are installed.
pub(crate) const READY: u32 = 0x5ec0_4e03;
/// The hart stopped in its fault stub; `kind` says why.
pub(crate) const FAULT: u32 = 0x5ec0_fa0f;

/// A CPU exception before the common exception path: `cause` = `mcause`,
/// `pc` = `mepc`, `value` = `mtval`.
pub(crate) const KIND_EXCEPTION: u32 = 1;
/// A nonzero PMP configuration: `cause` = the `pmpcfg` index, `pc` = 0,
/// `value` = the word read.
pub(crate) const KIND_PMP: u32 = 2;
/// A PMA write that read back otherwise: `cause` = the write's index (the
/// entry, plus 0x100 for its address), `pc` = the value written, `value` =
/// the value read.
pub(crate) const KIND_PMA: u32 = 3;
/// The interrupt-stack installation refused: `cause` = its code.
pub(crate) const KIND_INTERRUPT_STACK: u32 = 4;

/// The words the boot hart and a starting secondary hart exchange, in
/// internal SRAM. The entry's assembly stores to them by offset.
#[repr(C)]
pub(crate) struct Handshake {
    pub(crate) entered: AtomicU32,
    pub(crate) ack: AtomicU32,
    pub(crate) ready: AtomicU32,
    pub(crate) kind: AtomicU32,
    pub(crate) cause: AtomicU32,
    pub(crate) pc: AtomicU32,
    pub(crate) value: AtomicU32,
    pub(crate) ready_sp: AtomicU32,
}

impl Handshake {
    pub(crate) const fn new() -> Self {
        Self {
            entered: AtomicU32::new(0),
            ack: AtomicU32::new(0),
            ready: AtomicU32::new(0),
            kind: AtomicU32::new(0),
            cause: AtomicU32::new(0),
            pc: AtomicU32::new(0),
            value: AtomicU32::new(0),
            ready_sp: AtomicU32::new(0),
        }
    }

    /// Store a fault's observations, then `FAULT`.
    #[cfg_attr(
        not(target_arch = "riscv32"),
        allow(dead_code, reason = "the hart's code alone reports")
    )]
    fn report_fault(&self, kind: u32, cause: u32, pc: u32, value: u32) {
        self.kind.store(kind, Ordering::Relaxed);
        self.cause.store(cause, Ordering::Relaxed);
        self.pc.store(pc, Ordering::Relaxed);
        self.value.store(value, Ordering::Relaxed);
        self.ready.store(FAULT, Ordering::Release);
    }
}

crate::placed_static! {
    /// The one handshake of the boot: only the root's one-shot permit starts
    /// a hart, so no launch finds it used.
    #[allow(unsafe_code, reason = "the handshake is placed in internal SRAM")]
    pub(crate) static HANDSHAKE: Handshake = Handshake::new(), in critical_data "secondary_handshake";
}

/// What the boot hart observes of the handshake, so its waits run on the
/// host over a model.
trait HandshakeView {
    fn entered(&self) -> u32;
    fn ack(&self) -> u32;
    fn ready(&self) -> u32;
    fn ready_sp(&self) -> u32;
    fn fault(&self) -> EarlyFault;
    fn acknowledge(&self);
}

impl HandshakeView for Handshake {
    fn entered(&self) -> u32 {
        self.entered.load(Ordering::Acquire)
    }
    fn ack(&self) -> u32 {
        self.ack.load(Ordering::Acquire)
    }
    fn ready(&self) -> u32 {
        self.ready.load(Ordering::Acquire)
    }
    fn ready_sp(&self) -> u32 {
        self.ready_sp.load(Ordering::Acquire)
    }
    fn fault(&self) -> EarlyFault {
        EarlyFault {
            kind: self.kind.load(Ordering::Acquire),
            cause: self.cause.load(Ordering::Acquire),
            pc: self.pc.load(Ordering::Acquire),
            value: self.value.load(Ordering::Acquire),
        }
    }
    fn acknowledge(&self) {
        self.ack.store(ACK, Ordering::Release);
    }
}

/// The two named boot waits of a launch, with the chip's bounds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LaunchWaits {
    /// From the release to the entry's `ENTERED` (`secondary-entered`).
    pub entered: BootWait,
    /// From the acknowledgement to `READY` (`secondary-ready`).
    pub ready: BootWait,
}

/// What the hart's fault stub or its Rust root stored before `FAULT`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EarlyFault {
    pub kind: u32,
    pub cause: u32,
    pub pc: u32,
    pub value: u32,
}

/// Why a launch did not complete. `E` is the platform's start error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchFailure<E> {
    /// The image linked no task or no interrupt stack for the hart.
    Unprovisioned { task: u32, irq: u32 },
    /// The runtime has no generated entry for the hart.
    NoEntry { hart: u8 },
    /// The handshake was used before: a second launch.
    HandshakeUsed { entered: u32, ack: u32, ready: u32 },
    /// The platform refused or did not confirm the release.
    Start(E),
    /// The entry did not report itself within its wait or the deadline.
    NotEntered(WaitFailure),
    /// The hart stopped in its fault stub.
    EarlyFault(EarlyFault),
    /// The boot word was not retired.
    BootWord(E),
    /// The hart did not report ready within its wait or the deadline.
    NotReady(WaitFailure),
    /// The ready hart's stack pointer lies outside its task stack.
    StackOutside { sp: u32 },
}

impl<E: Copy> LaunchFailure<E> {
    /// The retained reason and its four words; `observe` gives a start
    /// error's code and observations.
    pub fn diagnostic(
        self,
        observe: impl FnOnce(E) -> [u32; 3],
    ) -> (crate::panic::FaultReason, [u32; 4]) {
        use crate::panic::FaultReason::{SecondaryEarlyFault, SecondaryLaunch};
        let wait = |failure: WaitFailure| match failure {
            WaitFailure::ZeroBound(_) => 0,
            WaitFailure::TimedOut(_) => 1,
            WaitFailure::DeadlineExceeded(_) => 2,
        };
        match self {
            Self::EarlyFault(EarlyFault {
                kind,
                cause,
                pc,
                value,
            }) => (SecondaryEarlyFault, [kind, cause, pc, value]),
            Self::Unprovisioned { task, irq } => (SecondaryLaunch, [1, task, irq, 0]),
            Self::NoEntry { hart } => (SecondaryLaunch, [2, u32::from(hart), 0, 0]),
            Self::HandshakeUsed {
                entered,
                ack,
                ready,
            } => (SecondaryLaunch, [3, entered, ack, ready]),
            Self::Start(error) => {
                let [code, first, second] = observe(error);
                (SecondaryLaunch, [4, code, first, second])
            }
            Self::NotEntered(failure) => (SecondaryLaunch, [5, wait(failure), 0, 0]),
            Self::BootWord(error) => {
                let [code, first, second] = observe(error);
                (SecondaryLaunch, [6, code, first, second])
            }
            Self::NotReady(failure) => (SecondaryLaunch, [7, wait(failure), 0, 0]),
            Self::StackOutside { sp } => (SecondaryLaunch, [8, sp, 0, 0]),
        }
    }
}

#[cfg(all(target_arch = "riscv32", chip_vectors))]
impl<E: Copy> LaunchFailure<E> {
    /// Record this failure and reset the chip through platform `P`'s fatal
    /// path: the released hart is not confirmed held again, so the boot
    /// cannot continue.
    pub fn reset<P>(self) -> !
    where
        P: oer_espressif_platform_port::SecondaryStart<StartError = E>,
    {
        let (reason, observations) = self.diagnostic(P::observations);
        crate::panic::reset_fault::<P>(reason, observations)
    }
}

/// A started secondary hart: its platform release, which keeps the hart's
/// owners for later parts, and the stack pointer it reported ready on.
#[cfg(chip_vectors)]
#[must_use = "dropping the running hart loses its retained owners"]
pub struct Running<P: oer_espressif_platform_port::SecondaryStart, const H: u8> {
    released: P::Released,
    ready_sp: u32,
}

#[cfg(chip_vectors)]
impl<P: oer_espressif_platform_port::SecondaryStart, const H: u8> Running<P, H> {
    /// The started hart.
    pub const fn hart(&self) -> u8 {
        H
    }

    /// The stack pointer the hart reported ready on, inside its task stack.
    pub const fn ready_sp(&self) -> u32 {
        self.ready_sp
    }

    /// The platform's released owners.
    pub fn released(&mut self) -> &mut P::Released {
        &mut self.released
    }
}

/// Launch secondary hart `H` during boot, before any executor exists, under
/// the boot `deadline`: release it into its generated entry, wait for it to
/// enter, retire its boot word, acknowledge and wait for it to report ready.
///
/// # Errors
///
/// [`LaunchFailure`] with its observations; the caller resets
/// ([`LaunchFailure::reset`]).
#[cfg(all(target_arch = "riscv32", chip_vectors))]
pub fn launch<P, C, const H: u8>(
    cores: &mut P::Cores,
    permit: P::Permit,
    preconditions: P::Preconditions,
    deadline: &BootDeadline<'_, C>,
    waits: LaunchWaits,
) -> Result<Running<P, H>, LaunchFailure<P::StartError>>
where
    P: oer_espressif_platform_port::SecondaryStart,
    C: Clock + ?Sized,
{
    const {
        if H as usize == crate::BOOT_HART as usize {
            panic!("the boot hart is not launched");
        }
        if H as usize >= oer_espressif_platform_port::HARTS {
            panic!("the hart lies inside the chip geometry");
        }
    }
    let stacks = crate::layout::stack_symbols();
    let Some(stack) = stacks.get(usize::from(H)) else {
        return Err(LaunchFailure::NoEntry { hart: H });
    };
    let entry = crate::entry::secondary_entry::<H>().ok_or(LaunchFailure::NoEntry { hart: H })?;
    let task = stack.task.clone();
    let irq = stack.irq.len();
    let (released, ready_sp) = run(
        &HANDSHAKE,
        deadline,
        waits,
        Stacks { task, irq },
        cores,
        |cores| P::release(cores, permit, preconditions, entry),
        |cores, released| P::retire_boot_word(cores, released),
        oer_riscv::fence,
    )?;
    Ok(Running { released, ready_sp })
}

/// The hart's linked stacks.
struct Stacks {
    task: core::ops::Range<usize>,
    irq: usize,
}

/// The ordered launch over a handshake `view`: provisioning and a fresh
/// handshake, the release, the entry, the boot word's retirement, a fence,
/// the acknowledgement and the ready report.
#[allow(clippy::too_many_arguments, reason = "the launch's ordered steps")]
fn run<V, C, X, R, E>(
    view: &V,
    deadline: &oer_espressif_boot::wait::BootDeadline<'_, C>,
    waits: LaunchWaits,
    stacks: Stacks,
    cores: &mut X,
    release: impl FnOnce(&mut X) -> Result<R, E>,
    retire: impl FnOnce(&mut X, &mut R) -> Result<(), E>,
    fence: impl FnOnce(),
) -> Result<(R, u32), LaunchFailure<E>>
where
    V: HandshakeView,
    C: Clock + ?Sized,
{
    if stacks.task.is_empty() || stacks.irq == 0 {
        return Err(LaunchFailure::Unprovisioned {
            task: stacks.task.len() as u32,
            irq: stacks.irq as u32,
        });
    }
    let (entered, ack, ready) = (view.entered(), view.ack(), view.ready());
    if entered != 0 || ack != 0 || ready != 0 {
        return Err(LaunchFailure::HandshakeUsed {
            entered,
            ack,
            ready,
        });
    }
    let mut released = release(cores).map_err(LaunchFailure::Start)?;
    deadline
        .poll(waits.entered, || {
            if view.ready() == FAULT {
                Some(Err(view.fault()))
            } else {
                (view.entered() == ENTERED).then_some(Ok(()))
            }
        })
        .map_err(LaunchFailure::NotEntered)?
        .map_err(LaunchFailure::EarlyFault)?;
    retire(cores, &mut released).map_err(LaunchFailure::BootWord)?;
    fence();
    view.acknowledge();
    deadline
        .poll(waits.ready, || match view.ready() {
            READY => Some(Ok(())),
            FAULT => Some(Err(view.fault())),
            _ => None,
        })
        .map_err(LaunchFailure::NotReady)?
        .map_err(LaunchFailure::EarlyFault)?;
    let sp = view.ready_sp();
    if !(stacks.task.start..=stacks.task.end).contains(&(sp as usize)) {
        return Err(LaunchFailure::StackOutside { sp });
    }
    Ok((released, sp))
}

/// Secondary hart `H`'s control at its Rust root: its stack-install authority.
/// Only the hart's generated `runtime_secondary_main_h<N>` mints it.
///
/// ```compile_fail
/// use oer_espressif_staged_runtime::SecondaryHandoff;
/// let handoff = SecondaryHandoff::<1>::new();
/// ```
#[cfg(chip_vectors)]
pub struct SecondaryHandoff<const H: u8> {
    admission: crate::PreAdmission<H>,
}

#[cfg(chip_vectors)]
impl<const H: u8> SecondaryHandoff<H> {
    /// The hart's one handoff. Only hart `H`'s generated root calls it, once
    /// per boot, after the launch's acknowledgement: no other context holds
    /// the hart's interrupt stack or vector registers.
    pub(crate) const fn new() -> Self {
        const {
            if H as usize == crate::BOOT_HART as usize {
                panic!("the boot hart's handoff is the entry's");
            }
        }
        Self {
            admission: crate::PreAdmission::new(),
        }
    }

    /// Install the hart's interrupt stack and the common exception path,
    /// report `READY` with the current stack pointer and idle with interrupts
    /// masked. Interrupt routing and executors on this hart belong to later
    /// owners. A refused installation reports `FAULT` and idles the same way.
    #[cfg(target_arch = "riscv32")]
    pub fn idle(mut self) -> ! {
        match crate::stacks::install_interrupt_stack(&mut self.admission) {
            Ok(()) => {
                let stacks = crate::layout::stack_symbols();
                let watched = stacks
                    .get(usize::from(H))
                    .map(|stack| crate::stacks::watch_task_stack(stack.task.start));
                match watched {
                    Some(Ok(())) => {
                        HANDSHAKE
                            .ready_sp
                            .store(oer_riscv::stack_pointer() as u32, Ordering::Relaxed);
                        HANDSHAKE.ready.store(READY, Ordering::Release);
                    }
                    Some(Err(error)) => {
                        HANDSHAKE.report_fault(KIND_INTERRUPT_STACK, boot_error_code(error), 0, 0)
                    }
                    None => HANDSHAKE.report_fault(KIND_INTERRUPT_STACK, 0, 0, u32::from(H)),
                }
            }
            Err(error) => {
                HANDSHAKE.report_fault(KIND_INTERRUPT_STACK, boot_error_code(error), 0, 0);
            }
        }
        crate::halt()
    }
}

/// A refused installation's code.
#[cfg_attr(
    not(target_arch = "riscv32"),
    allow(dead_code, reason = "the hart's code alone reports")
)]
const fn boot_error_code(error: crate::BootError) -> u32 {
    match error {
        crate::BootError::WrongHart { .. } => 1,
        crate::BootError::NoInterruptStack { .. } => 2,
        crate::BootError::InvalidStackGuard { .. } => 3,
        crate::BootError::ForeignVectors => 4,
        crate::BootError::InterruptController(_) => 5,
    }
}

#[cfg(test)]
mod tests;
