//! Wi-Fi fatal source observations, retained until complete successful output.
#[cfg(any(test, target_arch = "riscv32"))]
use oer_diagnostics::fault::Class;
use oer_diagnostics::fault::{Domain, Record};
use oer_espressif_staged_runtime::retained_fault;
oer_espressif_staged_runtime::retained_static! { static WIFI_FAULT: Record; }

#[cfg(target_arch = "riscv32")]
pub fn reset(raw: u32) -> ! {
    oer_espressif_staged_runtime::fatal::reset_recording::<oer_esp32s31_hal::platform::Port>(
        |claim, _| {
            if let Ok(record) = encode(raw) {
                let _committed = claim.commit(&WIFI_FAULT, record);
            }
        },
    )
}
#[cfg(any(test, target_arch = "riscv32"))]
fn encode(raw: u32) -> Result<Record, oer_diagnostics::fault::Error> {
    Record::encode(
        Class::UnexpectedHardware,
        Domain::Wifi,
        1,
        oer_diagnostics::fault::UNKNOWN_HART,
        0,
        &raw.to_le_bytes(),
    )
}
pub fn report_previous() {
    if let Some(mut record) = retained_fault::peek(&WIFI_FAULT) {
        let _reported = report(&mut record, |bytes: &[u8]| {
            oer_esp32s31_hal::peripherals::usb_serial_jtag::early_text(bytes).is_whole()
        });
    }
}
fn report(record: &mut Record, mut write: impl FnMut(&[u8]) -> bool) -> Result<(), ()> {
    record
        .report(|view| {
            let bytes: [u8; 4] = view.decode(Domain::Wifi).map_err(|_| ())?;
            let mut raw = u32::from_le_bytes(bytes);
            let mut digits = [b'0'; 8];
            for digit in digits.iter_mut().rev() {
                let nibble = (raw & 0xf) as u8;
                *digit = if nibble < 10 {
                    b'0'.wrapping_add(nibble)
                } else {
                    b'a'.wrapping_add(nibble.wrapping_sub(10))
                };
                raw >>= 4;
            }
            for bytes in [
                b"previous Wi-Fi MAC fatal assertion RAW=0x".as_slice(),
                &digits,
                b"\n",
            ] {
                if !write(bytes) {
                    return Err(());
                }
            }
            Ok(())
        })
        .map_err(|_| ())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_refused_chunk_preserves_the_full_raw_word() {
        for raw in [
            0,
            1,
            0xf,
            0x10,
            0xff,
            0x100,
            0x1234abcd,
            0x80000000,
            u32::MAX,
        ] {
            for refused in 0..3 {
                let mut record = encode(raw).unwrap();
                let mut count = 0;
                assert!(
                    report(&mut record, |_| {
                        let accept = count != refused;
                        count += 1;
                        accept
                    })
                    .is_err()
                );
                let mut bytes = std::vec::Vec::new();
                report(&mut record, |part| {
                    bytes.extend_from_slice(part);
                    true
                })
                .unwrap();
                assert_eq!(
                    bytes,
                    std::format!("previous Wi-Fi MAC fatal assertion RAW={raw:#010x}\n").as_bytes()
                );
                assert!(record.peek().is_err());
            }
        }
    }
}
