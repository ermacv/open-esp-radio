//! The runtime's handlers of a line no interrupt table entry binds and of
//! an exception the image handles not: the runtime linker script provides
//! each `__oer_interrupt_h<hart>_l<line>` and `oer_exception_handler` as
//! these unless the image defines its own.

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
    // An endless jump to itself, without sleeping, resetting or changing
    // the CSRs, so a debugger finds the trap's state.
    // SAFETY: this loop touches neither memory nor the stack and never returns.
    unsafe { core::arch::asm!("2: j 2b", options(noreturn, nomem, nostack)) }
}
