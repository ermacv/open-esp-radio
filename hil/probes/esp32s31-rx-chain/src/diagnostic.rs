//! A pre-RX fault survives reset without taking the live console's owner.
use core::sync::atomic::{AtomicU32, Ordering};
use oer_espressif_staged_runtime::{self as runtime, trap::TrapFrame};

use oer_diagnostics::fault::{Class, Domain, Record};
use runtime::retained_fault::{self, Claim};
runtime::retained_static! { static FAULT: Record; }
runtime::placed_static! {
    static PHASE: AtomicU32 = AtomicU32::new(0), in critical_data "rx_chain_phase";
}

pub(crate) fn phase(value: u32) {
    PHASE.store(value, Ordering::Relaxed);
}

pub(crate) fn exception(
    claim: &Claim<'_>,
    frame: &TrapFrame,
    _: Result<&oer_esp32s31_hal::cores::StopTheWorld<'_>, oer_esp32s31_hal::cores::StopError>,
) {
    record(claim, 1, frame.mcause, frame.mepc, frame.mtval, frame.ra);
}
/// The image's panic hook, which its entry names.
pub(crate) fn panic(
    claim: &Claim<'_>,
    _: &runtime::panic::PanicDetails<'_>,
    _: Result<&oer_esp32s31_hal::cores::StopTheWorld<'_>, oer_esp32s31_hal::cores::StopError>,
) {
    let registers = runtime::trap::TrapRegisters::read();
    record(
        claim,
        2,
        registers.mcause as u32,
        registers.mepc as u32,
        registers.mtval as u32,
        0,
    );
}
fn record(claim: &Claim<'_>, kind: u32, cause: u32, pc: u32, mtval: u32, ra: u32) {
    let words = [
        kind,
        PHASE.load(Ordering::Relaxed),
        oer_riscv::hart_id() as u32,
        cause,
        pc,
        mtval,
        ra,
        oer_riscv::read_mscratch() as u32,
    ];
    let class = if kind == 1 {
        Class::CpuException
    } else {
        Class::RustPanic
    };
    if let Ok(record) = Record::encode(
        class,
        Domain::RxChain,
        kind,
        oer_riscv::hart_id() as u32,
        PHASE.load(Ordering::Relaxed),
        &words,
    ) {
        let _committed = claim.commit(&FAULT, record);
    }
}

pub(crate) fn report_previous() {
    if let Some(previous) = runtime::panic::peek_previous() {
        let _ = runtime::diagnostics::emit_previous_panic(
            |bytes| {
                if oer_esp32s31_hal::peripherals::usb_serial_jtag::early_text(bytes).is_whole() {
                    Ok(())
                } else {
                    Err(())
                }
            },
            &previous,
        );
    }
    let Some(mut fault) = retained_fault::peek(&FAULT) else {
        return;
    };
    let Ok(view) = fault.peek() else {
        return;
    };
    let Ok([kind, phase, hart, cause, pc, tval, ra, mscratch]) =
        view.decode::<[u32; 8]>(Domain::RxChain)
    else {
        return;
    };
    let _reported = fault.report(|_| {
        let note = if kind == 1 {
            oer_diagnostics::Note::fatal_exception(hart, cause, pc, tval, ra, mscratch)
        } else {
            oer_diagnostics::Note::fatal_panic(hart, phase, cause, pc, tval)
        };
        if !oer_esp32s31_hal::peripherals::usb_serial_jtag::early_note(&note).is_whole() {
            return Err(());
        }
        use core::fmt::Write as _;
        let mut line = heapless::String::<80>::new();
        writeln!(&mut line, "RXCHAIN previous kind={kind} phase={phase}").map_err(|_| ())?;
        if !oer_esp32s31_hal::peripherals::usb_serial_jtag::early_text(line.as_bytes()).is_whole() {
            return Err(());
        }
        oer_esp32s31_hal::peripherals::usb_serial_jtag::early_text(
            b"OER-RESULT: fail rx-chain previous-fault\n",
        )
        .is_whole()
        .then_some(())
        .ok_or(())
    });
    drop(fault);
    runtime::fatal_halt::<oer_esp32s31_hal::platform::Port>(|_, _| {});
}
