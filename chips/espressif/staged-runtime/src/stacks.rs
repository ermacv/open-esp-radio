//! PSRAM thread stacks with per-hart SRAM interrupt stacks, and the
//! runtime's trap entries.
//!
//! The staged chips take interrupts through a CLIC with hardware vectoring.
//! The runtime owns both vector registers: `mtvec` names the exception entry
//! `_start_trap` (CLIC mode) and `mtvt` the hart's table in
//! `_runtime_mtvt`, `VECTOR_TABLE_WORDS` per hart, in SRAM; the build
//! generates the tables and every hart's line entries from the chip facts,
//! and `vectors.s` holds their common entry and exit. The word of line `n` holds
//! `_runtime_irq_h<hart>_l<n>`, which swaps `sp` with `mscratch` before its
//! first memory access, saves the interrupted caller registers and
//! `mstatus`, `mepc`, `mcause` and the CLIC threshold on the hart-local SRAM
//! stack, admits nesting, calls the line's handler `__oer_interrupt_h<hart>_l<n>`
//! directly (the interrupt table's entry, or `oer_unhandled_interrupt` for a
//! line no entry binds) and leaves through `_runtime_irq_exit`, which
//! closes nesting, restores those CSRs from its frame and the original stack
//! immediately before `mret` (`vectors.s`). Nested traps stay on the
//! same SRAM stack; only the outermost trap swaps back to PSRAM.
//!
//! No trap saves the floating-point registers, so every entry turns the FPU
//! off (`mstatus.FS`) until its `mret`: a floating-point instruction in
//! interrupt or exception code raises an illegal-instruction exception instead
//! of silently corrupting the interrupted task's registers (on a core without
//! the F extension the field reads zero and the writes do nothing). A debug
//! trigger watches each linked IRQ guard when it fits beside the bound plus
//! margin. Guard coverage is reported and does not guarantee overflow capture.
//!
//! The runtime linker script reserves every hart's task stack and sizes each
//! interrupt stack from its bound or contract maximum; this module finds them by
//! their bounding symbols.

#[cfg(target_arch = "riscv32")]
use core::{
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};

#[cfg_attr(
    not(target_arch = "riscv32"),
    allow(dead_code, reason = "the hart's code alone guards")
)]
mod guard;
#[cfg_attr(
    not(target_arch = "riscv32"),
    allow(dead_code, reason = "the hart's vectors alone take the shift")
)]
mod tcontrol;
#[cfg_attr(
    not(target_arch = "riscv32"),
    allow(dead_code, reason = "the hart's code alone paints")
)]
mod watermark;

#[cfg(target_arch = "riscv32")]
use oer_riscv::{
    clic,
    machine::{self, status},
    trigger,
};

/// The harts with an interrupt stack: the chip facts' harts.
#[cfg(target_arch = "riscv32")]
use oer_espressif_platform_port::HARTS;

/// The words of one hart's vector table in `_runtime_mtvt`, the table's
/// alignment in words, generated from the chip facts
/// (`vector-table-hart-slots`).
#[cfg(chip_vectors)]
pub use crate::vectors::VECTOR_TABLE_WORDS;

#[cfg(target_arch = "riscv32")]
static IRQ_PAINTED: [AtomicBool; HARTS] = [const { AtomicBool::new(false) }; HARTS];

/// Debug trigger of the interrupt-stack guard. Trigger 0 is the task stack
/// guard ([`watch_task_stack`]); trigger 1 stays free.
#[cfg(target_arch = "riscv32")]
const IRQ_STACK_GUARD_TRIGGER: usize = 2;

/// Debug trigger of the task stack guard.
#[cfg(target_arch = "riscv32")]
const TASK_STACK_GUARD_TRIGGER: usize = 0;

#[cfg(target_arch = "riscv32")]
use crate::layout::{_runtime_mtvt, _start_trap};

/// A nonempty interrupt stack belongs to exactly one admitted hart.
#[cfg(target_arch = "riscv32")]
#[inline(always)]
fn interrupt_stack(hart: usize) -> Option<core::ops::Range<usize>> {
    crate::layout::stack_symbols()
        .get(hart)
        .map(|stack| stack.irq.clone())
        .filter(|stack| !stack.is_empty())
}

#[cfg(target_arch = "riscv32")]
#[inline(always)]
fn interrupt_stack_guard_bytes(hart: usize) -> Option<usize> {
    crate::layout::stack_symbols()
        .get(hart)
        .map(|stack| stack.guard)
}

/// Linked interrupt allocation for a checked hart, including explicit zero.
#[cfg(target_arch = "riscv32")]
pub fn irq_stack_bytes(hart: oer_espressif_platform_port::Hart) -> usize {
    hart.at(&crate::layout::stack_symbols()).irq.len()
}

/// Linked PSRAM task allocation for a checked hart, including explicit zero.
#[cfg(target_arch = "riscv32")]
pub fn task_stack_bytes(hart: oer_espressif_platform_port::Hart) -> usize {
    hart.at(&crate::layout::stack_symbols()).task.len()
}

/// The current hart's id.
#[cfg(target_arch = "riscv32")]
#[inline(always)]
fn current_hart() -> usize {
    oer_riscv::hart_id()
}

