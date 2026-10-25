//! SOURCE: RISC-V Privileged Architecture, machine CSRs (`mhartid`,
//! `mstatus`, trap CSRs, `mcycle`, `mcycleh`) and Wait for Interrupt; RISC-V
//! Unprivileged ISA, Zicntr (stable RV32 high/low/high counter read), FENCE and
//! Zifencei; RISC-V CLIC specification (threshold and status); RISC-V Debug
//! Specification, Sdtrig (trigger selection, control and NAPOT store matching).
use core::arch::asm;

#[inline(always)]
pub fn hart_id() -> usize {
    let hart;
    // SAFETY: reading mhartid has no side effects or memory access.
    unsafe { asm!("csrr {hart}, mhartid", hart = out(reg) hart, options(nomem, nostack)) };
    hart
}

#[inline(always)]
pub fn cycle32() -> u32 {
    let value;
    // SAFETY: reading the machine cycle counter changes no state.
    unsafe { asm!("csrr {value}, mcycle", value = out(reg) value, options(nomem, nostack)) };
    value
}

pub fn cycle64() -> u64 {
    loop {
        let before: u32;
        let after: u32;
        // SAFETY: reading the high counter word changes no state.
        unsafe { asm!("csrr {value}, mcycleh", value = out(reg) before, options(nomem, nostack)) };
        let low = cycle32();
        // SAFETY: the second high read detects rollover during the low read.
        unsafe { asm!("csrr {value}, mcycleh", value = out(reg) after, options(nomem, nostack)) };
        if before == after {
            return (u64::from(before) << 32) | u64::from(low);
        }
    }
}

#[inline(always)]
pub fn wait_for_interrupt() {
    // SAFETY: wfi waits on this hart and does not access memory. It is legal
    // with MIE clear; a locally enabled pending interrupt resumes the hart.
    unsafe { asm!("wfi", options(nomem, nostack)) };
}

/// Mask this hart's machine interrupts for `body`, restoring only MIE.
/// Nested calls preserve the outer mask. No cross-hart exclusion is implied.
#[inline(always)]
pub fn interrupt_free<R>(body: impl FnOnce() -> R) -> R {
    struct Restore(usize);
    impl Drop for Restore {
        fn drop(&mut self) {
            if self.0 & crate::machine::status::MIE != 0 {
                // SAFETY: restore the caller's MIE bit; the memory clobber
                // keeps the body inside the masked interval.
                unsafe {
                    asm!("csrsi mstatus, {mie}", mie = const crate::machine::status::MIE, options(nostack))
                };
            }
        }
    }
    let previous;
    // SAFETY: change only this hart's MIE; the memory clobber prevents
    // compiler reordering across the masked interval.
    unsafe {
        asm!("csrrci {previous}, mstatus, {mie}", previous = out(reg) previous,
        mie = const crate::machine::status::MIE, options(nostack))
    };
    let _restore = Restore(previous);
    body()
}

/// Set only this hart's `mstatus.MIE`. This provides no cross-hart exclusion.
#[inline(always)]
pub fn enable_interrupts() {
    // SAFETY: changing this hart's interrupt admission accesses no memory.
    unsafe {
        asm!("csrsi mstatus, {mie}", mie = const crate::machine::status::MIE, options(nomem, nostack))
    };
}

/// Clear only this hart's `mstatus.MIE`.
#[inline(always)]
pub fn disable_interrupts() {
    // SAFETY: masking this hart's interrupts accesses no memory.
    unsafe {
        asm!("csrci mstatus, {mie}", mie = const crate::machine::status::MIE, options(nomem, nostack))
    };
}

/// Clear MIE and return whether it was set. The caller supplies any compiler
/// barriers needed around its protected memory accesses.
#[inline(always)]
pub fn take_interrupts() -> bool {
    let previous: usize;
    // SAFETY: changing only this hart's MIE accesses no memory.
    unsafe {
        asm!("csrrci {previous}, mstatus, {mie}", previous = out(reg) previous,
            mie = const crate::machine::status::MIE, options(nomem, nostack))
    };
    previous & crate::machine::status::MIE != 0
}

