//! Lines on the console without formatting on the target: the early writer,
//! which waits for room within its budget per record and counts what it
//! drops. [`println!`](crate::println) writes one application line
//! ([`oer_diagnostics::Line`]): its static text pieces and its values in
//! order, which the host prints as text. The target builds no text and calls
//! no formatter.

/// A number the line prints as `0x`-prefixed hexadecimal.
pub use oer_diagnostics::Hex;
/// An address the line prints dotted.
pub use oer_diagnostics::Ipv4;
/// What a line prints: the numbers, `bool`, [`Hex`], [`Ipv4`], and every
/// error that describes itself as a diagnostic path, which ends the line.
pub use oer_diagnostics::LineValue;
/// How an error describes itself to a line: its variants' names and numbers.
pub use oer_diagnostics::{DiagPath, DiagnosticPath};

#[doc(hidden)]
pub use oer_diagnostics as __diagnostics;

/// Write one line, whole under one hold of the block; whether all of it
/// reached the console.
#[doc(hidden)]
#[cfg(all(feature = "esp32s31", target_arch = "riscv32"))]
pub fn line(line: oer_diagnostics::Line) -> bool {
    oer_esp32s31_hal::peripherals::usb_serial_jtag::early_note(&line.note()).is_whole()
}

/// Off the chip there is no console.
#[doc(hidden)]
#[cfg(not(all(feature = "esp32s31", target_arch = "riscv32")))]
pub fn line(_: oer_diagnostics::Line) -> bool {
    false
}

/// Write `bytes` whole, or fail.
#[cfg(all(feature = "esp32s31", target_arch = "riscv32"))]
pub(crate) fn whole(bytes: &[u8]) -> Result<(), ()> {
    if oer_esp32s31_hal::peripherals::usb_serial_jtag::early_text(bytes).is_whole() {
        Ok(())
    } else {
        Err(())
    }
}

/// Write one line to the console: `println!("OER-WIFI ipv4 {}/{}",
/// Ipv4(address), prefix)`. Each `{}` takes the next value, a
/// [`LineValue`]: [`Hex`] and [`Ipv4`] choose a value's text, and an error
/// (a [`DiagnosticPath`]) is the last value, with no text after it. A hole
/// takes no options. The target sends the format's pieces and the values;
/// the host prints the line. It evaluates to whether the whole line reached
/// the console.
///
/// The format, the count of values, the words they take and the text are
/// checked when the line compiles
/// ([`oer_diagnostics::typed_line!`]). Only an error's path has a length
/// its type does not give: what of it does not fit is left out, and the
/// host prints `line-truncated`.
///
/// ```compile_fail,E0080
/// let error = 7_u8;
/// // A hole with options.
/// let _ = oer_espressif::println!("OER-WIFI connect-failed {error:?}");
/// ```
///
/// ```
/// let channel = 6_u8;
/// let _ = oer_espressif::println!("OER-WIFI connected channel={}", channel);
/// ```
#[macro_export]
macro_rules! println {
    ($($arg:tt)*) => {
        $crate::print::line($crate::print::__diagnostics::typed_line!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use crate::print::{Hex, Ipv4};

    #[test]
    fn a_line_carries_its_pieces_and_values_in_order() {
        let prefix = 24_u8;
        let note = crate::print::__diagnostics::typed_line!(
            "OER-WIFI ipv4 {}/{} {}",
            Ipv4([10, 0, 0, 2]),
            prefix,
            Hex(0x2a)
        )
        .note();
        assert_eq!(note.names(), ["OER-WIFI ipv4 ", "/", " "]);
        assert_eq!(note.words()[1..], [0x0a00_0002, 24, 0x2a]);
        assert!(!note.overflowed());
    }

    #[test]
    fn off_the_chip_no_line_reaches_a_console() {
        assert!(!crate::println!("OER-RESULT: pass"));
    }
}
