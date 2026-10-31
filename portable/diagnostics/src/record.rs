//! The transport-independent diagnostic record codec. Its output starts
//! with the record tag; callers put these exact bytes in their own frame.

use crate::note::{MAX_NAME, MAX_NAME_BYTES, MAX_NAMES, MAX_WORDS, Note, Topic};
use crate::path::{MAX_PATH_NAMES, MAX_PATH_WORDS};
use oer_trace::{Channel, Domain, Event, Kind};

/// The largest payload after the tag: a previous panic is 16 + 48 + 64, a
/// note 4 + its words and names with their lengths.
pub const MAX_PAYLOAD: usize = 4 + 4 * MAX_WORDS + MAX_NAMES + MAX_NAME_BYTES;

/// Closing the diagnostic sequence makes loss of a whole panic record
/// observable even when the following boot itself succeeded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootSmokePassed;

impl Event for BootSmokePassed {
    const KIND: Kind = Kind::new(Domain::Platform, 2);
    const CHANNEL: Channel = const {
        match Channel::new(Domain::Platform, 0) {
            Ok(channel) => channel,
            Err(_) => panic!("invalid trace channel"),
        }
    };

    fn encode(&self) -> [u32; 2] {
        [0, 0]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        (words == [0, 0]).then_some(Self)
    }
}

/// A secondary hart reported ready on its own stacks: its hart and the stack
/// pointer it reported. The boot console renders it as
/// `OER-HART ready hart=<n> sp=0x<sp>`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SecondaryReady {
    pub hart: u32,
    pub sp: u32,
}

impl Event for SecondaryReady {
    const KIND: Kind = Kind::new(Domain::Platform, 3);
    const CHANNEL: Channel = const {
        match Channel::new(Domain::Platform, 0) {
            Ok(channel) => channel,
            Err(_) => panic!("invalid trace channel"),
        }
    };

    fn encode(&self) -> [u32; 2] {
        [self.hart, self.sp]
    }

    fn decode([hart, sp]: [u32; 2]) -> Option<Self> {
        Some(Self { hart, sp })
    }
}

/// A request one hart's task sent a task on another hart, and its answer:
/// the answering hart, the request and the answer. The boot console
/// renders it as `OER-HART exchange hart=<n> request=<r> answer=<a>`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HartExchange {
    pub hart: u16,
    pub request: u16,
    pub answer: u32,
}

impl Event for HartExchange {
    const KIND: Kind = Kind::new(Domain::Platform, 4);
    const CHANNEL: Channel = const {
        match Channel::new(Domain::Platform, 0) {
            Ok(channel) => channel,
            Err(_) => panic!("invalid trace channel"),
        }
    };

    fn encode(&self) -> [u32; 2] {
        [
            (u32::from(self.hart) << 16) | u32::from(self.request),
            self.answer,
        ]
    }

    fn decode([pair, answer]: [u32; 2]) -> Option<Self> {
        Some(Self {
            hart: (pair >> 16) as u16,
            request: (pair & 0xffff) as u16,
            answer,
        })
    }
}

/// A known-answer operation one hart's task asked a task on another hart to
/// run on its hardware, and whether its result matched: the answering hart
/// and the verdict. The boot console renders it as
/// `OER-HART aes hart=<n> ok` or `OER-HART aes hart=<n> mismatch`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HartAes {
    pub hart: u16,
    pub matched: bool,
}

impl Event for HartAes {
    const KIND: Kind = Kind::new(Domain::Platform, 5);
    const CHANNEL: Channel = const {
        match Channel::new(Domain::Platform, 0) {
            Ok(channel) => channel,
            Err(_) => panic!("invalid trace channel"),
        }
    };

    fn encode(&self) -> [u32; 2] {
        [u32::from(self.hart), u32::from(self.matched)]
    }

    fn decode([hart, matched]: [u32; 2]) -> Option<Self> {
        Some(Self {
            hart: u16::try_from(hart).ok()?,
            matched: match matched {
                0 => false,
                1 => true,
                _ => return None,
            },
        })
    }
}

/// The liveness rounds one hart's supervisor completed with a task on
/// another hart so far: the supervised hart and the count. The boot console
/// renders it as `OER-HART liveness hart=<n> rounds=<r>`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HartLiveness {
    pub hart: u16,
    pub rounds: u32,
}

