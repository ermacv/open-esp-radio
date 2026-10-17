//! PSRAM thread stacks with per-hart SRAM interrupt stacks.
//!
//! The staged chips use CLIC hardware vectoring, so interrupts do not pass
//! through the ordinary exception entry. The runtime therefore replaces every
//! slot in ESP-HAL's active per-hart MTVT with an SRAM wrapper. Each wrapper
//! swaps `sp` with `mscratch` before its first memory access, saves the
//! interrupted caller context on the hart-local SRAM stack, invokes the
//! existing ESP interrupt dispatcher, and restores the original stack
//! immediately before `mret`. Nested traps remain on the same SRAM stack; only
//! the outermost trap swaps back to PSRAM.
//!
//! No trap saves the floating-point registers, so every entry turns the FPU
//! off (`mstatus.FS`) until its `mret`: a floating-point instruction in
//! interrupt or exception code raises an illegal-instruction exception instead
//! of silently corrupting the interrupted task's registers (on a core without
//! the F extension the field reads zero and the writes do nothing). A debug
//! trigger watches the lowest kibibyte of each hart's interrupt stack, so an
//! overflow traps before it reaches the state below the stack.
//!
//! The runtime linker script reserves the CPU0 task stack and each hart's
//! interrupt stack with the chip layout's sizes; this module finds them by
//! their bounding symbols.

use core::{
    arch::asm,
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};

mod watermark;

/// The harts with an interrupt stack.
const HARTS: usize = if cfg!(feature = "two-harts") { 2 } else { 1 };

static IRQ_PAINTED: [AtomicBool; HARTS] = [const { AtomicBool::new(false) }; HARTS];

/// Offset in the interrupt entry's trap frame of the stack pointer the
/// interrupt preempted: the task stack for an outermost entry, the
/// interrupted handler's SRAM stack for a nested one.
const INTERRUPTED_SP_OFFSET: usize = 72;

/// The stack pointer the running interrupt handler preempted, read from the
/// trap frame this runtime's interrupt entry stored; `None` outside an
/// interrupt handler.
///
/// esp-hal reports the frame ([`esp_hal::interrupt::interrupted_context`]),
/// but only the entry that allocated it knows which stack it moved to and how
/// large the frame is.
pub fn interrupted_stack_pointer() -> Option<usize> {
    let context = esp_hal::interrupt::interrupted_context()?;
    // SAFETY: `frame` is the 80-byte frame `PSRAM_TRAP_ENTER` stored on this
    // hart's SRAM interrupt stack; it stays there until the handler that
    // published it returns, which is after this call.
    Some(unsafe { ptr::read_volatile((context.frame + INTERRUPTED_SP_OFFSET) as *const usize) })
}

/// Aligned bytes at the bottom of each interrupt stack whose stores trap.
const IRQ_STACK_GUARD_BYTES: usize =
    oer_espressif_staged_layout::interrupts::IRQ_STACK_GUARD_BYTES as usize;
/// Debug trigger of the interrupt-stack guard. ESP-HAL owns trigger 0 (task
/// stack guard) and trigger 1 (trap-section guard).
const IRQ_STACK_GUARD_TRIGGER: usize = 2;

unsafe extern "C" {
    static _runtime_psram_mtvt_source: [u32; 48];
    static __runtime_cpu0_irq_stack_bottom: u8;
    static __runtime_cpu0_irq_stack_top: u8;
    static __runtime_cpu1_irq_stack_bottom: u8;
    static __runtime_cpu1_irq_stack_top: u8;
    static __runtime_cpu0_task_stack_bottom: u8;
    static __runtime_cpu0_task_stack_top: u8;
}

/// `hart`'s interrupt stack as `bottom..top`; `None` for a hart without one.
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

/// Bytes of each hart's interrupt stack, as the runtime linker script reserved
/// them.
pub fn irq_stack_bytes() -> usize {
    interrupt_stack(0).map_or(0, |stack| stack.len())
}

/// Bytes of the CPU0 PSRAM task stack, as the runtime linker script reserved
/// them.
pub fn cpu0_task_stack_bytes() -> usize {
    ptr::addr_of!(__runtime_cpu0_task_stack_top) as usize
        - ptr::addr_of!(__runtime_cpu0_task_stack_bottom) as usize
}

