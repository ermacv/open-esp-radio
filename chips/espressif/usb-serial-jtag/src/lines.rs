//! Typed diagnostic notes on the console's writer: one note is encoded into a
//! line first, then written whole, so that a line waits for the host once,
//! not once per chunk of its encoding.

use embedded_io_async::Write;
use oer_diagnostics::{MAX_NOTE_LINE, Note, emit_note};

#[cfg(test)]
mod tests;

/// A note could not be written.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoteError<E> {
    /// The encoded line outgrew its buffer; nothing was written.
    TooLong,
    /// The writer refused the line.
    Write(E),
}

/// Writes `note` as one `OER-DIAG:01` line of the boot's shared sequence:
/// encoded first, then handed to `writer` whole.
///
/// # Errors
/// The line does not fit its buffer, or `writer` fails. A failed note still
/// consumed its sequence number, so the host sees the gap.
pub async fn write_note<W: Write>(
    writer: &mut W,
    note: &Note<'_>,
) -> Result<(), NoteError<W::Error>> {
    let mut line = [0; MAX_NOTE_LINE];
    let mut length = 0;
    emit_note(note, |bytes: &[u8]| {
        let end = length + bytes.len();
        line.get_mut(length..end)
            .ok_or(NoteError::TooLong)?
            .copy_from_slice(bytes);
        length = end;
        Ok(())
    })?;
    match line.get(..length) {
        Some(bytes) => writer.write_all(bytes).await.map_err(NoteError::Write),
        None => Err(NoteError::TooLong),
    }
}
