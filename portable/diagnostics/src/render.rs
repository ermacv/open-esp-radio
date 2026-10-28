//! The text the host prints for a note. Firmware never links this module.

use core::fmt::{self, Write};

use crate::note::{Field, Note};

/// Writes `note` as one line without a terminator: the topic's text, then
/// each field as ` key=value`, or as a bare comma-separated value when its
/// key is empty.
pub fn note(note: &Note<'_>, out: &mut impl Write) -> fmt::Result {
    let (prefix, fields) = note.topic.layout();
    out.write_str(prefix)?;
    let mut words = note.words().iter().copied();
    let mut names = note.names().iter().copied();
    for (index, (key, field)) in fields.iter().enumerate() {
        if key.is_empty() {
            if index > 0 {
                out.write_char(',')?;
            }
        } else {
            write!(out, " {key}=")?;
        }
        let mut next = || words.next().ok_or(fmt::Error);
        match *field {
            Field::Dec => write!(out, "{}", next()?)?,
            Field::Hex(width) => write!(out, "{:0width$x}", next()?, width = usize::from(width))?,
            Field::PrefixedHex(width) => {
                write!(out, "{:#0width$x}", next()?, width = usize::from(width))?
            }
            Field::Flag => out.write_str(if next()? != 0 { "true" } else { "false" })?,
            Field::Bit => out.write_str(if next()? != 0 { "1" } else { "0" })?,
            Field::MacTail => {
                let [high, low] = (next()? as u16).to_be_bytes();
                write!(out, "xx:xx:xx:xx:{high:02X}:{low:02X}")?
            }
            Field::Ascii2 => {
                let [high, low] = (next()? as u16).to_be_bytes();
                write!(out, "{}{}", char::from(high), char::from(low))?
            }
            Field::I8x4 => {
                let [a, b, c, d] = next()?.to_be_bytes().map(|byte| byte as i8);
                write!(out, "{a},{b},{c},{d}")?
            }
            Field::Bytes8 | Field::Bytes16 => {
                let words = if matches!(*field, Field::Bytes8) {
                    2
                } else {
                    4
                };
                let length = next()? as usize;
                if length == 0 {
                    out.write_str("-")?;
                }
                let mut left = length;
                for _ in 0..words {
                    for byte in next()?.to_be_bytes() {
                        if let Some(remaining) = left.checked_sub(1) {
                            write!(out, "{byte:02x}")?;
                            left = remaining;
                        }
                    }
                }
            }
            Field::Dash => {
                let (present, value) = (next()?, next()?);
                if present != 0 {
                    write!(out, "{value}")?
                } else {
                    out.write_str("-")?
                }
            }
            Field::HexDash => {
                let (present, value) = (next()?, next()?);
                if present != 0 {
                    write!(out, "{value:#x}")?
                } else {
                    out.write_str("-")?
                }
            }
            Field::SignedDash => {
                let (present, value) = (next()?, next()?);
                if present != 0 {
                    write!(out, "{}", value as i32)?
                } else {
                    out.write_str("-")?
                }
            }
            Field::BitDash => out.write_str(match next()? {
                0 => "-",
                1 => "0",
                _ => "1",
            })?,
            Field::Range => {
                let (present, min, max) = (next()?, next()?, next()?);
                if present != 0 {
                    write!(out, "{min}..{max}")?
                } else {
                    out.write_str("-")?
                }
            }
            Field::SignedRange => {
                let (present, min, max) = (next()?, next()?, next()?);
                if present != 0 {
                    write!(out, "{}..{}", min as i32, max as i32)?
                } else {
                    out.write_str("-")?
                }
            }
            Field::Over => {
                let mut wide = || -> Result<u64, fmt::Error> {
                    let low = u64::from(next()?);
                    Ok(low | u64::from(next()?) << 32)
                };
                let (lower, upper) = (wide()?, wide()?);
                if lower == upper {
                    write!(out, "{lower}")?
                } else {
                    write!(out, "{lower}..{upper}")?
                }
            }
            Field::By => {
                let (a, b) = (next()?, next()?);
                write!(out, "{a}x{b}")?
            }
            Field::Indices => {
                let count = next()? as usize;
                let packed = [
                    next()?.to_be_bytes(),
                    next()?.to_be_bytes(),
                    next()?.to_be_bytes(),
                    next()?.to_be_bytes(),
                ];
                if count == 0 {
                    out.write_str("-")?;
                }
                for (position, index) in packed.as_flattened().iter().take(count).enumerate() {
                    if position > 0 {
                        out.write_char(',')?;
                    }
                    match index {
                        255 => out.write_char('x')?,
                        index => write!(out, "{index}")?,
                    }
                }
            }
            Field::IndexDash => match next()? {
                0 => out.write_str("-")?,
                1 => out.write_str("x")?,
                value => write!(out, "{}", value.checked_sub(2).ok_or(fmt::Error)?)?,
            },
            Field::DiscardsLow | Field::DiscardsHigh => {
                let high = matches!(*field, Field::DiscardsHigh);
                let mut separate = high && next()? != 0;
                let (first, count) = if high { (8, 7) } else { (0, 8) };
                for reason in crate::note::DISCARDS.iter().skip(first).take(count) {
                    let value = next()?;
                    if value != 0 {
                        if separate {
                            out.write_char(',')?;
                        }
                        separate = true;
                        write!(out, "{reason}:{value}")?;
                    }
                }
            }
            Field::Phrase => {
                for character in names.next().ok_or(fmt::Error)?.chars() {
                    out.write_char(if character == '_' { '-' } else { character })?;
                }
            }
            Field::Ipv4 => {
                let [a, b, c, d] = next()?.to_be_bytes();
                write!(out, "{a}.{b}.{c}.{d}")?
            }
            Field::Wide => {
                let low = u64::from(next()?);
                write!(out, "{}", low | u64::from(next()?) << 32)?
            }
            Field::Signed => write!(out, "{}", next()? as i32)?,
            Field::Optional => match next()? {
                0 => out.write_str("None")?,
                value => write!(out, "Some({})", value.checked_sub(1).ok_or(fmt::Error)?)?,
            },
            Field::Name => out.write_str(names.next().ok_or(fmt::Error)?)?,
            Field::Path => {
                // Nested like the value's own `Debug`: `Err(Failed(Run(..)))`.
                let depth = names.len();
                for (index, name) in names.by_ref().enumerate() {
                    if index > 0 {
                        out.write_char('(')?;
                    }
                    out.write_str(name)?;
                }
                for _ in 1..depth {
                    out.write_char(')')?;
                }
                let mut numbers = words.by_ref().peekable();
                if numbers.peek().is_some() {
                    out.write_str(" numbers=")?;
                    for (index, number) in numbers.enumerate() {
                        if index > 0 {
                            out.write_char(',')?;
                        }
                        write!(out, "{number}")?;
                    }
                }
                if note.overflowed() {
                    out.write_str(" path-truncated")?;
                }
            }
        }
    }
    Ok(())
}