/// Restore the per-hart interrupt stack after ESP-HAL reinitializes CLIC.
///
/// # Safety
///
/// Global interrupts must be disabled. The caller must execute on a hart with
/// an interrupt stack (hart 0, or 1 with `two-harts`) in thread mode and must not enable interrupts until this function returns.
/// The first installation must precede the hart's first IRQ admission.
/// Reinstallation preserves the original paint. Installation also silences
/// every source of this hart in the image's interrupt table and checks every
/// vector slot against it (`oer_espressif_interrupt_table_esp_hal`).
pub unsafe fn install_current_hart_interrupt_stack() {
    let hart: usize;
    unsafe { asm!("csrr {hart}, mhartid", hart = out(reg) hart, options(nomem, nostack)) };
    let stack = interrupt_stack(hart).unwrap_or_else(|| panic!("invalid hart for interrupt stack"));
    let (bottom, top) = (stack.start, stack.end);
    if !IRQ_PAINTED[hart].load(Ordering::Acquire) {
        // SAFETY: the caller owns this hart's inactive IRQ stack before
        // admission; the linker script reserved `bottom..top` for it alone.
        unsafe { watermark::paint(bottom as *mut u32, stack.len() / 4) };
        IRQ_PAINTED[hart].store(true, Ordering::Release);
    }
    // SAFETY: interrupts are disabled on this hart and the trigger is not
    // one ESP-HAL or the stack sampler uses.
    unsafe { guard_interrupt_stack(bottom) };
    let table: *mut u32;
    unsafe {
        asm!(
            "csrr {table}, 0x307",
            table = out(reg) table,
            options(nomem, nostack),
        )
    };

    // ESP-HAL owns one writable MTVT per hart and may select a different table
    // while initializing the second core. Preserve that ownership: replace
    // only the hardware-vector slots in whichever table is currently active.
    // Slot zero is not an interrupt entry and remains untouched.
    for (index, source) in unsafe { _runtime_psram_mtvt_source.iter().enumerate().skip(1) } {
        let entry = unsafe { ptr::from_ref(source).read_volatile() };
        unsafe { table.add(index).write_volatile(entry) };
    }
    unsafe {
        asm!(
            "csrw mscratch, {top}",
            "fence.i",
            top = in(reg) top,
            options(nostack),
        )
    };
    oer_espressif_interrupt_table_esp_hal::install_current_hart();
}

/// Panic unless the current hart's active MTVT holds the runtime's entries in
/// every hardware-vector slot.
///
/// The static stack analysis checks the entries of `_runtime_psram_mtvt_source`
/// (each swaps `sp` to the interrupt stack before its first memory access);
/// this keeps that check true of the table the hart dispatches through: a slot
/// another owner rewrote after [`install_current_hart_interrupt_stack`], such
/// as ESP-HAL's direct IPC binding, would run on the interrupted stack.
pub(crate) fn verify_current_hart_vectors() {
    let table: *const u32;
    unsafe {
        asm!(
            "csrr {table}, 0x307",
            table = out(reg) table,
            options(nomem, nostack),
        )
    };
    let source = unsafe { &*ptr::addr_of!(_runtime_psram_mtvt_source) };
    let active: [u32; 48] = core::array::from_fn(|slot| unsafe { table.add(slot).read_volatile() });
    if let Some(slot) = foreign_vector(&active, source) {
        panic!("interrupt vector {slot} is not the runtime's stack-switching entry");
    }
}

/// The first hardware-vector slot (slot zero is none) of `active` that differs
/// from `source`.
fn foreign_vector(active: &[u32; 48], source: &[u32; 48]) -> Option<usize> {
    (1..source.len()).find(|&slot| active[slot] != source[slot])
}

/// NAPOT `tdata2` watching `bytes` (a power of two) aligned at or above
/// `bottom`: the aligned start with the low bits encoding the length.
const fn napot_guard(bottom: usize, bytes: usize) -> usize {
    let start = (bottom + bytes - 1) & !(bytes - 1);
    start | (bytes / 2 - 1)
}

/// Make every store into the lowest aligned kibibyte of this hart's interrupt
/// stack raise a breakpoint exception. The exception entry clears
/// `tcontrol.mte`, so its own frame below the guard does not retrigger.
///
/// # Safety
///
/// Interrupts must be disabled on the calling hart, and no other code may own
/// debug trigger [`IRQ_STACK_GUARD_TRIGGER`].
unsafe fn guard_interrupt_stack(bottom: usize) {
    // A connected debugger owns the triggers, as for ESP-HAL's stack guard.
    if esp_hal::debugger::debugger_connected() {
        return;
    }
    // mcontrol: machine mode, store, NAPOT match.
    const TDATA1_M_STORE_NAPOT: usize = (1 << 6) | (1 << 1) | (1 << 7);
    // tcontrol.mte: triggers fire in machine mode.
    const TCONTROL_MTE: usize = 1 << 3;
    unsafe {
        asm!(
            "csrw 0x7a0, {select}",
            "csrw 0x7a5, {control}",
            "csrw 0x7a1, {data1}",
            "csrw 0x7a2, {data2}",
            select = in(reg) IRQ_STACK_GUARD_TRIGGER,
            control = in(reg) TCONTROL_MTE,
            data1 = in(reg) TDATA1_M_STORE_NAPOT,
            data2 = in(reg) napot_guard(bottom, IRQ_STACK_GUARD_BYTES),
            options(nostack),
        )
    };
}

