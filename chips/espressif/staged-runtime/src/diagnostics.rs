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
    })?;
    crate::panic::clear_previous(record);
    Ok(())
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
    )?;
    crate::panic::clear_previous(record);
    Ok(())
}

/// Raw reset cause beside best-effort retained diagnostics. A valid retained
/// record may belong to an older boot and is not proof of this reset's cause.
pub fn reset_cause<E>(
    hart: u32,
    raw: u32,
    mut emit: impl FnMut(&[u8]) -> Result<(), E>,
) -> Result<(), E> {
    fn hex(word: u32) -> [u8; 8] {
        let mut digits = [0; 8];
        let mut word = word;
        for digit in digits.iter_mut().rev() {
            let nibble = (word & 15) as u8;
            *digit = if nibble < 10 {
                b'0'.wrapping_add(nibble)
            } else {
                b'a'.wrapping_add(nibble.wrapping_sub(10))
            };
            word >>= 4;
        }
        digits
    }
    emit(b"reset cause hart=")?;
    emit(&hex(hart))?;
    emit(b" raw=")?;
    emit(&hex(raw))?;
    emit(b" retained=best-effort\n")
}

#[cfg(test)]
mod reset_tests {
    #[test]
    fn reset_output_preserves_unknown_codes_and_propagates_refusal() {
        let mut bytes = Vec::new();
        super::reset_cause(1, 0x3f, |part| {
            bytes.extend_from_slice(part);
            Ok::<_, ()>(())
        })
        .unwrap();
        assert_eq!(
            bytes,
            b"reset cause hart=00000001 raw=0000003f retained=best-effort\n"
        );
        assert_eq!(super::reset_cause(0, 3, |_| Err(7)), Err(7));
    }
}
