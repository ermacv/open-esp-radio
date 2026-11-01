//! The shared RF actor's fatal observations: its periodic PHY tracking
//! failed between requests, so no client answer carries the failure and
//! safe continuation is not established. The record keeps the fault's
//! class and its variant path (names, then numbers) as text, retained until
//! the next boot's complete output.
use oer_diagnostics::DiagPath;
use oer_diagnostics::fault::{Class, Domain, Record};
use oer_espressif_staged_runtime::retained_fault;
oer_espressif_staged_runtime::retained_static! { static RADIO_FAULT: Record; }

/// The record's reason: the periodic tracking failed.
const TRACKING: u32 = 1;

/// A copied observation of the failed periodic tracking: its class and
/// variant path, not a hardware owner or permission to continue. The actor
/// that returned it keeps the poisoned domain and every owner it holds; an
/// image consumes it through the platform's retained terminal path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the PHY may be half changed: record and reset"]
pub struct Failure {
    class: Class,
    path: DiagPath,
}

impl Failure {
    /// The failure of class `class` with `value`'s variant path.
    pub fn of(class: Class, value: &impl oer_diagnostics::DiagnosticPath) -> Self {
        Self {
            class,
            path: DiagPath::of(value),
        }
    }

    /// Record the failure for the next boot, then reset the chip.
    #[cfg(target_arch = "riscv32")]
    pub fn reset(self) -> ! {
        oer_espressif_staged_runtime::fatal::reset_recording::<oer_esp32s31_hal::platform::Port>(
            |claim, _stop| {
                if let Ok(record) = encode(self) {
                    let _committed = claim.commit(&RADIO_FAULT, record);
                }
            },
        )
    }
}

/// The variant path as text: the names joined by `/`, then each number as
/// ` 0x` and eight hex digits, then ` +` when the path kept less than its
/// value had. Text past the payload is cut, and its last two bytes become
/// ` +`.
#[cfg(any(test, target_arch = "riscv32"))]
fn text(path: &DiagPath) -> ([u8; oer_diagnostics::fault::PAYLOAD_BYTES], usize) {
    let mut text = [0; oer_diagnostics::fault::PAYLOAD_BYTES];
    let mut length = 0usize;
    let mut cut = false;
    let mut put = |bytes: &[u8]| {
        for byte in bytes {
            match text.get_mut(length) {
                Some(slot) => {
                    *slot = *byte;
                    length = length.saturating_add(1);
                }
                None => cut = true,
            }
        }
    };
    for (index, name) in path.names().iter().enumerate() {
        if index != 0 {
            put(b"/");
        }
        put(name.as_bytes());
    }
    for word in path.words() {
        put(b" 0x");
        put(&hex(*word));
    }
    if path.overflowed() {
        put(b" +");
    }
    if cut {
        let end = text.iter_mut().take(length).rev().take(2).rev();
        for (slot, byte) in end.zip(b" +") {
            *slot = *byte;
        }
    }
    (text, length)
}

/// `word` as eight lowercase hex digits.
#[cfg(any(test, target_arch = "riscv32"))]
fn hex(mut word: u32) -> [u8; 8] {
    let mut digits = [b'0'; 8];
    for digit in digits.iter_mut().rev() {
        let nibble = (word & 0xf) as u8;
        *digit = if nibble < 10 {
            b'0'.wrapping_add(nibble)
        } else {
            b'a'.wrapping_add(nibble.wrapping_sub(10))
        };
        word >>= 4;
    }
    digits
}

/// The class's registry spelling.
const fn class_name(class: Class) -> &'static [u8] {
    match class {
        Class::HardwareImpossible => b"hardware-impossible",
        Class::UnexpectedHardware => b"unexpected-hardware",
        Class::SoftwareInvariant => b"software-invariant",
        Class::ConfigurationBoot => b"configuration-boot",
        Class::Exhaustion => b"exhaustion",
        Class::Cancellation => b"cancellation",
        Class::CpuException => b"cpu-exception",
        Class::RequestedReset => b"requested-reset",
        Class::RustPanic => b"rust-panic",
    }
}

#[cfg(any(test, target_arch = "riscv32"))]
fn encode(failure: Failure) -> Result<Record, oer_diagnostics::fault::Error> {
    let (text, length) = text(&failure.path);
    Record::new(
        failure.class,
        Domain::Phy,
        TRACKING,
        oer_diagnostics::fault::UNKNOWN_HART,
        0,
        text.get(..length).unwrap_or(&[]),
    )
}

/// Print the previous boot's record once its whole line was written.
pub fn report_previous() {
    if let Some(mut record) = retained_fault::peek(&RADIO_FAULT) {
        let _reported = report(&mut record, |bytes: &[u8]| {
            oer_esp32s31_hal::peripherals::usb_serial_jtag::early_text(bytes).is_whole()
        });
    }
}

fn report(record: &mut Record, mut write: impl FnMut(&[u8]) -> bool) -> Result<(), ()> {
    record
        .report(|view| {
            if view.domain != Domain::Phy as u32 || view.reason != TRACKING {
                return Err(());
            }
            for bytes in [
                b"previous radio tracking fault class=".as_slice(),
                class_name(view.class),
                b" path=",
                view.payload,
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

    struct Path;
    impl oer_diagnostics::DiagnosticPath for Path {
        fn push_path(&self, path: &mut DiagPath) {
            for name in ["Tick", "CalibrationTracking", "Step", "EdgeTimedOut"] {
                path.name(name);
            }
            path.word(0xdead_beef);
            path.word(7);
        }
    }

    #[test]
    fn the_record_keeps_the_class_and_path_until_its_whole_line_is_written() {
        let failure = Failure::of(Class::UnexpectedHardware, &Path);
        let mut record = encode(failure).unwrap();
        let mut count = 0;
        assert!(
            report(&mut record, |_| {
                count += 1;
                count != 3
            })
            .is_err()
        );
        let mut line = std::vec::Vec::new();
        report(&mut record, |bytes| {
            line.extend_from_slice(bytes);
            true
        })
        .unwrap();
        assert_eq!(
            std::str::from_utf8(&line).unwrap(),
            "previous radio tracking fault class=unexpected-hardware \
             path=Tick/CalibrationTracking/Step/EdgeTimedOut 0xdeadbeef 0x00000007\n"
        );
        assert!(record.peek().is_err());
    }

    #[test]
    fn a_path_that_kept_less_says_so() {
        let mut path = DiagPath::new();
        path.name("CommonPower");
        path.name("Release");
        path.truncated();
        let (text, length) = text(&path);
        assert_eq!(&text[..length], b"CommonPower/Release +");
        let mut long = DiagPath::new();
        for _ in 0..6 {
            long.name("AVeryLongVariantNameThatTheRecordMustCutAtItsEnd");
        }
        let (cut, length) = super::text(&long);
        assert_eq!(length, oer_diagnostics::fault::PAYLOAD_BYTES);
        assert!(cut.ends_with(b" +"));
    }
}