/// Exchange the current hart's raw CLIC threshold, returning its low byte.
#[inline(always)]
pub fn swap_interrupt_threshold(threshold: u8) -> u8 {
    let previous: usize;
    // SAFETY: the threshold changes only local interrupt admission, not memory.
    unsafe {
        asm!("csrrw {previous}, {csr}, {threshold}", previous = out(reg) previous,
            threshold = in(reg) usize::from(threshold), csr = const crate::clic::MINTTHRESH,
            options(nomem, nostack))
    };
    previous.to_le_bytes()[0]
}

/// Restore a raw CLIC threshold and give pending interrupts three instructions
/// to enter before the caller continues. Compiler barriers remain the caller's.
/// SOURCE: esp-hal 1f646c5 `esp-sync/src/raw.rs` (`SingleCoreInterruptLock`).
#[inline(always)]
pub fn restore_interrupt_threshold(threshold: u8) {
    // SAFETY: writing the local threshold and three nops access no memory.
    unsafe {
        asm!("csrw {csr}, {threshold}", "nop", "nop", "nop",
            threshold = in(reg) usize::from(threshold), csr = const crate::clic::MINTTHRESH,
            options(nomem, nostack))
    };
}

/// Publish prior reads/writes to instruction fetch on this hart.
#[inline(always)]
pub fn synchronize_instructions() {
    // SAFETY: these barriers order accesses and fetch, without accessing memory.
    unsafe { asm!("fence rw, rw", "fence.i", options(nostack)) };
}

/// Install the current hart's CLIC trap entry, vector table and interrupt stack,
/// then synchronize instruction fetch.
///
/// # Safety
/// `mtvec` must name a live, aligned trap entry (including its mode bits), and
/// `mtvt` a live, aligned vector table. Every entry must implement the trap ABI
/// that uses `mscratch = stack_top` and must retain all memory it can reach.
/// `stack_top` must bound the hart's exclusive, writable interrupt stack.
/// Call in thread mode with interrupts disabled and keep them disabled until
/// all three registers are installed. No intervening exception may use a
/// partially installed configuration.
#[inline(always)]
pub unsafe fn install_trap_stack(mtvec: usize, mtvt: usize, stack_top: usize) {
    // SAFETY: the caller supplies the live trap ABI, vector memory and exclusive
    // stack, and excludes dispatch through a partially installed configuration.
    unsafe {
        asm!("csrw mtvec, {mtvec}", "csrw {mtvt_csr}, {mtvt}",
            "csrw mscratch, {top}", "fence.i", mtvec = in(reg) mtvec,
            mtvt = in(reg) mtvt, top = in(reg) stack_top,
            mtvt_csr = const crate::clic::MTVT, options(nostack))
    };
}

/// Program a machine-mode store watchpoint for the NAPOT encoding `data2`.
/// A debugger-owned trigger (`dmode`) stays unchanged. The caller coordinates
/// trigger selection with other local contexts; this grants no memory access.
#[inline(always)]
pub fn arm_store_trigger(select: usize, data2: usize) {
    use crate::trigger::{self, mcontrol};
    let current: usize;
    // SAFETY: selecting a trigger and reading its control access no memory.
    unsafe {
        asm!("csrw {tselect}, {select}", "csrr {current}, {tdata1}",
            tselect = const trigger::TSELECT, tdata1 = const trigger::TDATA1,
            select = in(reg) select, current = out(reg) current, options(nomem, nostack))
    };
    if current & mcontrol::DMODE != 0 {
        return;
    }
    // SAFETY: the trigger can raise a breakpoint but grants no memory access.
    // The memory clobber keeps guard programming ordered with surrounding accesses.
    unsafe {
        asm!("csrw {tcontrol}, {control}", "csrw {tdata1}, {data1}", "csrw {tdata2}, {data2}",
            tcontrol = const trigger::TCONTROL, tdata1 = const trigger::TDATA1,
            tdata2 = const trigger::TDATA2, control = in(reg) trigger::TCONTROL_MTE,
            data1 = in(reg) mcontrol::M | mcontrol::STORE | mcontrol::MATCH_NAPOT,
            data2 = in(reg) data2, options(nostack))
    };
}

