// An unconfigured image has no tasks or enabled interrupt sources.
use oer_esp32s31_hal::root::Root;
use oer_esp32s31_hal::peripherals::usb_serial_jtag;
use oer_espressif_staged_runtime::panic;

mod gate_fault;
mod release_fault;

oer_espressif_staged_runtime::interrupt_table! {
    contexts {}
}
oer_espressif_staged_runtime::entry!(main, oer_esp32s31_hal::platform::Port);

fn main(handoff: oer_espressif_staged_runtime::Handoff<oer_esp32s31_hal::platform::Port>) -> ! {
    let previous = panic::peek_previous::<oer_esp32s31_hal::platform::Port>();
    let Some(root) = Root::take() else {
        fail("root-taken");
    };
    let (_runtime, _psram, _adopted) = match oer_espressif_staged_runtime::adoption::run::<oer_esp32s31_compose::Esp32s31, _, _>(
        handoff,
        root.into_runtime(),
        INTERRUPT_TABLE,
        oer_esp32s31_board::BootstrapPsram,
        oer_esp32s31_compose::phy_fault::report_previous,
        core::convert::identity,
    ) {
        Ok(adopted) => adopted.into_parts(),
        Err(error) => fail(error.name().as_str()),
    };
    gate_fault::report_previous();
    release_fault::report_previous();
    if let Some(previous) = previous
        && usb_serial_jtag::early_text(b"OER-B0-BRINGUP: previous panic\r\n") {
        panic::clear_previous(&previous);
    }
    let _ = usb_serial_jtag::early_text(b"OER-B0-BRINGUP: unconfigured\r\n");
    oer_espressif_staged_runtime::idle()
}

fn fail(reason: &'static str) -> ! {
    let _ = usb_serial_jtag::early_text(b"OER-B0-BRINGUP: fail ");
    let _ = usb_serial_jtag::early_text(reason.as_bytes());
    let _ = usb_serial_jtag::early_text(b"\r\n");
    oer_espressif_staged_runtime::idle()
}
