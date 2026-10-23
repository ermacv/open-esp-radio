//! The one conversion of a retained panic to a boot-console diagnostic.
//! This reads the existing record; it changes neither capture nor storage.

pub use oer_diagnostics::{BootSmokePassed, Console};

pub fn previous_panic<F, E>(
    console: &mut Console<F>,
    record: &crate::panic::PanicRecord,
) -> Result<(), E>
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
