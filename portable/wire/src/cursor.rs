use crate::Error;

/// A bounded borrowed byte stream. A failed read retains the current position.
#[derive(Clone, Copy, Debug)]
pub struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
    pub const fn remaining(&self) -> &'a [u8] {
        self.bytes
    }
    pub const fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
    pub const fn take(&mut self, length: usize) -> Result<&'a [u8], Error> {
        let Some((taken, rest)) = self.bytes.split_at_checked(length) else {
            return Err(Error::Truncated {
                required: length,
                remaining: self.bytes.len(),
            });
        };
        self.bytes = rest;
        Ok(taken)
    }
    pub const fn array<const N: usize>(&mut self) -> Result<&'a [u8; N], Error> {
        let Some((taken, rest)) = self.bytes.split_first_chunk::<N>() else {
            return Err(Error::Truncated {
                required: N,
                remaining: self.bytes.len(),
            });
        };
        self.bytes = rest;
        Ok(taken)
    }
    pub const fn read<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        match self.array() {
            Ok(value) => Ok(*value),
            Err(error) => Err(error),
        }
    }
    pub const fn u8(&mut self) -> Result<u8, Error> {
        match self.read() {
            Ok([value]) => Ok(value),
            Err(error) => Err(error),
        }
    }
    pub const fn u16_le(&mut self) -> Result<u16, Error> {
        match self.read() {
            Ok(bytes) => Ok(u16::from_le_bytes(bytes)),
            Err(error) => Err(error),
        }
    }
    pub const fn u16_be(&mut self) -> Result<u16, Error> {
        match self.read() {
            Ok(bytes) => Ok(u16::from_be_bytes(bytes)),
            Err(error) => Err(error),
        }
    }
}

/// A bounded mutable byte stream. Each write admits its complete input before
/// modifying the destination; a failed write retains the current position.
pub struct Writer<'a> {
    bytes: &'a mut [u8],
    written: usize,
}

impl<'a> Writer<'a> {
    pub fn new(bytes: &'a mut [u8]) -> Self {
        Self { bytes, written: 0 }
    }
    pub const fn written(&self) -> usize {
        self.written
    }
    pub const fn remaining(&self) -> usize {
        self.bytes.len()
    }
    pub fn write(&mut self, input: &[u8]) -> Result<(), Error> {
        if input.len() > self.bytes.len() {
            return Err(Error::Truncated {
                required: input.len(),
                remaining: self.bytes.len(),
            });
        }
        let bytes = core::mem::take(&mut self.bytes);
        let (output, rest) = bytes.split_at_mut(input.len());
        output.copy_from_slice(input);
        self.bytes = rest;
        // Both counts are portions of the original slice, so this cannot overflow.
        self.written += input.len();
        Ok(())
    }
    pub fn u8(&mut self, value: u8) -> Result<(), Error> {
        self.write(&[value])
    }
    pub fn u16_le(&mut self, value: u16) -> Result<(), Error> {
        self.write(&value.to_le_bytes())
    }
    pub fn u16_be(&mut self, value: u16) -> Result<(), Error> {
        self.write(&value.to_be_bytes())
    }
}
