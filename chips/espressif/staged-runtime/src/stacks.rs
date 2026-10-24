//! PSRAM thread stacks with per-hart SRAM interrupt stacks, and the
//! runtime's trap entries.
//!
//! The staged chips take interrupts through a CLIC with hardware vectoring.
//! The runtime owns both vector registers: `mtvec` names the exception entry
//! `_start_trap` (CLIC mode) and `mtvt` the hart's table in
//! `_runtime_mtvt`, 64 words per hart, in SRAM. The word of line `n` holds
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
//! trigger watches the chip contract's guard of each hart's interrupt stack, so an
//! overflow traps before it reaches the state below the stack.
//!
//! The runtime linker script reserves the CPU0 task stack and sizes each
//! interrupt stack from its bound or contract maximum; this module finds them by
//! their bounding symbols.

#[cfg(target_arch = "riscv32")]
use core::{
    arch::asm,
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
    allow(dead_code, reason = "the hart's code alone paints")
)]
mod watermark;

#[cfg(target_arch = "riscv32")]
use oer_riscv::{
    clic,
    machine::{self, status},
    trigger::{self, mcontrol},
};

/// The harts with an interrupt stack.
const HARTS: usize = if cfg!(feature = "two-harts") { 2 } else { 1 };

/// Whether the trap exit raises the CLIC threshold before it clears
/// `mstatus.MIE`: on the ESP32-C5 clearing MIE alone does not mask every
/// interrupt (in the middle of a multi-cycle instruction).
///
/// SOURCE: esp-hal 1f646c5 `esp-sync/src/raw.rs` (`soc_cpu_has_zcmp_workaround`,
/// set for the ESP32-C5 and not the ESP32-S31 by
/// `esp-metadata-generated/src/_generated_esp32c5.rs`); ESP-IDF 4d59230d
/// `riscv/vectors.S` (`mintthresh_csr_disable`).
#[cfg(target_arch = "riscv32")]
const RAISE_THRESHOLD: bool = cfg!(feature = "esp32c5");

/// The words of one hart's vector table in `_runtime_mtvt`: 256 bytes, the
/// table's alignment, of which lines 0 to 47 are used.
pub const VECTOR_TABLE_WORDS: usize = 64;

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
unsafe extern "C" {
    static _runtime_mtvt: [u32; VECTOR_TABLE_WORDS];
    static _start_trap: u8;
    static __runtime_cpu0_irq_stack_bottom: u8;
    static __runtime_cpu0_irq_stack_top: u8;
    static __runtime_cpu0_irq_stack_guard_bytes: u8;
    static __runtime_cpu1_irq_stack_bottom: u8;
    static __runtime_cpu1_irq_stack_top: u8;
    static __runtime_cpu1_irq_stack_guard_bytes: u8;
    static __runtime_cpu0_task_stack_bottom: u8;
    static __runtime_cpu0_task_stack_top: u8;
}

/// `hart`'s interrupt stack as `bottom..top`; `None` for a hart without one.
#[cfg(target_arch = "riscv32")]
#[inline(always)]
fn interrupt_stack(hart: usize) -> Option<core::ops::Range<usize>> {
    let (bottom, top) = match hart {
        0 => (
            ptr::addr_of!(__runtime_cpu0_irq_stack_bottom),
            ptr::addr_of!(__runtime_cpu0_irq_stack_top),
        ),
        1 if HARTS > 1 => (
            ptr::addr_of!(__runtime_cpu1_irq_stack_bottom),
            ptr::addr_of!(__runtime_cpu1_irq_stack_top),
        ),
        _ => return None,
    };
    Some(bottom as usize..top as usize)
}

/// Absolute linker symbols supplied from this chip's per-hart contract.
#[cfg(target_arch = "riscv32")]
#[inline(always)]
fn interrupt_stack_guard_bytes(hart: usize) -> usize {
    match hart {
        0 => ptr::addr_of!(__runtime_cpu0_irq_stack_guard_bytes) as usize,
        1 if HARTS > 1 => ptr::addr_of!(__runtime_cpu1_irq_stack_guard_bytes) as usize,
        _ => 0,
    }
}

/// Bytes of hart 0's interrupt stack, as the runtime linker script reserved
/// it. Other harts may have different allocations.
#[cfg(target_arch = "riscv32")]
pub fn irq_stack_bytes() -> usize {
    interrupt_stack(0).map_or(0, |stack| stack.len())
}

/// Bytes of the CPU0 PSRAM task stack, as the runtime linker script reserved
/// them.
#[cfg(target_arch = "riscv32")]
pub fn cpu0_task_stack_bytes() -> usize {
    ptr::addr_of!(__runtime_cpu0_task_stack_top) as usize
        - ptr::addr_of!(__runtime_cpu0_task_stack_bottom) as usize
}

