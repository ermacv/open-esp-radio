//! The mask ROM's console output, which works without any driver: before
//! `esp_hal::init`, beside a USB driver, and on paths that must not take a
//! driver's lock.

use core::ffi::{CStr, c_char};
use oer_diagnostics::{Note, emit_note};

unsafe extern "C" {
    fn ets_install_usb_printf();
    fn ets_printf(format: *const c_char, ...) -> i32;
}

/// Route the ROM's console output to the USB Serial/JTAG endpoint.
pub fn install_usb_printf() {
    // SAFETY: the ROM function takes no arguments and only selects the
    // ROM's output channel.
    unsafe { ets_install_usb_printf() };
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

/// Print `note` as one `OER-DIAG:01` console line, in pieces of a small stack
/// buffer; the host decodes and renders it. The boot's shared sequence lets the
/// host see a lost or reordered one.
pub fn note(note: &Note<'_>) {
    let mut sink = Sink {
        buffer: [0; SINK_BYTES],
        length: 0,
    };
    let Ok(()) = emit_note(note, |bytes: &[u8]| {
        sink.push(bytes);
        Ok::<(), core::convert::Infallible>(())
    });
    sink.flush();
}

const SINK_BYTES: usize = 64;

struct Sink {
    buffer: [u8; SINK_BYTES],
    length: usize,
}

impl Sink {
    fn push(&mut self, bytes: &[u8]) {
        for &byte in bytes.iter().filter(|&&byte| byte != 0) {
            if self.length == SINK_BYTES - 1 {
                self.flush();
            }
            if let Some(slot) = self.buffer.get_mut(self.length) {
                *slot = byte;
                self.length += 1;
            }
        }
    }

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
