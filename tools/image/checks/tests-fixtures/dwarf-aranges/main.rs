#![no_std]
#![no_main]

#[unsafe(no_mangle)]
#[unsafe(link_section = ".ram")]
pub extern "C" fn _start(value: u32) -> u32 {
    shared::callee(value)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {}
}