impl Event for HartLiveness {
    const KIND: Kind = Kind::new(Domain::Platform, 6);
    const CHANNEL: Channel = const {
        match Channel::new(Domain::Platform, 0) {
            Ok(channel) => channel,
            Err(_) => panic!("invalid trace channel"),
        }
    };

    fn encode(&self) -> [u32; 2] {
        [u32::from(self.hart), self.rounds]
    }

    fn decode([hart, rounds]: [u32; 2]) -> Option<Self> {
        Some(Self {
            hart: u16::try_from(hart).ok()?,
            rounds,
        })
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
    Note(Note<'a>),
}

/// Maximum encoded record: its tag and the largest note payload.
pub const MAX_RECORD_BYTES: usize = 1 + MAX_PAYLOAD;

/// Invalid arbitrary retained-text input. Lengths are UTF-8 byte counts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanicTextError {
    FileTooLong { bytes: usize },
    MessageTooLong { bytes: usize },
}

/// An invalid input is refused before writing; a sink failure is preserved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncodeError<E> {
    Input(PanicTextError),
    Sink(E),
}

/// Emit exactly one tagged record as raw bytes, without sequence, CRC,
/// escaping or any transport header. The sink must accept the complete slice
/// or return its own error. HIL consumers can collect these bytes in their
/// existing bounded payload buffer; the line console streams them directly.
/// Invalid retained-text input is rejected before calling the sink.
#[inline]
pub fn encode<E>(
    record: &Record<'_>,
    write: impl FnMut(&[u8]) -> Result<(), E>,
) -> Result<(), EncodeError<E>> {
    match record {
        Record::PreviousPanic(panic) => encode_previous_panic(panic, write),
        Record::Event { kind, words } => {
            encode_event_words(*kind, || *words, write).map_err(EncodeError::Sink)
        }
        Record::Note(note) => encode_note(note, write).map_err(EncodeError::Sink),
    }
}

// Concrete entry points let the console stream a known variant without
// constructing the larger Record enum on the firmware stack. The public
// enum encoder above uses exactly these same field encoders.
#[inline]
pub(super) fn encode_previous_panic<E>(
    panic: &PreviousPanic<'_>,
    mut write: impl FnMut(&[u8]) -> Result<(), E>,
) -> Result<(), EncodeError<E>> {
    validate_previous_panic(panic).map_err(EncodeError::Input)?;
    let mut write = |bytes: &[u8]| write(bytes).map_err(EncodeError::Sink);
    write(&[1])?;
    let flags = u8::from(panic.in_interrupt)
        | (u8::from(panic.interrupted_pc.is_some()) << 1)
        | (u8::from(panic.message.is_some()) << 2);
    write(&[panic.hart, flags])?;
    write(&panic.line.to_le_bytes())?;
    write(&panic.column.to_le_bytes())?;
    write(&panic.interrupted_pc.unwrap_or(0).to_le_bytes())?;
    write(&[
        panic.file.len() as u8,
        panic.message.map_or(0, str::len) as u8,
    ])?;
    write(panic.file.as_bytes())?;
    write(panic.message.unwrap_or("").as_bytes())
}

/// Tag 3: topic u16, word count u8, name count u8, the words, then each name
/// as a length u8 and its bytes.
#[inline]
pub(super) fn encode_note<E>(
    note: &Note<'_>,
    mut write: impl FnMut(&[u8]) -> Result<(), E>,
) -> Result<(), E> {
    write(&[3])?;
    write(&note.topic.raw().to_le_bytes())?;
    // The top bit of the name count says the path lost something.
    write(&[
        note.words().len() as u8,
        note.names().len() as u8 | if note.overflowed() { 0x80 } else { 0 },
    ])?;
    for word in note.words() {
        write(&word.to_le_bytes())?;
    }
    for name in note.names() {
        write(&[name.len() as u8])?;
        write(name.as_bytes())?;
    }
    Ok(())
}

#[inline]
pub(super) fn encode_event<T: Event, E>(
    event: &T,
    write: impl FnMut(&[u8]) -> Result<(), E>,
) -> Result<(), E> {
    encode_event_words(T::KIND, || event.encode(), write)
}