/// The vector registers `hart` dispatches through: `mtvec` (the exception
/// entry in CLIC mode) and `mtvt` (the hart's table).
#[cfg(target_arch = "riscv32")]
fn vectors_of(hart: usize) -> (usize, usize) {
    let mtvec = ptr::addr_of!(_start_trap) as usize | machine::MTVEC_CLIC_MODE;
    let mtvt = ptr::addr_of!(_runtime_mtvt) as usize + hart * VECTOR_TABLE_WORDS * 4;
    (mtvec, mtvt)
}

/// Install this pre-admission hart's interrupt stack and vectors. The token
/// cannot cross harts or survive interrupt admission. Reinstallation before
/// admission preserves the original paint. Interrupts remain disabled.
///
/// # Errors
/// A different current hart, missing allocation or invalid guard is a boot error.
#[cfg(target_arch = "riscv32")]
pub fn install_interrupt_stack<const H: u8>(
    admission: &mut crate::PreAdmission<H>,
) -> Result<(), crate::BootError> {
    let hart = usize::from(admission.hart().id());
    let actual = current_hart();
    if actual != hart {
        return Err(crate::BootError::WrongHart {
            actual,
            expected: hart,
        });
    }
    // Safe architectural APIs can change MIE even while a token is held.
    // Mask here as well; the local token excludes an interrupt-context caller.
    oer_riscv::disable_interrupts();
    let (Some(stack), Some(painted)) = (interrupt_stack(hart), IRQ_PAINTED.get(hart)) else {
        return Err(crate::BootError::NoInterruptStack { hart });
    };
    let (bottom, top) = (stack.start, stack.end);
    if !painted.load(Ordering::Acquire) {
        // SAFETY: the local pre-admission token owns this inactive IRQ stack;
        // interrupts are masked above; the linker script reserved `bottom..top` for it alone.
        unsafe { watermark::paint(bottom as *mut u32, stack.len() / 4) };
        painted.store(true, Ordering::Release);
    }
    let guard =
        interrupt_stack_guard_bytes(hart).ok_or(crate::BootError::NoInterruptStack { hart })?;
    guard_interrupt_stack(bottom, guard)?;
    let (mtvec, mtvt) = vectors_of(hart);
    // SAFETY: `mtvec` names `_start_trap`, which the assembly aligns to 64
    // bytes, and `mtvt` this hart's 256-byte-aligned table in SRAM; `top` is
    // this hart's interrupt stack top, which every entry expects in
    // `mscratch`. Interrupts stay disabled, so no trap sees the registers
    // change.
    unsafe { oer_riscv::install_trap_stack(mtvec, mtvt, top) };
    Ok(())
}

/// Return a boot error unless the current hart dispatches through the runtime's vectors:
/// `mtvec` names the exception entry in CLIC mode and `mtvt` the hart's
/// table. The static stack analysis checks the table's entries (each swaps
/// `sp` to the interrupt stack before its first memory access and returns
/// through the checked exit); this keeps that check true of the registers
/// the hart dispatches through.
#[cfg(target_arch = "riscv32")]
pub(crate) fn verify_current_hart_vectors() -> Result<(), crate::BootError> {
    let mtvec = oer_riscv::read_mtvec();
    let mtvt = oer_riscv::read_mtvt();
    if (mtvec, mtvt) != vectors_of(current_hart()) {
        return Err(crate::BootError::ForeignVectors);
    }
    Ok(())
}

/// Arm trigger 0 as a store watchpoint on the word at `address`, the bottom
/// of the task stack; a trigger a connected debugger owns stays its own.
///
/// Call with local interrupts disabled and the runtime's word-aligned task
/// stack bottom; the runtime reserves trigger 0 for this guard.
#[cfg(target_arch = "riscv32")]
pub(crate) fn watch_task_stack(address: usize) -> Result<(), crate::BootError> {
    // A NAPOT region of four bytes: the word's address with one trailing one.
    let data2 = guard::napot_guard(address, 4)?;
    oer_riscv::arm_store_trigger(TASK_STACK_GUARD_TRIGGER, data2);
    Ok(())
}

/// Stop on a hart id with no interrupt stack.
#[cfg(target_arch = "riscv32")]
#[cold]
#[expect(
    clippy::panic,
    reason = "fail-stop on a corrupt `mhartid` read before the interrupt stack exists; no caller input"
)]
fn invalid_hart() -> ! {
    panic!("invalid hart for interrupt stack")
}

/// Make every store into the aligned guard of this hart's interrupt
/// stack raise a breakpoint exception. Interrupt handlers run with the
/// triggers of the context they interrupted (`vectors.s`); the trap into
/// the exception entry clears `tcontrol.mte` and the entry leaves it clear,
/// so its own frame below the guard does not retrigger.
///
/// Called with local interrupts disabled; the runtime reserves trigger 2
/// for this hart's interrupt-stack guard.
#[cfg(target_arch = "riscv32")]
fn guard_interrupt_stack(bottom: usize, bytes: usize) -> Result<(), crate::BootError> {
    guard::arm_interrupt_guard(bottom, bytes, |data2| {
        oer_riscv::arm_store_trigger(IRQ_STACK_GUARD_TRIGGER, data2);
    })
}

