//! The mask ROM's console output, which works without any driver: before
//! `esp_hal::init`, beside a USB driver, and on paths that must not take a
//! driver's lock.

use core::ffi::{CStr, c_char};

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
