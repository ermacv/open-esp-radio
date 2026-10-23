#![no_main]
#![no_std]
#![forbid(unsafe_code)]

//! The smallest staged image: the bootstrap stages it into PSRAM, it adopts
//! the mapping, reports the panic record the previous boot left, if any, and
//! sends typed diagnostics the host renders as `OER-RESULT: pass boot-smoke`.

use esp_hal::usb::usb_serial_jtag::UsbSerialJtag;
use oer_esp32s31_board as board;
use oer_espressif_staged_runtime::diagnostics::{self, BootSmokePassed, Console};
use oer_espressif_staged_runtime::panic;

oer_espressif_staged_runtime::interrupt_table! {}

oer_espressif_staged_runtime::entry!(main);

fn main(handoff: oer_espressif_staged_runtime::Handoff) -> ! {
    // Before anything can panic and overwrite it.
    let previous = panic::take_previous();
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let (_psram, _adopted) =
        handoff.adopt(INTERRUPT_TABLE, board::BootstrapPsram(peripherals.PSRAM));
    let mut console = UsbSerialJtag::new(peripherals.USB_DEVICE);
    let mut diagnostics = Console::new(|bytes| console.write(bytes));
    if let Some(record) = previous {
        let _ = diagnostics::previous_panic(&mut diagnostics, &record);
    }
    let _ = diagnostics.event(&BootSmokePassed);
    oer_espressif_staged_runtime::idle()
}
