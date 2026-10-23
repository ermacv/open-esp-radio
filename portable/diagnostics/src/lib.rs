#![no_std]
#![forbid(unsafe_code)]

//! Boot-only diagnostics on the existing line console. The target writes
//! typed fields, hex escaped for line readers; only the host renders text.
//! No retained storage, queue, clock, or global transport is installed.

use oer_trace::{Channel, Domain, Event, Kind};

pub const PREFIX: &[u8] = b"OER-DIAG:01";
pub const MAX_PAYLOAD: usize = 16 + 48 + 64;

/// Closing the diagnostic sequence makes loss of a whole panic record
/// observable even when the following boot itself succeeded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootSmokePassed;

impl Event for BootSmokePassed {
    const KIND: Kind = Kind::new(Domain::Platform, 2);
    const CHANNEL: Channel = Channel::new(Domain::Platform, 0);

    fn encode(&self) -> [u32; 2] {
        [0, 0]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        (words == [0, 0]).then_some(Self)
    }
}

/// A borrowed view of the previous panic. The retained record stays its
/// platform owner's; the same view can be sent by any staged image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreviousPanic<'a> {
    pub hart: u8,
    pub in_interrupt: bool,
    pub interrupted_pc: Option<u32>,
    pub line: u32,
    pub column: u32,
    pub file: &'a str,
    /// `None` means the panic message needed formatting and was not kept.
    pub message: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Record<'a> {
    PreviousPanic(PreviousPanic<'a>),
    Event { kind: Kind, words: [u32; 2] },
}

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
        // Platform retained limits: 48 bytes of file suffix and 64 of the
        // static message prefix. Refuse an invalid caller before emitting.
        assert!(panic.file.len() <= 48);
        assert!(panic.message.is_none_or(|message| message.len() <= 64));
        let mut crc = self.begin(1)?;
        let flags = u8::from(panic.in_interrupt)
            | (u8::from(panic.interrupted_pc.is_some()) << 1)
            | (u8::from(panic.message.is_some()) << 2);
        self.hex(&[panic.hart, flags], &mut crc)?;
        self.hex(&panic.line.to_le_bytes(), &mut crc)?;
        self.hex(&panic.column.to_le_bytes(), &mut crc)?;
        self.hex(&panic.interrupted_pc.unwrap_or(0).to_le_bytes(), &mut crc)?;
        self.hex(
            &[
                panic.file.len() as u8,
                panic.message.map_or(0, str::len) as u8,
            ],
            &mut crc,
        )?;
        self.hex(panic.file.as_bytes(), &mut crc)?;
        self.hex(panic.message.unwrap_or("").as_bytes(), &mut crc)?;
        self.end(crc)
    }

    pub fn event<T: Event>(&mut self, event: &T) -> Result<(), E> {
        let mut crc = self.begin(2)?;
        self.hex(&T::KIND.raw().to_le_bytes(), &mut crc)?;
        for word in event.encode() {
            self.hex(&word.to_le_bytes(), &mut crc)?;
        }
        self.end(crc)
    }

    fn begin(&mut self, tag: u8) -> Result<u16, E> {
        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);
        // Recover line framing after a partial failed write. Its consumed
        // sequence still exposes the loss to the host.
        (self.write)(b"\r\nOER-DIAG:01")?;
        let mut crc = 0xffff;
        self.hex(&sequence.to_le_bytes(), &mut crc)?;
        self.hex(&[tag], &mut crc)?;
        Ok(crc)
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "nibbles are at most 15; chunks fit the fixed escape buffer"
    )]
    fn hex(&mut self, bytes: &[u8], crc: &mut u16) -> Result<(), E> {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut escaped = [0; 16];
        for chunk in bytes.chunks(escaped.len() / 2) {
            for (index, &byte) in chunk.iter().enumerate() {
                *crc = checksum(*crc, byte);
                escaped[index * 2] = HEX[usize::from(byte >> 4)];
                escaped[index * 2 + 1] = HEX[usize::from(byte & 15)];
            }
            (self.write)(&escaped[..chunk.len() * 2])?;
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
    let mut fields = Fields(protected);
    let sequence = u32::from_le_bytes(fields.array()?);
    let [tag] = fields.array()?;
    let record = match tag {
        1 => {
            let [hart, flags] = fields.array()?;
            let line = u32::from_le_bytes(fields.array()?);
            let column = u32::from_le_bytes(fields.array()?);
            let pc = u32::from_le_bytes(fields.array()?);
            let [file_length, message_length] = fields.array()?;
            if flags & !7 != 0
                || file_length > 48
                || message_length > 64
                || (flags & 4 == 0 && message_length != 0)
                || (flags & 2 == 0 && pc != 0)
            {
                return Err(DecodeError::Payload);
            }
            let text = |bytes| core::str::from_utf8(bytes).map_err(|_| DecodeError::Payload);
            let file = text(fields.bytes(usize::from(file_length))?)?;
            let message = text(fields.bytes(usize::from(message_length))?)?;
            Record::PreviousPanic(PreviousPanic {
                hart,
                in_interrupt: flags & 1 != 0,
                interrupted_pc: (flags & 2 != 0).then_some(pc),
                line,
                column,
                file,
                message: if flags & 4 != 0 { Some(message) } else { None },
            })
        }
        2 => Record::Event {
            kind: Kind::from_raw(u16::from_le_bytes(fields.array()?))
                .ok_or(DecodeError::UnknownRecord)?,
            words: [
                u32::from_le_bytes(fields.array()?),
                u32::from_le_bytes(fields.array()?),
            ],
        },
        _ => return Err(DecodeError::UnknownRecord),
    };
    if !fields.0.is_empty() {
        return Err(DecodeError::Payload);
    }
    Ok(Some((sequence, record)))
}

struct Fields<'a>(&'a [u8]);

impl<'a> Fields<'a> {
    fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let (field, rest) = self
            .0
            .split_first_chunk::<N>()
            .ok_or(DecodeError::Payload)?;
        self.0 = rest;
        Ok(*field)
    }

    fn bytes(&mut self, length: usize) -> Result<&'a [u8], DecodeError> {
        let (field, rest) = self
            .0
            .split_at_checked(length)
            .ok_or(DecodeError::Payload)?;
        self.0 = rest;
        Ok(field)
    }
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests;
