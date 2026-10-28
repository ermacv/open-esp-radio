//! Retained Wi-Fi fatal assertion diagnostics for staged Wi-Fi images.
//!
//! The driver quarantines both banks and lends the captured RAW word to the
//! image. This module owns the record, the never-returning reset path and
//! next-boot reporting. Neither register nor DMA ownership is released here.

const MAGIC: u32 = 0x5749_4641;

/// Twelve retained bytes. Magic and checksum validate stored values at boot;
/// ordinary stores are not a transactional or power-fail commit protocol.
/// The software-reset path orders completed writes before triggering reset.
#[derive(Clone, Copy, bytemuck::AnyBitPattern)]
#[repr(C)]
struct RetainedFault {
    magic: u32,
    raw: u32,
    checksum: u32,
}

impl RetainedFault {
    fn record(&mut self, raw: u32) {
        self.magic = 0;
        self.raw = raw;
        self.checksum = checksum(raw);
        self.magic = MAGIC;
    }

    fn take(&mut self) -> Option<u32> {
        let valid = self.magic == MAGIC && self.checksum == checksum(self.raw);
        self.magic = 0;
        valid.then_some(self.raw)
    }
}

const fn checksum(raw: u32) -> u32 {
    (MAGIC ^ raw).wrapping_mul(0x0100_0193)
}

crate::retained_static! {
    static WIFI_FAULT: RetainedFault;
}

/// Record a Wi-Fi fatal assertion and reset with all owners retained.
/// The caller has quarantined its banks; this path never returns to the
/// preempted DMA owners or drops them before reset.
#[cfg(target_arch = "riscv32")]
pub fn reset(raw: u32) -> ! {
    crate::fatal::reset_recording(|_stop| {
        if let Some(mut record) = WIFI_FAULT.try_lock() {
            record.record(raw);
        }
    })
}

/// Consume and report the previous boot's Wi-Fi fault, after
/// `Root::into_runtime` installs the early console and before it retires.
pub fn report_previous() {
    if let Some(raw) = WIFI_FAULT.try_lock().and_then(|mut record| record.take()) {
        report(raw, |bytes| {
            let _written = oer_esp32s31_hal::usb_serial_jtag::early_text(bytes);
        });
    }
}

fn report(mut raw: u32, mut write: impl FnMut(&[u8])) {
    let mut digits = [b'0'; 8];
    for digit in digits.iter_mut().rev() {
        let nibble = (raw & 0xf) as u8;
        *digit = if nibble < 10 {
            b'0'.wrapping_add(nibble)
        } else {
            b'a'.wrapping_add(nibble - 10)
        };
        raw >>= 4;
    }
    write(b"previous Wi-Fi MAC fatal assertion RAW=0x");
    write(&digits);
    write(b"\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_fault_preserves_full_raw_and_reports_once() {
        let mut record: RetainedFault = bytemuck::Zeroable::zeroed();
        assert_eq!(record.take(), None);
        let raw = 0xa582_1357;
        record.record(raw);
        assert_eq!(record.take(), Some(raw));
        assert_eq!(record.take(), None);
        let mut output = std::vec::Vec::new();
        report(raw, |bytes| output.extend_from_slice(bytes));
        assert_eq!(
            output,
            b"previous Wi-Fi MAC fatal assertion RAW=0xa5821357\n"
        );
    }

    #[test]
    fn raw_output_preserves_hex_width_case_and_newline() {
        for raw in [
            0,
            1,
            0xf,
            0x10,
            0xff,
            0x100,
            0x1234_abcd,
            0x8000_0000,
            u32::MAX,
        ] {
            let mut output = std::vec::Vec::new();
            report(raw, |bytes| output.extend_from_slice(bytes));
            assert_eq!(
                output,
                std::format!("previous Wi-Fi MAC fatal assertion RAW={raw:#010x}\n").as_bytes()
            );
        }
    }

    #[test]
    fn invalid_values_are_rejected() {
        let mut record: RetainedFault = bytemuck::Zeroable::zeroed();
        record.record(0x1234_abcd);
        record.raw ^= 1;
        assert_eq!(record.take(), None);
        record.record(0x1234_abcd);
        record.magic = 0;
        assert_eq!(record.take(), None);
    }
}
