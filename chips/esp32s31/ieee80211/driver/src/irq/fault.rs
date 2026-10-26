//! Small retained diagnostic for the image's terminal Wi-Fi interrupt path.
//!
//! The image places one record in retained memory, writes it under
//! `fatal::reset_recording`, and reports it through its early console next boot.
//! No peripheral authority or DMA owner is released on this path.

use core::fmt::{self, Write};

use super::Fault;

const MAGIC: u32 = 0x5749_4641;

/// Twelve retained bytes per image. Every bit pattern is a valid stored value;
/// a magic and checksum reject power-on contents and interrupted writes.
#[derive(Clone, Copy, bytemuck::AnyBitPattern)]
#[repr(C)]
pub struct RetainedFault {
    magic: u32,
    status: u32,
    checksum: u32,
}

impl RetainedFault {
    pub fn record(&mut self, fault: Fault) {
        let Fault::HardwareAssertion { status } = fault;
        self.magic = 0;
        self.status = status;
        self.checksum = checksum(status);
        self.magic = MAGIC;
    }

    /// Consume one valid previous-boot record before opening the console.
    pub fn take(&mut self) -> Option<Fault> {
        let valid = self.magic == MAGIC && self.checksum == checksum(self.status);
        self.magic = 0;
        valid.then_some(Fault::HardwareAssertion {
            status: self.status,
        })
    }
}

const fn checksum(status: u32) -> u32 {
    (MAGIC ^ status).wrapping_mul(0x0100_0193)
}

/// Render the retained reason at boot, outside interrupt context.
pub fn report(fault: Fault, write: impl FnMut(&[u8])) {
    struct Output<F>(F);
    impl<F: FnMut(&[u8])> Write for Output<F> {
        fn write_str(&mut self, value: &str) -> fmt::Result {
            (self.0)(value.as_bytes());
            Ok(())
        }
    }
    let Fault::HardwareAssertion { status } = fault;
    let _written = writeln!(
        Output(write),
        "previous Wi-Fi MAC hardware assertion STATUS={status:#010x}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_fault_preserves_full_status_and_reports_once() {
        let mut record: RetainedFault = bytemuck::Zeroable::zeroed();
        assert_eq!(record.take(), None);
        let fault = Fault::HardwareAssertion {
            status: 0xa582_1357,
        };
        record.record(fault);
        assert_eq!(record.take(), Some(fault));
        assert_eq!(record.take(), None);
        let mut output = std::vec::Vec::new();
        report(fault, |bytes| output.extend_from_slice(bytes));
        assert_eq!(
            output,
            b"previous Wi-Fi MAC hardware assertion STATUS=0xa5821357\n"
        );
    }

    #[test]
    fn damaged_or_uncommitted_records_are_rejected() {
        let fault = Fault::HardwareAssertion {
            status: 0x1234_abcd,
        };
        let mut record: RetainedFault = bytemuck::Zeroable::zeroed();
        record.record(fault);
        record.status ^= 1;
        assert_eq!(record.take(), None);
        record.record(fault);
        record.magic = 0;
        assert_eq!(record.take(), None);
    }
}
