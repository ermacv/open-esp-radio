//! A secondary hart's launch during boot, before any executor exists, the
//! hand-over of its owners and its own interrupt context.
//!
//! On the boot hart, [`launch`] checks that the hart's stacks are linked and
//! the handshake is fresh, releases the hart into its generated entry through
//! the platform ([`SecondaryStart`]), waits (`secondary-entered`, a named boot
//! wait) for the entry to report itself, retires the ROM boot word, moves the
//! hart's interrupt grant and the image's parts for it into the hart's
//! [`Mailbox`], fences and acknowledges, then waits (`secondary-ready`) for
//! the hart's Rust root to report its interrupt stack and exception path
//! installed, and checks the stack pointer it reports against the hart's task
//! stack.
//!
//! On the secondary hart, the generated entry (`entry.rs`) masks interrupts
//! first, installs a stack-free fault stub, stores `ENTERED`, waits for `ACK`,
//! checks PMP, programs its PMA chain from the chip facts and installs `gp`,
//! its task stack and the FPU before it tail-calls its root
//! `runtime_secondary_main_h<N>`, which the image declares with
//! [`secondary_entry!`](crate::secondary_entry). The root takes the mailbox
//! once, mints the hart's [`SecondaryHandoff`], installs the hart's interrupt
//! stack and the common exception path, reports `READY` and calls the
//! image's hart main with the handoff and its parts. A fault before `READY`
//! stores its kind and observations and `FAULT`; the boot hart then records
//! and resets.
//!
//! The image's hart main makes its own interrupt context
//! ([`SecondaryHandoff::adopt`]): only on the hart does its grant become its
//! controller, whose router silences every route of the hart's bank and
//! disables every line of its own CLIC; then it binds its entries and
//! enables interrupts ([`Adopted::enable_interrupts`](crate::Adopted)),
//! which returns the hart's router and its `Hart<H>` for its executor.
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
/// The root found no hand-over: `cause` = 1 for another hart than its own
/// (`value` = the hart that ran it), 2 for an empty or taken mailbox.
pub(crate) const KIND_HANDOVER: u32 = 5;

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
    /// The hart's owners were not handed over: `code` 1 when the released
    /// hart held no interrupt grant, 2 when the mailbox was used before.
    Handover { code: u32 },
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
            Self::Handover { code } => (SecondaryLaunch, [9, code, 0, 0]),
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

/// What the boot hart hands a secondary hart at its launch: the platform's
/// interrupt grant `I` of the hart and the image's parts `T` for it (its
/// executor, its interrupt parts and tokens, its endpoints). The image
/// declares one per secondary hart, in internal SRAM, with
/// [`secondary_entry!`](crate::secondary_entry); the launch fills it once
/// before its acknowledgement and the hart's root takes it once after.
pub struct Mailbox<I, T> {
    cell: oer_memory::ClaimCell<(I, T)>,
}

impl<I, T> Mailbox<I, T> {
    /// The empty mailbox, which the image declares once.
    #[allow(
        clippy::new_without_default,
        reason = "the one mailbox is a static, built in a const context"
    )]
    pub const fn new() -> Self {
        Self {
            cell: oer_memory::ClaimCell::new(),
        }
    }
}

/// Launch secondary hart `H` during boot, before any executor exists, under
/// the boot `deadline`: release it into its generated entry, wait for it to
/// enter, retire its boot word, hand it its interrupt grant and `parts`
/// through `mailbox`, acknowledge and wait for it to report ready.
///
/// # Errors
///
/// [`LaunchFailure`] with its observations; the caller resets
/// ([`LaunchFailure::reset`]).
#[cfg(all(target_arch = "riscv32", chip_vectors))]
#[allow(clippy::too_many_arguments, reason = "the launch's owners and waits")]
pub fn launch<P, C, const H: u8, T>(
    cores: &mut P::Cores,
    permit: P::Permit,
    preconditions: P::Preconditions,
    deadline: &BootDeadline<'_, C>,
    waits: LaunchWaits,
    mailbox: &'static Mailbox<P::Interrupts, T>,
    parts: T,
) -> Result<Running<P, H>, LaunchFailure<P::StartError>>
where
    P: oer_espressif_platform_port::SecondaryStart,
    C: Clock + ?Sized,
    T: Send,
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
        |released| hand_over(mailbox, P::interrupts(released), parts),
        oer_riscv::fence,
    )?;
    Ok(Running { released, ready_sp })
}

/// Move the hart's interrupt grant and its parts into `mailbox`.
#[cfg_attr(
    not(target_arch = "riscv32"),
    allow(dead_code, reason = "the chip's launch hands over")
)]
fn hand_over<I, T>(mailbox: &Mailbox<I, T>, interrupts: Option<I>, parts: T) -> Result<(), u32> {
    let interrupts = interrupts.ok_or(1_u32)?;
    mailbox.cell.install((interrupts, parts)).map_err(|_| 2_u32)
}

/// The hart's linked stacks.
struct Stacks {
    task: core::ops::Range<usize>,
    irq: usize,
}