/// Read the selected trigger's control and address without changing selection.
#[inline(always)]
pub fn stack_watchpoint() -> (usize, usize) {
    let (control, address);
    // SAFETY: CSR reads only; the output registers carry no memory authority.
    unsafe {
        asm!("csrr {control}, {tdata1}", "csrr {address}, {tdata2}",
            tdata1 = const crate::trigger::TDATA1, tdata2 = const crate::trigger::TDATA2,
            control = out(reg) control, address = out(reg) address, options(nomem, nostack))
    };
    (control, address)
}

/// Read the calling context's stack pointer; inlined so no call frame intervenes.
#[inline(always)]
pub fn stack_pointer() -> usize {
    let value;
    // SAFETY: reading sp changes no state and accesses no memory.
    unsafe { asm!("mv {value}, sp", value = out(reg) value, options(nomem, nostack)) };
    value
}

/// An endless self-jump, preserving trap state for a debugger without sleeping.
#[inline(always)]
pub fn spin_forever() -> ! {
    // SAFETY: the jump accesses neither memory nor stack and never returns.
    unsafe { asm!("2: j 2b", options(noreturn, nomem, nostack)) }
}

/// Read this hart's `mcause` without accessing memory.
#[inline(always)]
pub fn read_mcause() -> usize {
    let value;
    // SAFETY: this CSR read changes no state and accesses no memory.
    unsafe {
        asm!("csrr {value}, mcause", value = out(reg) value, options(nomem, nostack, preserves_flags))
    };
    value
}

/// Read this hart's `mepc` without accessing memory.
#[inline(always)]
pub fn read_mepc() -> usize {
    let value;
    // SAFETY: this CSR read changes no state and accesses no memory.
    unsafe {
        asm!("csrr {value}, mepc", value = out(reg) value, options(nomem, nostack, preserves_flags))
    };
    value
}

/// Read this hart's `mtval` without accessing memory.
#[inline(always)]
pub fn read_mtval() -> usize {
    let value;
    // SAFETY: this CSR read changes no state and accesses no memory.
    unsafe {
        asm!("csrr {value}, mtval", value = out(reg) value, options(nomem, nostack, preserves_flags))
    };
    value
}

/// Read this hart's `mscratch` without accessing memory.
#[inline(always)]
pub fn read_mscratch() -> usize {
    let value;
    // SAFETY: this CSR read changes no state and accesses no memory.
    unsafe {
        asm!("csrr {value}, mscratch", value = out(reg) value, options(nomem, nostack, preserves_flags))
    };
    value
}

/// Read this hart's `mtvec` without accessing memory.
#[inline(always)]
pub fn read_mtvec() -> usize {
    let value;
    // SAFETY: this CSR read changes no state and accesses no memory.
    unsafe {
        asm!("csrr {value}, mtvec", value = out(reg) value, options(nomem, nostack, preserves_flags))
    };
    value
}

/// Read this hart's `mtvt` without accessing memory.
#[inline(always)]
pub fn read_mtvt() -> usize {
    let value;
    // SAFETY: this CSR read changes no state and accesses no memory.
    unsafe {
        asm!("csrr {value}, {csr}", value = out(reg) value, csr = const crate::clic::MTVT, options(nomem, nostack, preserves_flags))
    };
    value
}

/// Read this hart's `mintstatus` without accessing memory.
#[inline(always)]
pub fn read_mintstatus() -> usize {
    let value;
    // SAFETY: this CSR read changes no state and accesses no memory.
    unsafe {
        asm!("csrr {value}, {csr}", value = out(reg) value, csr = const crate::clic::MINTSTATUS, options(nomem, nostack, preserves_flags))
    };
    value
}
