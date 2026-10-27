// An unconfigured image has no tasks or enabled interrupt sources.
use oer_esp32s31_hal::root::Root;
use oer_esp32s31_hal::usb_serial_jtag;
use oer_espressif_staged_runtime::panic;

mod gate_fault;

oer_espressif_staged_runtime::interrupt_table! {
    contexts {}
}
oer_espressif_staged_runtime::entry!(main);

fn main(handoff: oer_espressif_staged_runtime::Handoff) -> ! {
    let previous = panic::take_previous();
    let Some(root) = Root::take() else {
        fail("root-taken");
    };
    let runtime = match root.into_runtime() {
        Ok(runtime) => runtime,
        Err(error) => fail(error.name()),
    };
    let (_psram, _adopted) = match handoff.adopt(
        INTERRUPT_TABLE,
        runtime.interrupts.controller,
        oer_esp32s31_board::BootstrapPsram,
    ) {
        Ok(adopted) => adopted,
        Err(error) => fail(error.name()),
    };
    gate_fault::report_previous();
    if previous.is_some() {
        let _ = usb_serial_jtag::early_text(b"OER-B0-BRINGUP: previous panic\r\n");
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
