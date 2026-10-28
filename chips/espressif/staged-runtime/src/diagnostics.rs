//! The one conversion of a retained panic to a boot-console diagnostic.
//! This reads the existing record; it changes neither capture nor storage.

pub use oer_diagnostics::{BootSmokePassed, Console};

pub fn previous_panic<F, E>(
    console: &mut Console<F>,
    record: &crate::panic::PanicRecord,
) -> Result<(), oer_diagnostics::record::EncodeError<E>>
where
    F: FnMut(&[u8]) -> Result<(), E>,
{
    console.previous_panic(&oer_diagnostics::PreviousPanic {
        hart: record.hart,
        in_interrupt: record.in_interrupt,
        interrupted_pc: record.interrupted_pc,
        line: record.line,
        column: record.column,
        file: record.file(),
        message: record.message(),
    })
}

/// Writes the retained panic as one frame of the shared note sequence, for an
/// image that writes its other diagnostics with [`oer_diagnostics::emit_note`].
pub fn emit_previous_panic<F, E>(
    write: F,
    record: &crate::panic::PanicRecord,
) -> Result<(), oer_diagnostics::record::EncodeError<E>>
where
    F: FnMut(&[u8]) -> Result<(), E>,
{
    oer_diagnostics::emit_previous_panic(
        &oer_diagnostics::PreviousPanic {
            hart: record.hart,
            in_interrupt: record.in_interrupt,
            interrupted_pc: record.interrupted_pc,
            line: record.line,
            column: record.column,
            file: record.file(),
            message: record.message(),
        },
        write,
    )
}
