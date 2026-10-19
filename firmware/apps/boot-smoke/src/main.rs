#![no_main]
#![no_std]

//! The smallest staged image: the bootstrap stages it into PSRAM, it adopts
//! the mapping, reports the panic record the previous boot left, if any, and
//! prints `OER-RESULT: pass boot-smoke`.

use core::fmt::Write as _;

use esp_hal::usb::usb_serial_jtag::UsbSerialJtag;
use oer_platform_esp32s31_board as board;
use oer_platform_staged_runtime::panic;

oer_platform_staged_runtime::interrupt_table! {}

#[unsafe(no_mangle)]
extern "C" fn runtime_main() -> ! {
    // Before anything can panic and overwrite it.
    let previous = panic::take_previous();
    let peripherals = esp_hal::init(esp_hal::Config::default());
    // SAFETY: `runtime_main` runs once on CPU0, right after `_runtime_start`,
    // with interrupts disabled and the bootstrap's mapping intact, which
    // nothing has reset or remapped; this image never enables interrupts.
    let _psram = unsafe {
        oer_platform_staged_runtime::adopt(INTERRUPT_TABLE, || {
            board::adopt_psram_mapping(peripherals.PSRAM)
        })
    };
    let mut console = UsbSerialJtag::new(peripherals.USB_DEVICE);
    if let Some(record) = previous {
        let _ = write!(console, "OER-PANIC previous {record}\r\n");
    }
    let _ = console.write(b"OER-RESULT: pass boot-smoke\r\n");
    loop {
        // SAFETY: `wfi` only waits; with interrupts disabled it parks the
        // hart, which is all the image does from here on.
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}