/// Whether `sp` lies in `hart`'s dedicated interrupt stack. Reads no memory,
/// so a panic in any context can ask.
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
pub fn current_hart_interrupt_stack_free_bytes() -> usize {
    let hart: usize;
    let sp: usize;
    unsafe {
        asm!("csrr {hart}, mhartid", "mv {sp}, sp", hart = out(reg) hart, sp = out(reg) sp, options(nomem, nostack))
    };
    let stack = interrupt_stack(hart).unwrap_or_else(|| panic!("invalid hart for interrupt stack"));
    let bottom = stack.start as *const u32;
    assert!(
        IRQ_PAINTED[hart].load(Ordering::Acquire),
        "IRQ stack is not painted"
    );
    assert!(
        !(stack.start..=stack.end).contains(&sp),
        "IRQ stack sampling requires thread mode"
    );
    let previous: usize;
    // MIE is hart-local. Do not acquire a cross-core lock or scan a foreign stack.
    // SAFETY: clearing MIE only masks this hart's interrupts; the prior state
    // is restored below.
    unsafe {
        asm!("csrrci {previous}, mstatus, 8", previous = out(reg) previous, options(nostack))
    };
    // SAFETY: own painted stack, thread mode, local interrupts masked. No other
    // hart uses this allocation; the scanner has a finite capacity and cannot panic.
    let free = unsafe { watermark::free_words(bottom, stack.len() / 4) } * 4;
    if previous & 8 != 0 {
        // SAFETY: restores the MIE state observed on entry; handlers were
        // already enabled then, so no new dispatch becomes possible.
        unsafe { asm!("csrsi mstatus, 8", options(nostack)) };
    }
    free
}

