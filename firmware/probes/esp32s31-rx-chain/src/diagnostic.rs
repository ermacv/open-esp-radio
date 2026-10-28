//! A pre-RX fault survives reset without taking the live console's owner.
use core::sync::atomic::{AtomicU32, Ordering};
use oer_espressif_staged_runtime::{self as runtime, trap::TrapFrame};

const MAGIC: u32 = 0x5258_4631;
runtime::retained_static! { static FAULT: [u32; 9]; }
runtime::placed_static! {
    static PHASE: AtomicU32 = AtomicU32::new(0), in critical_data "rx_chain_phase";
}

pub(crate) fn phase(value: u32) {
    PHASE.store(value, Ordering::Relaxed);
}

runtime::exception_handler!(exception);
fn exception(frame: &TrapFrame) -> ! {
    runtime::reset_recording::<oer_esp32s31_hal::platform::Port>(|_| {
        record(1, frame.mcause, frame.mepc, frame.ra)
    })
}
/// The image's panic hook, which its entry names.
pub(crate) fn panic(
    _: &runtime::panic::PanicDetails<'_>,
    _: Result<&oer_esp32s31_hal::cores::StopTheWorld<'_>, oer_esp32s31_hal::cores::StopError>,
) {
    let registers = runtime::trap::TrapRegisters::read();
    record(2, registers.mcause as u32, registers.mepc as u32, 0);
}
fn record(kind: u32, cause: u32, pc: u32, ra: u32) {
    if let Some(mut fault) = FAULT.try_lock() {
        *fault = [
            MAGIC,
            kind,
            PHASE.load(Ordering::Relaxed),
            oer_riscv::hart_id() as u32,
            cause,
            pc,
            oer_riscv::read_mtval() as u32,
            ra,
            oer_riscv::read_mscratch() as u32,
        ];
    }
}

pub(crate) fn report_previous() {
    if let Some(previous) = runtime::panic::take_previous::<oer_esp32s31_hal::platform::Port>() {
        let _ = runtime::diagnostics::emit_previous_panic(
            |bytes| {
                if oer_esp32s31_hal::peripherals::usb_serial_jtag::early_text(bytes) {
                    Ok(())
                } else {
                    Err(())
                }
            },
            &previous,
        );
    }
    let Some(mut fault) = FAULT.try_lock() else {
        return;
    };
    let [magic, kind, phase, hart, cause, pc, tval, ra, mscratch] = *fault;
    *fault = [0; 9];
    drop(fault);
    if magic != MAGIC {
        return;
    }
    let note = if kind == 1 {
        oer_diagnostics::Note::fatal_exception(hart, cause, pc, tval, ra, mscratch)
    } else {
        oer_diagnostics::Note::fatal_panic(hart, phase, cause, pc, tval)
    };
    let _ = oer_esp32s31_hal::peripherals::usb_serial_jtag::early_note(&note);
    use core::fmt::Write as _;
    let mut line = heapless::String::<80>::new();
    if writeln!(&mut line, "RXCHAIN previous kind={kind} phase={phase}").is_ok() {
        let _ = oer_esp32s31_hal::peripherals::usb_serial_jtag::early_text(line.as_bytes());
    }
    let _ = oer_esp32s31_hal::peripherals::usb_serial_jtag::early_text(
        b"OER-RESULT: fail rx-chain previous-fault\n",
    );
    runtime::fatal_halt::<oer_esp32s31_hal::platform::Port>(|_| {});
}