/// Frames missing before a record: the stream expected `expected` and the
/// record carries `received`. A repeated or reordered frame is the same gap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Gap {
    pub expected: u32,
    pub received: u32,
}

/// A record the stream took, and the gap before it when frames were missed.
#[derive(Clone, Copy, Debug)]
pub struct Next<'a> {
    pub gap: Option<Gap>,
    pub record: crate::Record<'a>,
}

/// The host's view of one boot's diagnostic lines: each one is decoded and
/// numbered in order. Other console lines are not its.
#[derive(Clone, Copy, Debug)]
pub struct Stream {
    /// The number the next line must carry; `None` takes the first line's.
    next: Option<u32>,
}

impl Default for Stream {
    fn default() -> Self {
        Self::new()
    }
}

impl Stream {
    /// A stream that starts at a boot's first line.
    pub const fn new() -> Self {
        Self { next: Some(0) }
    }

    /// A stream that joins a boot in the middle: its first line sets the
    /// number to expect.
    pub const fn unanchored() -> Self {
        Self { next: None }
    }

    /// The target restarted: its numbering starts again.
    pub fn restart(&mut self) {
        self.next = Some(0);
    }

    /// `line` (without its line end) as the next record of the boot, or
    /// `None` when it is not a diagnostic line. A record that follows a gap
    /// is still returned, with the gap.
    pub fn line<'a>(
        &mut self,
        line: &[u8],
        scratch: &'a mut [u8; crate::MAX_PAYLOAD + 7],
    ) -> Result<Option<Next<'a>>, crate::DecodeError> {
        let Some((sequence, record)) = crate::decode(line, scratch)? else {
            return Ok(None);
        };
        let expected = self.next.unwrap_or(sequence);
        self.next = Some(sequence.wrapping_add(1));
        let gap = (sequence != expected).then_some(Gap {
            expected,
            received: sequence,
        });
        Ok(Some(Next { gap, record }))
    }
}

#[cfg(test)]
mod tests {
    use super::{Gap, Stream};
    use crate::{Console, Note, Record};

    fn frame(sequence: u32) -> std::vec::Vec<u8> {
        let mut bytes = std::vec::Vec::new();
        Console::starting_at(
            |chunk: &[u8]| {
                bytes.extend_from_slice(chunk);
                Ok::<_, ()>(())
            },
            sequence,
        )
        .note(&Note::rearm(1, 20, 20_000))
        .unwrap();
        // The line itself, without its line ends.
        bytes[2..bytes.len() - 2].to_vec()
    }

    fn take(stream: &mut Stream, sequence: u32) -> Option<Gap> {
        let mut scratch = [0; crate::MAX_PAYLOAD + 7];
        let next = stream
            .line(&frame(sequence), &mut scratch)
            .unwrap()
            .unwrap();
        assert!(matches!(next.record, Record::Note(_)), "the record is kept");
        next.gap
    }

    #[test]
    fn a_record_after_a_gap_is_returned_with_the_gap() {
        let mut stream = Stream::new();
        assert_eq!(take(&mut stream, 0), None);
        assert_eq!(
            take(&mut stream, 2),
            Some(Gap {
                expected: 1,
                received: 2
            })
        );
        assert_eq!(take(&mut stream, 3), None);
    }
}
