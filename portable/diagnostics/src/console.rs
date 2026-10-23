//! Version-one synchronous line framing over the shared record codec.
//! This layer alone owns sequence, CRC, the console marker and hex escaping.

use crate::record::{self, MAX_PAYLOAD, PreviousPanic, Record};
use oer_trace::Event;

pub const PREFIX: &[u8] = b"OER-DIAG:01";

/// One synchronous CPU0 boot console. `write` must write all the supplied
/// bytes or return an error. A failed record consumes its sequence too, so
/// the next intact record exposes the loss. There is no IRQ/task logger.
pub struct Console<F> {
    write: F,
    sequence: u32,
}

impl<F, E> Console<F>
where
    F: FnMut(&[u8]) -> Result<(), E>,
{
    pub const fn new(write: F) -> Self {
        Self { write, sequence: 0 }
    }

    pub fn previous_panic(&mut self, panic: &PreviousPanic<'_>) -> Result<(), E> {
        // Preserve validation before starting a line or consuming a sequence.
        record::validate_previous_panic(panic);
        let mut crc = self.begin()?;
        record::encode_previous_panic(panic, |bytes| self.hex(bytes, &mut crc))?;
        self.end(crc)
    }

    pub fn event<T: Event>(&mut self, event: &T) -> Result<(), E> {
        let mut crc = self.begin()?;
        record::encode_event(event, |bytes| self.hex(bytes, &mut crc))?;
        self.end(crc)
    }

    fn begin(&mut self) -> Result<u16, E> {
        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);
        // Recover line framing after a partial failed write. Its consumed
        // sequence still exposes the loss to the host.
        (self.write)(b"\r\nOER-DIAG:01")?;
        let mut crc = 0xffff;
        self.hex(&sequence.to_le_bytes(), &mut crc)?;
        Ok(crc)
    }

    fn hex(&mut self, bytes: &[u8], crc: &mut u16) -> Result<(), E> {
        let mut escaped = [0; 16];
        for chunk in bytes.chunks(escaped.len() / 2) {
            for (&byte, [high, low]) in chunk.iter().zip(escaped.as_chunks_mut::<2>().0) {
                *crc = checksum(*crc, byte);
                let digit = |nibble: u8| {
                    if nibble < 10 {
                        b'0'.wrapping_add(nibble)
                    } else {
                        b'a'.wrapping_add(nibble.wrapping_sub(10))
                    }
                };
                *high = digit(byte >> 4);
                *low = digit(byte & 15);
            }
            // Each input chunk is at most eight bytes; the paired iterator
            // above filled exactly twice that many output bytes. Checked
            // access avoids retaining a console bounds-panic source location.
            if let Some(encoded) = escaped.get(..chunk.len() * 2) {
                (self.write)(encoded)?;
            }
        }
        Ok(())
    }

    fn end(&mut self, crc: u16) -> Result<(), E> {
        self.hex(&crc.to_le_bytes(), &mut 0)?;
        (self.write)(b"\r\n")
    }
}

/// CRC-16/CCITT-FALSE, no lookup table in target RAM or rodata.
fn checksum(mut crc: u16, byte: u8) -> u16 {
    crc ^= u16::from(byte) << 8;
    for _ in 0..8 {
        crc = (crc << 1) ^ if crc & 0x8000 != 0 { 0x1021 } else { 0 };
    }
    crc
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeError {
    Version,
    Truncated,
    Hex,
    Checksum,
    UnknownRecord,
    Payload,
}

/// Decode one complete console line (without CR/LF), borrowing the caller's
/// scratch space. Non-diagnostic lines return `Ok(None)`.
pub fn decode<'a>(
    line: &[u8],
    scratch: &'a mut [u8; MAX_PAYLOAD + 7],
) -> Result<Option<(u32, Record<'a>)>, DecodeError> {
    if !line.starts_with(b"OER-DIAG:") {
        return Ok(None);
    }
    let hex = line.strip_prefix(PREFIX).ok_or(DecodeError::Version)?;
    if hex.len() < 14 || hex.len() % 2 != 0 {
        return Err(DecodeError::Truncated);
    }
    if hex.len() / 2 > scratch.len() {
        return Err(DecodeError::Payload);
    }
    let raw = scratch
        .get_mut(..hex.len() / 2)
        .ok_or(DecodeError::Payload)?;
    for (byte, pair) in raw.iter_mut().zip(hex.chunks_exact(2)) {
        let [high, low] = pair else {
            return Err(DecodeError::Truncated);
        };
        let digit = |byte| match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            _ => Err(DecodeError::Hex),
        };
        *byte = digit(*high)? * 16 + digit(*low)?;
    }
    let (protected, crc) = raw.split_last_chunk::<2>().ok_or(DecodeError::Truncated)?;
    let expected = protected
        .iter()
        .fold(0xffff, |crc, &byte| checksum(crc, byte));
    if *crc != expected.to_le_bytes() {
        return Err(DecodeError::Checksum);
    }
    let (sequence, bytes) = protected
        .split_first_chunk::<4>()
        .ok_or(DecodeError::Truncated)?;
    let record = record::decode(bytes).map_err(|error| match error {
        record::DecodeError::UnknownRecord => DecodeError::UnknownRecord,
        record::DecodeError::Truncated | record::DecodeError::Payload => DecodeError::Payload,
    })?;
    Ok(Some((u32::from_le_bytes(*sequence), record)))
}

#[cfg(test)]
mod tests;
