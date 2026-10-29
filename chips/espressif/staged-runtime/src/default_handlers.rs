//! The default handler of an interrupt line no image entry binds.

#[unsafe(no_mangle)]
#[expect(
    clippy::panic,
    reason = "fail-stop: an interrupt without a handler is an image defect; the panic handler records it"
)]
pub extern "C" fn oer_unhandled_interrupt() -> ! {
    panic!("unhandled interrupt");
}