/// The current hart's id.
#[cfg(target_arch = "riscv32")]
#[inline(always)]
fn current_hart() -> usize {
    let hart: usize;
    // SAFETY: reading `mhartid` touches no memory.
    unsafe { asm!("csrr {hart}, mhartid", hart = out(reg) hart, options(nomem, nostack)) };
    hart
}

/// The vector registers `hart` dispatches through: `mtvec` (the exception
/// entry in CLIC mode) and `mtvt` (the hart's table).
#[cfg(target_arch = "riscv32")]
fn vectors_of(hart: usize) -> (usize, usize) {
    let mtvec = ptr::addr_of!(_start_trap) as usize | machine::MTVEC_CLIC_MODE;
    let mtvt = ptr::addr_of!(_runtime_mtvt) as usize + hart * VECTOR_TABLE_WORDS * 4;
    (mtvec, mtvt)
}

/// Install the current hart's interrupt stack and vectors: paint the stack
/// once, guard its bottom, point `mtvec` at the exception entry and `mtvt`
/// at the hart's table, and arm `mscratch` with the stack's top.
///
/// # Safety
///
/// Global interrupts must be disabled. The caller must execute on a hart with
/// an interrupt stack (hart 0, or 1 with `two-harts`) in thread mode and must
/// not enable interrupts until this function returns. The first installation
/// must precede the hart's first interrupt admission. Reinstallation
/// preserves the original paint.
#[cfg(target_arch = "riscv32")]
pub unsafe fn install_current_hart_interrupt_stack() {
    let hart = current_hart();
    let (Some(stack), Some(painted)) = (interrupt_stack(hart), IRQ_PAINTED.get(hart)) else {
        invalid_hart()
    };
    let (bottom, top) = (stack.start, stack.end);
    if !painted.load(Ordering::Acquire) {
        // SAFETY: the caller owns this hart's inactive IRQ stack before
        // admission; the linker script reserved `bottom..top` for it alone.
        unsafe { watermark::paint(bottom as *mut u32, stack.len() / 4) };
        painted.store(true, Ordering::Release);
    }
    // SAFETY: interrupts are disabled on this hart and the trigger is the
    // interrupt stack's own.
    unsafe { guard_interrupt_stack(bottom, interrupt_stack_guard_bytes(hart)) };
    let (mtvec, mtvt) = vectors_of(hart);
    // SAFETY: `mtvec` names `_start_trap`, which the assembly aligns to 64
    // bytes, and `mtvt` this hart's 256-byte-aligned table in SRAM; `top` is
    // this hart's interrupt stack top, which every entry expects in
    // `mscratch`. Interrupts stay disabled, so no trap sees the registers
    // change.
    unsafe {
        asm!(
            "csrw mtvec, {mtvec}",
            "csrw {mtvt_csr}, {mtvt}",
            "csrw mscratch, {top}",
            "fence.i",
            mtvec = in(reg) mtvec,
            mtvt = in(reg) mtvt,
            top = in(reg) top,
            mtvt_csr = const clic::MTVT,
            options(nostack),
        )
    };
}

/// Panic unless the current hart dispatches through the runtime's vectors:
/// `mtvec` names the exception entry in CLIC mode and `mtvt` the hart's
/// table. The static stack analysis checks the table's entries (each swaps
/// `sp` to the interrupt stack before its first memory access and returns
/// through the checked exit); this keeps that check true of the registers
/// the hart dispatches through.
#[cfg(target_arch = "riscv32")]
pub(crate) fn verify_current_hart_vectors() {
    let (mtvec, mtvt): (usize, usize);
    // SAFETY: CSR reads only.
    unsafe {
        asm!(
            "csrr {mtvec}, mtvec",
            "csrr {mtvt}, {mtvt_csr}",
            mtvec = out(reg) mtvec,
            mtvt = out(reg) mtvt,
            mtvt_csr = const clic::MTVT,
            options(nomem, nostack),
        )
    };
    if (mtvec, mtvt) != vectors_of(current_hart()) {
        foreign_vectors_installed();
    }
}

/// Arm trigger 0 as a store watchpoint on the word at `address`, the bottom
/// of the task stack; a trigger a connected debugger owns stays its own.
///
/// # Safety
///
/// Interrupts must be disabled on the calling hart, `address` must be word
/// aligned, and no other code may own trigger 0.
#[cfg(target_arch = "riscv32")]
pub(crate) unsafe fn watch_task_stack(address: usize) {
    // A NAPOT region of four bytes: the word's address with one trailing one.
    let data2 = guard::napot_guard(address, 4);
    // SAFETY: the caller's contract.
    unsafe { arm_store_trigger(TASK_STACK_GUARD_TRIGGER, data2) };
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

/// Stop on vector registers that are not the runtime's.
#[cfg(target_arch = "riscv32")]
#[cold]
#[expect(
    clippy::panic,
    reason = "fail-stop on vector registers another writer changed; no caller input"
)]
fn foreign_vectors_installed() -> ! {
    panic!("the hart dispatches through vectors that are not the runtime's");
}

