//! Runtime-owned fallbacks for the trap entry and interrupt vector table.

#[unsafe(no_mangle)]
#[expect(
    clippy::panic,
    reason = "fail-stop: an interrupt without a handler is an image defect; the panic handler records it"
)]
pub extern "C" fn oer_unhandled_interrupt() -> ! {
    panic!("unhandled interrupt");
}

#[unsafe(no_mangle)]
pub extern "C" fn oer_default_exception() -> ! {
    // Match riscv-rt 0.16.0/src/asm.rs's `_default_abort`: an endless
    // jump to itself, without sleeping, resetting or changing the CSRs.
    // SAFETY: this loop touches neither memory nor the stack and never returns.
    unsafe { core::arch::asm!("2: j 2b", options(noreturn, nomem, nostack)) }
}