/// Whether `sp` lies in `hart`'s dedicated interrupt stack. Reads no memory,
/// so a panic in any context can ask.
#[cfg(target_arch = "riscv32")]
#[inline(always)]
pub(crate) fn on_interrupt_stack(hart: usize, sp: usize) -> bool {
    interrupt_stack(hart).is_some_and(|stack| (stack.start..=stack.end).contains(&sp))
}

/// Observed unused prefix of the current hart's dedicated IRQ stack.
///
/// Call from thread mode only. The function briefly masks local interrupts
/// while scanning SRAM; it never reads another hart's stack. It panics before
/// scanning if the stack is uninitialized or the caller is on that IRQ stack.
/// Painting measures observed writes, not a worst-case call-chain bound.
#[cfg(target_arch = "riscv32")]
pub fn current_hart_interrupt_stack_free_bytes() -> usize {
    let hart = oer_riscv::hart_id();
    let sp = oer_riscv::stack_pointer();
    let (Some(stack), Some(painted)) = (interrupt_stack(hart), IRQ_PAINTED.get(hart)) else {
        invalid_hart()
    };
    let bottom = stack.start as *const u32;
    assert!(painted.load(Ordering::Acquire), "IRQ stack is not painted");
    assert!(
        !(stack.start..=stack.end).contains(&sp),
        "IRQ stack sampling requires thread mode"
    );
    // MIE is hart-local. Do not acquire a cross-core lock or scan a foreign stack.
    oer_riscv::interrupt_free(|| {
        // SAFETY: own painted stack, thread mode, local interrupts masked. No other
        // hart uses this allocation; the scanner has a finite capacity and cannot panic.
        (unsafe { watermark::free_words(bottom, stack.len() / 4) }) * 4
    })
}

// The static and its assembly export share this module/codegen unit: an ELF
// assembler cannot export a .set alias of a symbol defined in another unit.
#[cfg(target_arch = "riscv32")]
include!(concat!(env!("OUT_DIR"), "/vector-tables.rs"));

#[cfg(target_arch = "riscv32")]
core::arch::global_asm!(
    include_str!(concat!(env!("OUT_DIR"), "/vectors.s")),
    include_str!("vectors.s"),
    mtvt = sym MTVT,
    frame_bytes = const crate::trap::TRAP_FRAME_BYTES,
    exception_frames = const crate::trap::MAX_EXCEPTION_FRAMES,
    frame_ra = const core::mem::offset_of!(crate::trap::TrapFrame, ra),
    frame_t0 = const core::mem::offset_of!(crate::trap::TrapFrame, t0),
    frame_t1 = const core::mem::offset_of!(crate::trap::TrapFrame, t1),
    frame_t2 = const core::mem::offset_of!(crate::trap::TrapFrame, t2),
    frame_t3 = const core::mem::offset_of!(crate::trap::TrapFrame, t3),
    frame_t4 = const core::mem::offset_of!(crate::trap::TrapFrame, t4),
    frame_t5 = const core::mem::offset_of!(crate::trap::TrapFrame, t5),
    frame_t6 = const core::mem::offset_of!(crate::trap::TrapFrame, t6),
    frame_a0 = const core::mem::offset_of!(crate::trap::TrapFrame, a0),
    frame_a1 = const core::mem::offset_of!(crate::trap::TrapFrame, a1),
    frame_a2 = const core::mem::offset_of!(crate::trap::TrapFrame, a2),
    frame_a3 = const core::mem::offset_of!(crate::trap::TrapFrame, a3),
    frame_a4 = const core::mem::offset_of!(crate::trap::TrapFrame, a4),
    frame_a5 = const core::mem::offset_of!(crate::trap::TrapFrame, a5),
    frame_a6 = const core::mem::offset_of!(crate::trap::TrapFrame, a6),
    frame_a7 = const core::mem::offset_of!(crate::trap::TrapFrame, a7),
    frame_outermost = const core::mem::offset_of!(crate::trap::TrapFrame, outermost),
    frame_mstatus = const core::mem::offset_of!(crate::trap::TrapFrame, mstatus),
    frame_sp = const core::mem::offset_of!(crate::trap::TrapFrame, sp),
    frame_mepc = const core::mem::offset_of!(crate::trap::TrapFrame, mepc),
    frame_mcause = const core::mem::offset_of!(crate::trap::TrapFrame, mcause),
    frame_mintthresh = const core::mem::offset_of!(crate::trap::TrapFrame, mintthresh),
    frame_tcontrol = const core::mem::offset_of!(crate::trap::TrapFrame, tcontrol),
    frame_mtval = const core::mem::offset_of!(crate::trap::TrapFrame, mtval),
    fs = const status::FS_MASK,
    mie = const status::MIE,
    mintthresh = const clic::MINTTHRESH,
    tcontrol = const trigger::TCONTROL,
    mte = const trigger::TCONTROL_MTE,
    mpte_shift = const tcontrol::MPTE_SHIFT,
);