/// Make every store into the aligned guard of this hart's interrupt
/// stack raise a breakpoint exception. The exception entry clears
/// `tcontrol.mte`, so its own frame below the guard does not retrigger.
///
/// # Safety
///
/// Interrupts must be disabled on the calling hart, and no other code may own
/// debug trigger [`IRQ_STACK_GUARD_TRIGGER`].
#[cfg(target_arch = "riscv32")]
unsafe fn guard_interrupt_stack(bottom: usize, bytes: usize) {
    let data2 = guard::napot_guard(bottom, bytes);
    // SAFETY: the caller's contract.
    unsafe { arm_store_trigger(IRQ_STACK_GUARD_TRIGGER, data2) };
}

/// Arm trigger `select` to raise a breakpoint exception on a machine-mode
/// store into the NAPOT region `data2`, unless a connected debugger owns
/// the trigger (`tdata1.dmode`, which ignores machine-mode writes).
///
/// # Safety
///
/// Interrupts must be disabled on the calling hart, and no other code may
/// own trigger `select`.
#[cfg(target_arch = "riscv32")]
unsafe fn arm_store_trigger(select: usize, data2: usize) {
    // mcontrol: machine mode, store, NAPOT match.
    const TDATA1_M_STORE_NAPOT: usize = mcontrol::M | mcontrol::STORE | mcontrol::MATCH_NAPOT;
    let current: usize;
    // SAFETY: selecting a trigger and reading its control touch no memory.
    unsafe {
        asm!(
            "csrw {tselect}, {select}",
            "csrr {current}, {tdata1}",
            tselect = const trigger::TSELECT,
            tdata1 = const trigger::TDATA1,
            select = in(reg) select,
            current = out(reg) current,
            options(nomem, nostack),
        )
    };
    if current & mcontrol::DMODE != 0 {
        return;
    }
    // SAFETY: the caller's contract: interrupts are disabled and trigger
    // `select` is the caller's; programming it touches no memory.
    unsafe {
        asm!(
            "csrw {tcontrol}, {control}",
            "csrw {tdata1}, {data1}",
            "csrw {tdata2}, {data2}",
            tcontrol = const trigger::TCONTROL,
            tdata1 = const trigger::TDATA1,
            tdata2 = const trigger::TDATA2,
            control = in(reg) trigger::TCONTROL_MTE,
            data1 = in(reg) TDATA1_M_STORE_NAPOT,
            data2 = in(reg) data2,
            options(nostack),
        )
    };
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
    let hart: usize;
    let sp: usize;
    // SAFETY: reads `mhartid` and `sp`; touches no memory.
    unsafe {
        asm!("csrr {hart}, mhartid", "mv {sp}, sp", hart = out(reg) hart, sp = out(reg) sp, options(nomem, nostack))
    };
    let (Some(stack), Some(painted)) = (interrupt_stack(hart), IRQ_PAINTED.get(hart)) else {
        invalid_hart()
    };
    let bottom = stack.start as *const u32;
    assert!(painted.load(Ordering::Acquire), "IRQ stack is not painted");
    assert!(
        !(stack.start..=stack.end).contains(&sp),
        "IRQ stack sampling requires thread mode"
    );
    let previous: usize;
    // MIE is hart-local. Do not acquire a cross-core lock or scan a foreign stack.
    // SAFETY: clearing MIE only masks this hart's interrupts; the prior state
    // is restored below.
    unsafe {
        asm!("csrrci {previous}, mstatus, {mie}", previous = out(reg) previous, mie = const status::MIE, options(nostack))
    };
    // SAFETY: own painted stack, thread mode, local interrupts masked. No other
    // hart uses this allocation; the scanner has a finite capacity and cannot panic.
    let free = unsafe { watermark::free_words(bottom, stack.len() / 4) } * 4;
    if previous & status::MIE != 0 {
        // SAFETY: restores the MIE state observed on entry; handlers were
        // already enabled then, so no new dispatch becomes possible.
        unsafe { asm!("csrsi mstatus, {mie}", mie = const status::MIE, options(nostack)) };
    }
    free
}

#[cfg(target_arch = "riscv32")]
core::arch::global_asm!(
    include_str!("vectors.s"),
    fs = const status::FS_MASK,
    mie = const status::MIE,
    mintthresh = const clic::MINTTHRESH,
    harts = const HARTS,
    raise_threshold = const RAISE_THRESHOLD as u8,
);
