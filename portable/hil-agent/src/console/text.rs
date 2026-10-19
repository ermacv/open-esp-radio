//! Bounded text records.

use core::ffi::CStr;
use core::fmt::Write;

/// One bounded, NUL-terminated line of text.
#[derive(Clone, Copy)]
pub struct TextBuffer<const N: usize> {
    bytes: [u8; N],
    len: usize,
    truncated: bool,
}

impl<const N: usize> Default for TextBuffer<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> TextBuffer<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
            truncated: false,
        }
    }

    /// The text formatted from `args`, cut to fit.
    pub fn format(args: core::fmt::Arguments<'_>) -> Self {
        let mut text = Self::new();
        let _ = text.write_fmt(args);
        text
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    /// The text for C-string consumers such as a ROM `printf`.
    pub fn as_c_str(&self) -> &CStr {
        CStr::from_bytes_until_nul(&self.bytes).expect("the buffer keeps a terminating NUL")
    }

    pub const fn was_truncated(&self) -> bool {
        self.truncated
    }
}

impl<const N: usize> Write for TextBuffer<N> {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let available = self.bytes.len() - 1 - self.len;
        let length = text.len().min(available);
        self.bytes[self.len..self.len + length].copy_from_slice(&text.as_bytes()[..length]);
        self.len += length;
        self.truncated |= length != text.len();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::TextBuffer;
    use core::fmt::Write;

    #[test]
    fn text_buffer_keeps_space_for_nul() {
        let mut buffer = TextBuffer::<5>::new();
        write!(&mut buffer, "abcdef").unwrap();
        assert_eq!(buffer.as_bytes(), b"abcd");
        assert_eq!(buffer.as_c_str().to_bytes(), b"abcd");
        assert!(buffer.was_truncated());
    }

    #[test]
    fn exact_fit_is_not_truncated() {
        let mut buffer = TextBuffer::<5>::new();
        write!(&mut buffer, "abcd").unwrap();
        assert_eq!(buffer.as_bytes(), b"abcd");
        assert!(!buffer.was_truncated());
    }
}
