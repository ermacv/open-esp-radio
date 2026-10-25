//! SOURCE: RISC-V Privileged Architecture, machine CSRs (`mhartid`,
//! `mstatus`, `mcycle`, `mcycleh`) and Wait for Interrupt; RISC-V
//! Unprivileged ISA, Zicntr (stable RV32 high/low/high counter read).
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
