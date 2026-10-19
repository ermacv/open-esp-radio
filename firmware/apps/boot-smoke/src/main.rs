#![no_main]
#![no_std]
#![forbid(unsafe_code)]

//! The smallest staged image: the bootstrap stages it into PSRAM, it adopts
//! the mapping, reports the panic record the previous boot left, if any, and
//! prints `OER-RESULT: pass boot-smoke`.

use core::fmt::Write as _;

use esp_hal::usb::usb_serial_jtag::UsbSerialJtag;
use oer_platform_esp32s31_board as board;
use oer_platform_staged_runtime::panic;

oer_platform_staged_runtime::interrupt_table! {}

oer_platform_staged_runtime::entry!(main);

fn main(handoff: oer_platform_staged_runtime::Handoff) -> ! {
    // Before anything can panic and overwrite it.
    let previous = panic::take_previous();
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let (_psram, _adopted) =
        handoff.adopt(INTERRUPT_TABLE, board::BootstrapPsram(peripherals.PSRAM));
    let mut console = UsbSerialJtag::new(peripherals.USB_DEVICE);
    if let Some(record) = previous {
        let _ = write!(console, "OER-PANIC previous {record}\r\n");
    }
    let _ = console.write(b"OER-RESULT: pass boot-smoke\r\n");
    oer_platform_staged_runtime::idle()
}
