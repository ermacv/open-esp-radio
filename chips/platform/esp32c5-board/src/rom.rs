//! The mask ROM's console output, which works without any driver: before
//! `esp_hal::init`, beside a USB driver, and on paths that must not take a
//! driver's lock.

use core::ffi::{CStr, c_char};
use core::fmt;

unsafe extern "C" {
    fn ets_printf(format: *const c_char, ...) -> i32;
}

/// Print `text` as it is.
pub fn print(text: &CStr) {
    // SAFETY: the format consumes exactly one NUL-terminated string, which
    // `text` is.
    unsafe { ets_printf(c"%s".as_ptr(), text.as_ptr()) };
}

/// Print `text` and a line end.
pub fn line(text: &CStr) {
    // SAFETY: as in `print`.
    unsafe { ets_printf(c"%s\r\n".as_ptr(), text.as_ptr()) };
}

/// Print formatted text through the ROM, in pieces of a small stack
/// buffer; a NUL byte in the text is left out.
pub fn print_fmt(arguments: fmt::Arguments<'_>) {
    let mut writer = Writer {
        buffer: [0; WRITER_BYTES],
        length: 0,
    };
    let _ = fmt::Write::write_fmt(&mut writer, arguments);
    writer.flush();
}

const WRITER_BYTES: usize = 64;

struct Writer {
    buffer: [u8; WRITER_BYTES],
    length: usize,
}

impl Writer {
    fn flush(&mut self) {
        if let Some(end) = self.buffer.get_mut(self.length) {
            *end = 0;
        }
        if let Ok(text) = CStr::from_bytes_until_nul(&self.buffer) {
            print(text);
        }
        self.length = 0;
    }
}

impl fmt::Write for Writer {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for &byte in text.as_bytes().iter().filter(|&&byte| byte != 0) {
            if self.length == WRITER_BYTES - 1 {
                self.flush();
            }
            if let Some(slot) = self.buffer.get_mut(self.length) {
                *slot = byte;
                self.length += 1;
            }
        }
        Ok(())
    }
}