#[inline]
fn encode_event_words<E>(
    kind: Kind,
    words: impl FnOnce() -> [u32; 2],
    mut write: impl FnMut(&[u8]) -> Result<(), E>,
) -> Result<(), E> {
    write(&[2])?;
    write(&kind.raw().to_le_bytes())?;
    for word in words() {
        write(&word.to_le_bytes())?;
    }
    Ok(())
}

#[inline]
pub(super) fn validate_previous_panic(panic: &PreviousPanic<'_>) -> Result<(), PanicTextError> {
    if panic.file.len() > 48 {
        return Err(PanicTextError::FileTooLong {
            bytes: panic.file.len(),
        });
    }
    if let Some(message) = panic.message
        && message.len() > 64
    {
        return Err(PanicTextError::MessageTooLong {
            bytes: message.len(),
        });
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeError {
    Truncated,
    UnknownRecord,
    Payload,
}

/// Decode exactly one raw tagged record, borrowing its textual fields.
/// An unknown tag/kind, incomplete record, invalid fields/UTF-8 or trailing
/// bytes is an error. The containing transport owns framing and integrity.
pub fn decode(bytes: &[u8]) -> Result<Record<'_>, DecodeError> {
    let mut fields = Fields(bytes);
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
        3 => {
            let topic = Topic::from_raw(u16::from_le_bytes(fields.array()?))
                .ok_or(DecodeError::UnknownRecord)?;
            let [count, name_count] = fields.array()?;
            let overflow = name_count & 0x80 != 0;
            let name_count = name_count & 0x7f;
            let (words, names) = (usize::from(count), usize::from(name_count));
            let (extra_words, extra_names) = if topic.has_path() {
                (MAX_PATH_WORDS, MAX_PATH_NAMES)
            } else {
                (0, 0)
            };
            // The topics' const checks keep both ranges inside the arrays.
            if !(topic.word_count()
                ..=topic
                    .word_count()
                    .checked_add(extra_words)
                    .ok_or(DecodeError::Payload)?)
                .contains(&words)
                || !(topic.name_count()
                    ..=topic
                        .name_count()
                        .checked_add(extra_names)
                        .ok_or(DecodeError::Payload)?)
                    .contains(&names)
                || words > MAX_WORDS
                || names > MAX_NAMES
                || (overflow && !topic.has_path())
            {
                return Err(DecodeError::Payload);
            }
            let mut words = [0; MAX_WORDS];
            for word in words.iter_mut().take(usize::from(count)) {
                *word = u32::from_le_bytes(fields.array()?);
            }
            let mut names = [""; MAX_NAMES];
            let mut name_bytes = 0_usize;
            for name in names.iter_mut().take(usize::from(name_count)) {
                let [length] = fields.array()?;
                name_bytes = name_bytes
                    .checked_add(usize::from(length))
                    .ok_or(DecodeError::Payload)?;
                if usize::from(length) > MAX_NAME || name_bytes > MAX_NAME_BYTES {
                    return Err(DecodeError::Payload);
                }
                *name = core::str::from_utf8(fields.bytes(usize::from(length))?)
                    .map_err(|_| DecodeError::Payload)?;
            }
            Record::Note(Note {
                topic,
                words,
                count,
                names,
                name_count,
                name_bytes: name_bytes as u8,
                overflow,
            })
        }
        _ => return Err(DecodeError::UnknownRecord),
    };
    if !fields.0.is_empty() {
        return Err(DecodeError::Payload);
    }
    Ok(record)
}

struct Fields<'a>(&'a [u8]);

impl<'a> Fields<'a> {
    fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let (field, rest) = self
            .0
            .split_first_chunk::<N>()
            .ok_or(DecodeError::Truncated)?;
        self.0 = rest;
        Ok(*field)
    }

    fn bytes(&mut self, length: usize) -> Result<&'a [u8], DecodeError> {
        let (field, rest) = self
            .0
            .split_at_checked(length)
            .ok_or(DecodeError::Truncated)?;
        self.0 = rest;
        Ok(field)
    }
}

#[cfg(test)]
mod tests;
