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
/// What a line prints: the numbers, `bool`, `&'static str`, [`Hex`],
/// [`Ipv4`], and every error that describes itself as a diagnostic path,
/// which ends the line.
pub use oer_diagnostics::LineValue;
/// The capacities a line is checked against when it compiles.
pub use oer_diagnostics::line::MAX_SEGMENTS;
pub use oer_diagnostics::note::{MAX_NAME, MAX_NAME_BYTES, MAX_NAMES};
/// How an error describes itself to a line: its variants' names and numbers.
pub use oer_diagnostics::{DiagPath, DiagnosticPath};

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

#[doc(hidden)]
pub use oer_diagnostics::Line as __Line;

/// The longer of two lengths, for a line's longest text piece.
#[doc(hidden)]
pub const fn __longest(a: usize, b: usize) -> usize {
    if a > b { a } else { b }
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

/// Write one line to the console, its static text pieces and values in
/// order: `println!("OER-WIFI ipv4 ", Ipv4(address), "/", prefix)`. Every
/// literal is a text piece, so a string literal; a value is any other
/// expression of a [`LineValue`] (a number literal in braces: `{ 7_u8 }`),
/// and an error (a diagnostic path) ends the line. It evaluates to whether the whole line
/// reached the console. The counts of segments and text are checked when
/// the line compiles; a value past the line's words, or an error's path past
/// its names, is left out and the host prints `line-truncated`.
///
/// ```compile_fail,E0080
/// let n = 1_u32;
/// // Nine segments: more than MAX_SEGMENTS.
/// let _ = oer_espressif::println!("a", n, "b", n, "c", n, "d", n, "e");
/// ```
///
/// ```
/// let n = 1_u32;
/// let _ = oer_espressif::println!("a", n, "b", n, "c", n, "d", n);
/// ```
#[macro_export]
macro_rules! println {
    ($($arg:tt)*) => {
        $crate::print::line($crate::__print_line!(
            [$crate::print::__Line::new()] [0] [0] [0] [0] $($arg)*
        ))
    };
}

/// One segment at a time: a literal is a piece, anything else a value.
/// The counters are the segments, the pieces, their bytes and the longest.
#[doc(hidden)]
#[macro_export]
macro_rules! __print_line {
    ([$line:expr] [$segments:expr] [$pieces:expr] [$bytes:expr] [$longest:expr]) => {{
        const {
            if $segments > $crate::print::MAX_SEGMENTS {
                panic!("a println! line has at most MAX_SEGMENTS pieces and values");
            }
            if $pieces > $crate::print::MAX_NAMES
                || $bytes > $crate::print::MAX_NAME_BYTES
                || $longest > $crate::print::MAX_NAME
            {
                panic!("a println! line's text pieces fit MAX_NAMES, MAX_NAME_BYTES and MAX_NAME");
            }
        };
        $line
    }};
    ([$line:expr] [$segments:expr] [$pieces:expr] [$bytes:expr] [$longest:expr]
        $text:literal $(, $($rest:tt)*)?) => {
        $crate::__print_line!(
            [$line.text($text)] [$segments + 1] [$pieces + 1] [$bytes + $text.len()]
            [$crate::print::__longest($longest, $text.len())] $($($rest)*)?
        )
    };
    ([$line:expr] [$segments:expr] [$pieces:expr] [$bytes:expr] [$longest:expr]
        $value:expr $(, $($rest:tt)*)?) => {
        $crate::__print_line!(
            [$line.value(&$value)] [$segments + 1] [$pieces] [$bytes] [$longest] $($($rest)*)?
        )
    };
}

#[cfg(test)]
mod tests {
    use crate::print::{Hex, Ipv4};

    #[test]
    fn a_line_carries_its_pieces_and_values_in_order() {
        let prefix = 24_u8;
        let line = crate::__print_line!(
            [crate::print::__Line::new()] [0] [0] [0] [0]
            "OER-WIFI ipv4 ", Ipv4([10, 0, 0, 2]), "/", prefix, " ", Hex(0x2a)
        );
        let note = line.note();
        assert_eq!(note.names(), ["OER-WIFI ipv4 ", "/", " "]);
        assert_eq!(note.words()[1..], [0x0a00_0002, 24, 0x2a]);
        assert!(!note.overflowed());
    }

    #[test]
    fn off_the_chip_no_line_reaches_a_console() {
        assert!(!crate::println!("OER-RESULT: pass"));
    }
}