core::arch::global_asm!(
    r#"
    .section .trap.psram_task_stack, "ax", @progbits
    .option push
    .option norelax
    .option norvc

    .balign 4
    .global _runtime_stack_bootstrap
    .type _runtime_stack_bootstrap, @function
_runtime_stack_bootstrap:
    la t0, __runtime_cpu0_task_stack_bottom
    addi t0, t0, 256
    la t1, __runtime_cpu0_task_stack_top
    addi t1, t1, -256
    li t2, 0xa55aa55a
1:
    bgeu t0, t1, 2f
    sw t2, 0(t0)
    addi t0, t0, 4
    j 1b
2:
    la sp, __runtime_cpu0_task_stack_top
    la t0, __runtime_cpu0_irq_stack_top
    csrw mscratch, t0
    ret
    .size _runtime_stack_bootstrap, . - _runtime_stack_bootstrap

    // Swap before the first memory access. The second CSR exchange preserves
    // the interrupted t0 in mscratch and gives t0 the other stack pointer:
    //
    // outer:  sp = IRQ top, t0 = task sp
    // nested: sp = task sp, t0 = interrupted IRQ sp
    //
    // The unsigned address comparison distinguishes internal SRAM from PSRAM.
    // A nested entry swaps sp/t0 back with register-only XOR operations, then
    // both paths allocate an aligned frame on the SRAM stack. During dispatch,
    // mscratch always contains the task sp so deeper nesting is handled alike.
    //
    // Frame (80 bytes): ra, t0..t6 and a0..a7 at 0..60 as riscv-rt's
    // TrapFrame, the outermost flag at 64, the interrupted mstatus at 68 and
    // the interrupted sp at INTERRUPTED_SP_OFFSET (72).
    .macro PSRAM_TRAP_ENTER
    csrrw sp, mscratch, sp
    csrrw t0, mscratch, t0
    bltu sp, t0, 90f

    // Nested trap: normalize sp back to the interrupted SRAM stack while
    // retaining the task stack pointer in t0.
    xor sp, sp, t0
    xor t0, sp, t0
    xor sp, sp, t0
    addi sp, sp, -80
    sw t1, 8(sp)
    sw zero, 64(sp)
    // The interrupted handler's sp is just above this frame.
    addi t1, sp, 80
    sw t1, 72(sp)
    j 91f

90:
    // Outermost trap: sp already names the hart-local SRAM stack.
    addi sp, sp, -80
    sw t1, 8(sp)
    li t1, 1
    sw t1, 64(sp)
    // t0 holds the interrupted task sp.
    sw t0, 72(sp)

91:
    sw ra, 0(sp)
    // Both paths saved t1 before using it to recover the interrupted t0.
    csrr t1, mscratch
    sw t1, 4(sp)
    sw t2, 12(sp)
    sw t3, 16(sp)
    sw t4, 20(sp)
    sw t5, 24(sp)
    sw t6, 28(sp)
    sw a0, 32(sp)
    sw a1, 36(sp)
    sw a2, 40(sp)
    sw a3, 44(sp)
    sw a4, 48(sp)
    sw a5, 52(sp)
    sw a6, 56(sp)
    sw a7, 60(sp)
    // No trap saves the FP registers: run the handler with the FPU off and
    // keep the interrupted FS state for the matching return.
    csrr t1, mstatus
    sw t1, 68(sp)
    li t1, 0x6000
    csrc mstatus, t1
    csrw mscratch, t0
    .endm

    .macro PSRAM_TRAP_RESTORE_REGISTERS
    lw ra, 0(sp)
    lw t0, 4(sp)
    lw t1, 8(sp)
    lw t2, 12(sp)
    lw t3, 16(sp)
    lw t4, 20(sp)
    lw t5, 24(sp)
    lw t6, 28(sp)
    lw a0, 32(sp)
    lw a1, 36(sp)
    lw a2, 40(sp)
    lw a3, 44(sp)
    lw a4, 48(sp)
    lw a5, 52(sp)
    lw a6, 56(sp)
    lw a7, 60(sp)
    .endm

    // Direct exceptions enter through mtvec slot zero. This deliberately
    // mirrors riscv-rt's caller-register TrapFrame on the SRAM stack.
    .balign 4
    .global _start_trap
    .type _start_trap, @function
_start_trap:
    PSRAM_TRAP_ENTER
    mv a0, sp
    call _start_trap_rust
    j _runtime_psram_trap_restore
    .size _start_trap, . - _start_trap

    // Every hardware-vector entry performs the stack swap itself because CLIC
    // dispatches through mtvt without visiting the direct exception entry.
    .macro PSRAM_IRQ_ENTRY number
    .balign 4
    .global _runtime_psram_irq_entry_\number
    .type _runtime_psram_irq_entry_\number, @function
_runtime_psram_irq_entry_\number:
    PSRAM_TRAP_ENTER
    la a0, Trap\number
    j _runtime_psram_interrupt_continue
    .size _runtime_psram_irq_entry_\number, . - _runtime_psram_irq_entry_\number
    .endm

    PSRAM_IRQ_ENTRY 1
    PSRAM_IRQ_ENTRY 2
    PSRAM_IRQ_ENTRY 3
    PSRAM_IRQ_ENTRY 4
    PSRAM_IRQ_ENTRY 5
    PSRAM_IRQ_ENTRY 6
    PSRAM_IRQ_ENTRY 7
    PSRAM_IRQ_ENTRY 8
    PSRAM_IRQ_ENTRY 9
    PSRAM_IRQ_ENTRY 10
    PSRAM_IRQ_ENTRY 11
    PSRAM_IRQ_ENTRY 12
    PSRAM_IRQ_ENTRY 13
    PSRAM_IRQ_ENTRY 14
    PSRAM_IRQ_ENTRY 15
    PSRAM_IRQ_ENTRY 16
    PSRAM_IRQ_ENTRY 17
    PSRAM_IRQ_ENTRY 18
    PSRAM_IRQ_ENTRY 19
    PSRAM_IRQ_ENTRY 20
    PSRAM_IRQ_ENTRY 21
    PSRAM_IRQ_ENTRY 22
    PSRAM_IRQ_ENTRY 23
    PSRAM_IRQ_ENTRY 24
    PSRAM_IRQ_ENTRY 25
    PSRAM_IRQ_ENTRY 26
    PSRAM_IRQ_ENTRY 27
    PSRAM_IRQ_ENTRY 28
    PSRAM_IRQ_ENTRY 29
    PSRAM_IRQ_ENTRY 30
    PSRAM_IRQ_ENTRY 31
    PSRAM_IRQ_ENTRY 32
    PSRAM_IRQ_ENTRY 33
    PSRAM_IRQ_ENTRY 34
    PSRAM_IRQ_ENTRY 35
    PSRAM_IRQ_ENTRY 36
    PSRAM_IRQ_ENTRY 37
    PSRAM_IRQ_ENTRY 38
    PSRAM_IRQ_ENTRY 39
    PSRAM_IRQ_ENTRY 40
    PSRAM_IRQ_ENTRY 41
    PSRAM_IRQ_ENTRY 42
    PSRAM_IRQ_ENTRY 43
    PSRAM_IRQ_ENTRY 44
    PSRAM_IRQ_ENTRY 45
    PSRAM_IRQ_ENTRY 46
    PSRAM_IRQ_ENTRY 47

    .balign 4
    .global _runtime_psram_interrupt_continue
    .type _runtime_psram_interrupt_continue, @function
_runtime_psram_interrupt_continue:
    jalr ra, a0, 0

_runtime_psram_trap_restore:
    // Give the interrupted context back its FPU state (FS bits only).
    lw t0, 68(sp)
    li t1, 0x6000
    and t0, t0, t1
    csrs mstatus, t0
    lw t0, 64(sp)
    beqz t0, 92f

    // Outermost return restores the task stack and arms mscratch with the IRQ
    // stack top for the next first-level trap.
    PSRAM_TRAP_RESTORE_REGISTERS
    addi sp, sp, 80
    csrrw sp, mscratch, sp
    mret

92:
    // A nested return resumes the interrupted handler on its existing SRAM
    // stack. mscratch remains the task stack pointer for further nesting.
    PSRAM_TRAP_RESTORE_REGISTERS
    addi sp, sp, 80
    mret

    // Source image copied into ESP-HAL's active per-hart hardware-vector table.
    // Slot zero remains unused because synchronous exceptions use mtvec and
    // `_start_trap` directly.
    .balign 0x40
    .global _runtime_psram_mtvt_source
    .type _runtime_psram_mtvt_source, @object
_runtime_psram_mtvt_source:
    .word 0
    .word _runtime_psram_irq_entry_1
    .word _runtime_psram_irq_entry_2
    .word _runtime_psram_irq_entry_3
    .word _runtime_psram_irq_entry_4
    .word _runtime_psram_irq_entry_5
    .word _runtime_psram_irq_entry_6
    .word _runtime_psram_irq_entry_7
    .word _runtime_psram_irq_entry_8
    .word _runtime_psram_irq_entry_9
    .word _runtime_psram_irq_entry_10
    .word _runtime_psram_irq_entry_11
    .word _runtime_psram_irq_entry_12
    .word _runtime_psram_irq_entry_13
    .word _runtime_psram_irq_entry_14
    .word _runtime_psram_irq_entry_15
    .word _runtime_psram_irq_entry_16
    .word _runtime_psram_irq_entry_17
    .word _runtime_psram_irq_entry_18
    .word _runtime_psram_irq_entry_19
    .word _runtime_psram_irq_entry_20
    .word _runtime_psram_irq_entry_21
    .word _runtime_psram_irq_entry_22
    .word _runtime_psram_irq_entry_23
    .word _runtime_psram_irq_entry_24
    .word _runtime_psram_irq_entry_25
    .word _runtime_psram_irq_entry_26
    .word _runtime_psram_irq_entry_27
    .word _runtime_psram_irq_entry_28
    .word _runtime_psram_irq_entry_29
    .word _runtime_psram_irq_entry_30
    .word _runtime_psram_irq_entry_31
    .word _runtime_psram_irq_entry_32
    .word _runtime_psram_irq_entry_33
    .word _runtime_psram_irq_entry_34
    .word _runtime_psram_irq_entry_35
    .word _runtime_psram_irq_entry_36
    .word _runtime_psram_irq_entry_37
    .word _runtime_psram_irq_entry_38
    .word _runtime_psram_irq_entry_39
    .word _runtime_psram_irq_entry_40
    .word _runtime_psram_irq_entry_41
    .word _runtime_psram_irq_entry_42
    .word _runtime_psram_irq_entry_43
    .word _runtime_psram_irq_entry_44
    .word _runtime_psram_irq_entry_45
    .word _runtime_psram_irq_entry_46
    .word _runtime_psram_irq_entry_47
    .size _runtime_psram_mtvt_source, . - _runtime_psram_mtvt_source

    .option pop
"#
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rewritten_hardware_vector_is_foreign_but_slot_zero_is_not_one() {
        let source: [u32; 48] = core::array::from_fn(|slot| 0x1000 + 4 * slot as u32);
        let mut active = source;
        active[0] = 0;
        assert_eq!(foreign_vector(&active, &source), None);
        active[3] = 0x2000;
        assert_eq!(foreign_vector(&active, &source), Some(3));
    }
}