/// The ordered launch over a handshake `view`: provisioning and a fresh
/// handshake, the release, the entry, the boot word's retirement, the
/// hand-over, a fence, the acknowledgement and the ready report.
#[allow(clippy::too_many_arguments, reason = "the launch's ordered steps")]
fn run<V, C, X, R, E>(
    view: &V,
    deadline: &oer_espressif_boot::wait::BootDeadline<'_, C>,
    waits: LaunchWaits,
    stacks: Stacks,
    cores: &mut X,
    release: impl FnOnce(&mut X) -> Result<R, E>,
    retire: impl FnOnce(&mut X, &mut R) -> Result<(), E>,
    hand_over: impl FnOnce(&mut R) -> Result<(), u32>,
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
    hand_over(&mut released).map_err(|code| LaunchFailure::Handover { code })?;
    // The hand-over and the retired word before the acknowledgement, which
    // the entry reads before it takes the mailbox.
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

/// Secondary hart `H`'s control at its Rust root: its stack-install authority
/// and its interrupt grant. Only the hart's root
/// ([`secondary_entry!`](crate::secondary_entry)) mints it, once, from the
/// mailbox the launch filled.
///
/// ```compile_fail
/// use oer_espressif_staged_runtime::SecondaryHandoff;
/// let handoff = SecondaryHandoff::<(), 1>::new();
/// ```
#[cfg(chip_vectors)]
#[must_use = "the handoff holds the hart's interrupt grant"]
pub struct SecondaryHandoff<P: oer_espressif_platform_port::SecondaryStart, const H: u8> {
    admission: crate::PreAdmission<H>,
    interrupts: P::Interrupts,
}

#[cfg(chip_vectors)]
impl<P: oer_espressif_platform_port::SecondaryStart, const H: u8> SecondaryHandoff<P, H> {
    /// Make the hart's own interrupt context, interrupts still masked: its
    /// grant becomes its controller here, on the hart, and the router over
    /// it silences every route of the hart's bank and disables every line of
    /// its own CLIC, then checks them against `table`, the image's
    /// `INTERRUPT_TABLE`; the hart's CLIC threshold is set to admit every
    /// level. The returned context binds the hart's entries and
    /// enables its interrupts ([`Adopted::enable_interrupts`](crate::Adopted)).
    ///
    /// # Errors
    ///
    /// [`BootError::WrongHart`](crate::BootError) when the grant or its
    /// controller is another hart's; a controller disagreement with its chip
    /// facts or the table.
    pub fn adopt(
        self,
        table: &'static crate::InterruptTable,
    ) -> Result<crate::Adopted<P::Controller, H>, crate::BootError>
    where
        P::Controller: oer_espressif_interrupt_table::Controller,
    {
        let controller = P::controller(self.interrupts)?;
        let actual = usize::from(oer_espressif_interrupt_table::Controller::hart(&controller));
        if actual != usize::from(H) {
            return Err(crate::BootError::WrongHart {
                actual,
                expected: usize::from(H),
            });
        }
        let router = oer_espressif_interrupt_table::Router::install(controller, table)?;
        // The hart's CLIC threshold admits every level from here: the ROM's
        // secondary path leaves it unestablished, and interrupts stay masked
        // (`mstatus.MIE`) until `enable_interrupts`.
        #[cfg(target_arch = "riscv32")]
        oer_riscv::restore_interrupt_threshold(0);
        Ok(crate::Adopted {
            router,
            admission: self.admission,
        })
    }

    /// Idle for good with interrupts masked, the grant unused: a hart with
    /// nothing to run.
    #[cfg(target_arch = "riscv32")]
    pub fn idle(self) -> ! {
        crate::halt()
    }
}

/// The root of secondary hart `H`, which
/// [`secondary_entry!`](crate::secondary_entry) exports as
/// `runtime_secondary_main_h<H>` for the hart's generated entry: on hart `H`,
/// take `mailbox` once, install the hart's interrupt stack and the common
/// exception path, report `READY` with the stack pointer and call `main`
/// with the hart's one handoff and the image's parts. Any refusal reports
/// `FAULT` and idles masked; the boot hart then records and resets.
#[doc(hidden)]
#[cfg(all(target_arch = "riscv32", chip_vectors))]
pub fn __root<P, const H: u8, T, R>(
    mailbox: &'static Mailbox<P::Interrupts, T>,
    main: impl FnOnce(SecondaryHandoff<P, H>, T) -> R,
) -> !
where
    P: oer_espressif_platform_port::SecondaryStart,
{
    let current = oer_riscv::hart_id();
    if current != usize::from(H) {
        HANDSHAKE.report_fault(KIND_HANDOVER, 1, 0, current as u32);
        crate::halt()
    }
    let Ok((interrupts, parts)) = mailbox.cell.take() else {
        HANDSHAKE.report_fault(KIND_HANDOVER, 2, 0, u32::from(H));
        crate::halt()
    };
    // The one mint of the hart's authority: the mailbox gives its contents
    // once, only the launch's one permit fills one, and the hart check above
    // keeps it on hart `H`.
    let mut admission = crate::PreAdmission::<H>::new();
    if let Err(error) = crate::stacks::install_interrupt_stack(&mut admission) {
        HANDSHAKE.report_fault(KIND_INTERRUPT_STACK, boot_error_code(error), 0, 0);
        crate::halt()
    }
    let stacks = crate::layout::stack_symbols();
    match stacks
        .get(usize::from(H))
        .map(|stack| crate::stacks::watch_task_stack(stack.task.start))
    {
        Some(Ok(())) => {}
        Some(Err(error)) => {
            HANDSHAKE.report_fault(KIND_INTERRUPT_STACK, boot_error_code(error), 0, 0);
            crate::halt()
        }
        None => {
            HANDSHAKE.report_fault(KIND_INTERRUPT_STACK, 0, 0, u32::from(H));
            crate::halt()
        }
    }
    HANDSHAKE
        .ready_sp
        .store(oer_riscv::stack_pointer() as u32, Ordering::Relaxed);
    HANDSHAKE.ready.store(READY, Ordering::Release);
    // The image's main never returns (`-> !`); a main that does idles masked.
    let _never = main(
        SecondaryHandoff {
            admission,
            interrupts,
        },
        parts,
    );
    crate::halt()
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
