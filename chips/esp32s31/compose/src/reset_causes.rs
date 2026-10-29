//! Boot-time raw reset-cause reporting alongside retained diagnostics.
use oer_esp32s31_hal::{
    peripherals::reset_cause::{ResetCauseRegisters, ResetCauses},
    platform::Hart,
};

/// Consume the root's reset-cause owner once, report every hart, and retain the
/// value snapshot for HIL evidence. Output refusal is best effort.
pub fn read(registers: ResetCauseRegisters) -> ResetCauses {
    let causes = ResetCauses::read(registers);
    let _reported = report(causes, |bytes| {
        oer_esp32s31_hal::peripherals::usb_serial_jtag::early_text(bytes)
            .then_some(())
            .ok_or(())
    });
    causes
}
fn report<E>(causes: ResetCauses, mut emit: impl FnMut(&[u8]) -> Result<(), E>) -> Result<(), E> {
    for hart in Hart::ALL {
        oer_espressif_staged_runtime::diagnostics::reset_cause(
            hart.get() as u32,
            u32::from(causes.of(hart).code()),
            &mut emit,
        )?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    #[test]
    fn fake_reset_registers_report_every_hart_including_unknown_causes() {
        use oer_esp32s31_hal::peripherals::reset_cause::{ResetCauseRegisters, ResetCauses};
        // Fill the owned test bank: 0x7f keeps all six raw cause bits set.
        let mut words = vec![0x7f; 1024];
        let causes = ResetCauses::read(ResetCauseRegisters::host(&mut words).unwrap());
        let mut bytes = Vec::new();
        super::report(causes, |part| {
            bytes.extend_from_slice(part);
            Ok::<_, ()>(())
        })
        .unwrap();
        let output = String::from_utf8(bytes).unwrap();
        assert_eq!(
            output.lines().count(),
            oer_esp32s31_hal::platform::Hart::ALL.len()
        );
        assert!(output.contains("hart=00000000 raw=0000003f"));
        assert!(output.contains("hart=00000001 raw=0000003f"));
        assert_eq!(super::report(causes, |_| Err(9)), Err(9));
    }
}
