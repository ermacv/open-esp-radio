//! One console line in a fixed buffer, written without `core::fmt`: text,
//! lowercase hexadecimal of a fixed width and unsigned decimal. A line that
//! would not fit is a typed error; nothing is truncated.

/// The buffer refused a byte: the line is longer than its capacity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LineFull;

pub(crate) struct Line<const N: usize> {
    bytes: [u8; N],
    length: usize,
}

const HEX: &[u8; 16] = b"0123456789abcdef";

impl<const N: usize> Line<N> {
    pub(crate) const fn new() -> Self {
        Self {
            bytes: [0; N],
            length: 0,
        }
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..self.length).unwrap_or_default()
    }

    fn byte(&mut self, byte: u8) -> Result<(), LineFull> {
        let slot = self.bytes.get_mut(self.length).ok_or(LineFull)?;
        *slot = byte;
        self.length = self.length.checked_add(1).ok_or(LineFull)?;
        Ok(())
    }

    pub(crate) fn text(&mut self, text: &[u8]) -> Result<&mut Self, LineFull> {
        for byte in text {
            self.byte(*byte)?;
        }
        Ok(self)
    }

    /// `digits` hexadecimal digits of `value`, most significant first.
    fn hex(&mut self, value: u64, digits: u32) -> Result<&mut Self, LineFull> {
        let mut digit = digits;
        while digit > 0 {
            digit = digit.wrapping_sub(1);
            let shift = digit.wrapping_mul(4);
            let nibble = value.checked_shr(shift).unwrap_or(0) & 0xf;
            let symbol = HEX.get(nibble as usize).copied().ok_or(LineFull)?;
            self.byte(symbol)?;
        }
        Ok(self)
    }

    pub(crate) fn hex8(&mut self, value: u8) -> Result<&mut Self, LineFull> {
        self.hex(u64::from(value), 2)
    }

    /// The bytes as hexadecimal pairs, first byte first.
    pub(crate) fn hex_bytes(&mut self, bytes: &[u8]) -> Result<&mut Self, LineFull> {
        for byte in bytes {
            self.hex8(*byte)?;
        }
        Ok(self)
    }

    pub(crate) fn decimal(&mut self, value: u64) -> Result<&mut Self, LineFull> {
        let mut digits = [0u8; 20];
        let mut count = 0usize;
        let mut rest = value;
        loop {
            let slot = digits.get_mut(count).ok_or(LineFull)?;
            *slot = b'0'.wrapping_add((rest % 10) as u8);
            count = count.wrapping_add(1);
            rest /= 10;
            if rest == 0 {
                break;
            }
        }
        while count > 0 {
            count = count.wrapping_sub(1);
            let digit = digits.get(count).copied().ok_or(LineFull)?;
            self.byte(digit)?;
        }
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_hex_and_decimal_are_written_in_order() {
        let mut line = Line::<64>::new();
        line.text(b"a=")
            .unwrap()
            .hex_bytes(&[0x01, 0x23, 0xab, 0xcd])
            .unwrap()
            .text(b" c=")
            .unwrap()
            .decimal(0)
            .unwrap()
            .text(b",")
            .unwrap()
            .decimal(u64::MAX)
            .unwrap()
            .text(b" d=")
            .unwrap()
            .hex_bytes(&[0, 0xff, 0x10])
            .unwrap();
        assert_eq!(
            line.as_bytes(),
            b"a=0123abcd c=0,18446744073709551615 d=00ff10"
        );
    }

    #[test]
    fn a_line_that_does_not_fit_is_refused_not_truncated_silently() {
        let mut line = Line::<4>::new();
        assert!(line.text(b"abcd").is_ok());
        assert_eq!(line.text(b"e").err(), Some(LineFull));
        assert_eq!(line.as_bytes(), b"abcd");
        let mut line = Line::<1>::new();
        assert_eq!(line.hex8(1).err(), Some(LineFull));
    }
}
