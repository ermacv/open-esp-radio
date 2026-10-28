// An unconfigured image has no tasks or enabled interrupt sources.
use oer_esp32s31_hal::root::Root;
use oer_esp32s31_hal::peripherals::usb_serial_jtag;
use oer_espressif_staged_runtime::{diagnostics, panic};

oer_espressif_staged_runtime::interrupt_table! {
    contexts {}
}
oer_espressif_staged_runtime::entry!(main, oer_esp32s31_hal::platform::Port);

fn main(handoff: oer_espressif_staged_runtime::Handoff<oer_esp32s31_hal::platform::Port>) -> ! {
    let previous = panic::take_previous::<oer_esp32s31_hal::platform::Port>();
    let Some(root) = Root::take() else {
        fail(oer_diagnostics::name!("root-taken"; 32));
    };
    let runtime = match root.into_runtime() {
        Ok(runtime) => runtime,
        Err(error) => fail(error.name()),
    };
    oer_esp32s31_compose::wifi_fault::report_previous();
    let (_psram, _adopted) = match handoff.adopt(
        INTERRUPT_TABLE,
        runtime.interrupts.controller,
        oer_esp32s31_board::BootstrapPsram,
        oer_esp32s31_compose::phy_fault::report_previous,
    ) {
        Ok(adopted) => adopted,
        Err(error) => fail(error.name()),
    };
    if let Some(record) = previous {
        let _ = diagnostics::emit_previous_panic(
            |bytes| {
                if usb_serial_jtag::early_text(bytes) {
                    Ok(())
                } else {
                    Err(())
                }
            },
            &record,
        );
    }
    fail(oer_diagnostics::name!("unconfigured"; 32))
}

fn fail<const B: usize>(reason: oer_diagnostics::Name<B>) -> ! {
    let _ = usb_serial_jtag::early_text(b"OER-RESULT: fail w2 ");
    let _ = usb_serial_jtag::early_text(reason.as_str().as_bytes());
    let _ = usb_serial_jtag::early_text(b"\r\n");
    oer_espressif_staged_runtime::idle()
}
