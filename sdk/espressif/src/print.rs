//! Text on the console: the early writer, which waits for room within its
//! budget per record and counts what it drops. [`println!`](crate::println)
//! formats one line of at most [`LINE`] bytes; a longer one is cut at the
//! limit and marked with `…`.

use core::fmt::Write as _;

/// The longest line [`println!`](crate::println) writes.
pub const LINE: usize = 160;

/// Format and write one line; whether all of it reached the console.
#[doc(hidden)]
pub fn line(args: core::fmt::Arguments<'_>) -> bool {
    whole(render(args).as_bytes()).is_ok()
}

/// One line of at most [`LINE`] bytes, newline included.
fn render(args: core::fmt::Arguments<'_>) -> heapless::String<LINE> {
    let mut text = Truncating(heapless::String::new(), false);
    let _ = text.write_fmt(args);
    let Truncating(mut text, cut) = text;
    let mark = if cut { "\u{2026}\n" } else { "\n" };
    while text.len().saturating_add(mark.len()) > LINE {
        text.pop();
    }
    let _ = text.push_str(mark);
    text
}

/// A line that keeps what fits.
struct Truncating(heapless::String<LINE>, bool);
impl core::fmt::Write for Truncating {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for c in s.chars() {
            if self.0.push(c).is_err() {
                self.1 = true;
                break;
            }
        }
        Ok(())
    }
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

/// Off the chip there is no console.
#[cfg(not(all(feature = "esp32s31", target_arch = "riscv32")))]
pub(crate) fn whole(_: &[u8]) -> Result<(), ()> {
    Err(())
}

/// Write one formatted line to the console: `println!("ip {}", address)`.
/// It evaluates to whether the whole line reached the console.
#[macro_export]
macro_rules! println {
    ($($arg:tt)*) => {
        $crate::print::line(::core::format_args!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_ends_with_a_newline_and_a_long_one_is_cut_and_marked() {
        assert_eq!(render(format_args!("ip {}", 7)).as_str(), "ip 7\n");
        let long = render(format_args!("{:200}", "x"));
        assert!(long.len() <= LINE);
        assert!(long.ends_with("\u{2026}\n"));
        assert!(long.starts_with('x'));
    }
}
