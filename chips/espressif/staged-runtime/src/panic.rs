//! Panic metadata and numeric runtime faults in the common retained format.
mod fault;
use crate::retained_fault;
#[cfg(all(target_arch = "riscv32", chip_vectors))]
use crate::retained_fault::Claim;
pub use fault::FaultReason;
const INVALID_HART: u32 = 2;
use oer_diagnostics::fault::{Class, Domain, Record};
/// Bytes of the location's file path kept: its end, which names the file.
pub const FILE_BYTES: usize = 48;
/// Bytes of a static panic message kept: its start.
pub const MESSAGE_BYTES: usize = 64;

/// Metadata safe for the image's panic hook to read. Task context keeps the
/// caller's metadata; interrupt context leaves it empty, because compiler
/// location and format constants may be in unavailable cached memory. The
/// hook's code and every state object it reads must themselves be in SRAM or
/// retained memory, and it must not acquire a lock.
pub struct PanicDetails<'a> {
    pub file: &'a str,
    pub line: u32,
    pub column: u32,
    pub message: Option<&'a str>,
}

/// One panic, as the next boot reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PanicRecord {
    pub hart: u8,
    /// The panic ran on the hart's interrupt stack.
    pub in_interrupt: bool,
    /// `mepc` when [`Self::in_interrupt`]: the interrupted instruction.
    pub interrupted_pc: Option<u32>,
    pub line: u32,
    pub column: u32,
    file: [u8; FILE_BYTES],
    file_length: u8,
    message: Message,
}

/// A panic's message as recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Message {
    /// The start of a static message.
    Static([u8; MESSAGE_BYTES], u8),
    /// A message with arguments, which the entry does not format.
    Formatted,
}

impl PanicRecord {
    /// The end of the location's file path, as recorded.
    pub fn file(&self) -> &str {
        let bytes = self
            .file
            .get(..usize::from(self.file_length))
            .unwrap_or_default();
        // The kept suffix may start inside a UTF-8 sequence; skip to the
        // first boundary.
        let start = (0..bytes.len())
            .find(|&i| {
                bytes
                    .get(i..)
                    .is_some_and(|tail| core::str::from_utf8(tail).is_ok())
            })
            .unwrap_or(bytes.len());
        bytes
            .get(start..)
            .and_then(|tail| core::str::from_utf8(tail).ok())
            .unwrap_or("")
    }

    /// The start of the panic's message when it was a static string; `None`
    /// for a formatted one.
    pub fn message(&self) -> Option<&str> {
        let Message::Static(bytes, length) = &self.message else {
            return None;
        };
        let bytes = bytes.get(..usize::from(*length)).unwrap_or_default();
        // The kept prefix may end inside a UTF-8 sequence; drop that part.
        let kept = (0..=bytes.len())
            .rev()
            .find_map(|i| {
                bytes
                    .get(..i)
                    .and_then(|head| core::str::from_utf8(head).ok())
            })
            .unwrap_or("");
        Some(kept)
    }
}

#[derive(Clone, Copy, bytemuck::AnyBitPattern)]
#[repr(C)]
struct Payload {
    hart: u32,
    in_interrupt: u32,
    interrupted_pc: u32,
    line: u32,
    column: u32,
    file_length: u32,
    message_length: u32,
    file: [u8; FILE_BYTES],
    message: [u8; MESSAGE_BYTES],
}
impl Payload {
    fn bytes(&self) -> [u8; 28 + FILE_BYTES + MESSAGE_BYTES] {
        let mut bytes = [0; 28 + FILE_BYTES + MESSAGE_BYTES];
        let words = [
            self.hart,
            self.in_interrupt,
            self.interrupted_pc,
            self.line,
            self.column,
            self.file_length,
            self.message_length,
        ];
        for (to, from) in bytes.iter_mut().zip(
            words
                .into_iter()
                .flat_map(u32::to_ne_bytes)
                .chain(self.file)
                .chain(self.message),
        ) {
            *to = from;
        }
        bytes
    }
}
crate::retained_static! { pub(crate) static SLOT: Record; }

/// Inspect without consuming. Output acknowledgement, through diagnostics,
/// clears an ordinary panic; a refused output leaves its retained bytes intact.
pub fn peek_previous<P: oer_espressif_platform_port::EarlyText>() -> Option<PanicRecord> {
    let mut slot = retained_fault::peek(&SLOT)?;
    let view = slot.peek().ok()?;
    if view.domain != Domain::Panic as u32 {
        report_fault_early::<P>(&mut slot);
        return None;
    }
    let payload: Payload = view.decode(Domain::Panic).ok()?;
    decode_panic(payload)
}

fn decode_panic(p: Payload) -> Option<PanicRecord> {
    if p.in_interrupt > 1
        || p.file_length > FILE_BYTES as u32
        || p.message_length > MESSAGE_BYTES as u32 + 1
    {
        return None;
    }
    Some(PanicRecord {
        hart: u8::try_from(p.hart).ok()?,
        in_interrupt: p.in_interrupt != 0,
        interrupted_pc: (p.in_interrupt != 0).then_some(p.interrupted_pc),
        line: p.line,
        column: p.column,
        file: p.file,
        file_length: p.file_length as u8,
        message: match p.message_length {
            0 => Message::Formatted,
            n => Message::Static(p.message, n.wrapping_sub(1) as u8),
        },
    })
}

/// Called only after successful output of this exact snapshot.
pub fn clear_previous(record: &PanicRecord) {
    if let Some(mut slot) = retained_fault::peek(&SLOT) {
        let _reported = slot.report(|view| {
            let payload = view.decode::<Payload>(Domain::Panic).map_err(|_| ())?;
            (decode_panic(payload).as_ref() == Some(record))
                .then_some(())
                .ok_or(())
        });
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) fn report_previous_fault<P: oer_espressif_platform_port::EarlyText>() {
    if let Some(mut slot) = retained_fault::peek(&SLOT) {
        report_fault_early::<P>(&mut slot);
    }
}
fn report_fault_early<P: oer_espressif_platform_port::EarlyText>(slot: &mut Record) {
    let _reported = slot.report(|view| {
        if view.domain == Domain::InvalidHart as u32 {
            if view.reason != INVALID_HART || !view.payload.is_empty() {
                return Err(());
            }
            return oer_diagnostics::emit_note(
                &oer_diagnostics::Note::fatal_fail(
                    view.hart,
                    oer_diagnostics::name!("InvalidHart"),
                ),
                |bytes| P::text(bytes).then_some(()).ok_or(()),
            )
            .map_err(|_| ());
        }
        if view.domain == Domain::CpuException as u32 {
            return crate::trap::report_exception(view, |bytes| {
                P::text(bytes).then_some(()).ok_or(())
            });
        }
        if view.domain == Domain::Boot as u32 {
            if view.class != Class::ConfigurationBoot || view.reason != 1 {
                return Err(());
            }
            P::text(b"previous boot failure: ")
                .then_some(())
                .ok_or(())?;
            P::text(view.payload).then_some(()).ok_or(())?;
            return P::text(b"\n").then_some(()).ok_or(());
        }
        let observations = view.decode::<[u32; 4]>(Domain::Runtime).map_err(|_| ())?;
        let reason = FaultReason::from_code(view.reason).ok_or(())?;
        write_named_fault(reason.name(), observations, |bytes| {
            P::text(bytes).then_some(()).ok_or(())
        })
    });
}

#[cfg(all(target_arch = "riscv32", chip_vectors))]
pub(crate) fn with_invalid_hart_record<R>(
    claim: &Claim<'_>,
    raw: u32,
    reset: impl FnOnce() -> R,
) -> R {
    if let Ok(record) = encode_invalid_hart(raw) {
        let _committed = claim.commit(&SLOT, record);
    }
    reset()
}

fn encode_invalid_hart(raw: u32) -> Result<Record, oer_diagnostics::fault::Error> {
    Record::encode(
        Class::HardwareImpossible,
        Domain::InvalidHart,
        INVALID_HART,
        raw,
        0,
        &[0_u8; 0],
    )
}

#[cfg(all(target_arch = "riscv32", chip_vectors))]
pub fn reset_fault<P: oer_espressif_platform_port::Platform>(
    reason: FaultReason,
    observations: [u32; 4],
) -> ! {
    crate::fatal::reset_recording::<P>(|claim, _| {
        if let Ok(record) = Record::encode(
            reason.class(),
            Domain::Runtime,
            reason as u32,
            oer_riscv::hart_id() as u32,
            0,
            &observations,
        ) {
            let _committed = claim.commit(&SLOT, record);
        }
    })
}

#[cfg(all(target_arch = "riscv32", chip_vectors))]
pub fn reset_request<P: oer_espressif_platform_port::Platform, T>(
    error: impl Into<oer_memory::RequestError>,
) -> T {
    let error = error.into();
    reset_fault::<P>(FaultReason::RequestUnanswered, error.observations())
}

fn write_named_fault<E>(
    name: &str,
    observations: [u32; 4],
    mut emit: impl FnMut(&[u8]) -> Result<(), E>,
) -> Result<(), E> {
    emit(b"previous fault ")?;
    emit(name.as_bytes())?;
    emit(b":")?;
    for mut word in observations {
        let mut digits = [0; 8];
        for digit in digits.iter_mut().rev() {
            let nibble = (word & 0xf) as u8;
            *digit = if nibble < 10 {
                b'0'.wrapping_add(nibble)
            } else {
                b'a'.wrapping_add(nibble.wrapping_sub(10))
            };
            word >>= 4;
        }
        emit(b" ")?;
        emit(&digits)?;
    }
    emit(b"\n")
}

fn encode_panic(
    hart: u32,
    in_interrupt: bool,
    mepc: u32,
    details: &PanicDetails<'_>,
) -> Result<Record, oer_diagnostics::fault::Error> {
    let (file, file_length) = file_tail(details.file);
    let (message, message_length) = match details.message {
        Some(message) => {
            let (bytes, length) = message_head(message);
            (bytes, length.wrapping_add(1))
        }
        None => ([0; MESSAGE_BYTES], 0),
    };
    let payload = Payload {
        hart,
        in_interrupt: u32::from(in_interrupt),
        interrupted_pc: if in_interrupt { mepc } else { 0 },
        line: details.line,
        column: details.column,
        file_length,
        message_length,
        file,
        message,
    };
    Record::encode(
        Class::RustPanic,
        Domain::Panic,
        1,
        hart,
        u32::from(in_interrupt),
        &payload.bytes(),
    )
}

#[doc(hidden)]
#[cfg(all(target_arch = "riscv32", chip_vectors))]
pub fn handle<'i, P: oer_espressif_platform_port::Platform>(
    info: &'i core::panic::PanicInfo<'_>,
    hook: impl for<'a, 'b> FnOnce(&Claim<'_>, &PanicDetails<'i>, Result<&'b P::Stop<'a>, P::StopError>),
) -> ! {
    let recorded = with_panic_mask(
        || {
            let _ = oer_riscv::swap_interrupt_threshold(u8::MAX);
        },
        oer_riscv::disable_interrupts,
        || {
            let claim = retained_fault::claim().ok()?;
            let hart = oer_riscv::hart_id();
            if oer_espressif_platform_port::Hart::new(hart as u32).is_err() {
                return Some((
                    claim,
                    PanicDetails {
                        file: "",
                        line: 0,
                        column: 0,
                        message: Some("InvalidHart"),
                    },
                ));
            }
            let in_interrupt = crate::stacks::on_interrupt_stack(hart, oer_riscv::stack_pointer());
            let details = if in_interrupt {
                PanicDetails {
                    file: "",
                    line: 0,
                    column: 0,
                    message: None,
                }
            } else {
                let (file, line, column) = info
                    .location()
                    .map_or(("", 0, 0), |l| (l.file(), l.line(), l.column()));
                PanicDetails {
                    file,
                    line,
                    column,
                    message: info.message().as_str(),
                }
            };
            if let Ok(record) = encode_panic(
                hart as u32,
                in_interrupt,
                oer_riscv::read_mepc() as u32,
                &details,
            ) {
                let _committed = claim.commit(&SLOT, record);
            }
            Some((claim, details))
        },
    );
    let Some((claim, details)) = recorded else {
        crate::fatal::reset::<P>()
    };
    crate::fatal::reset_claimed::<P>(&claim, |claim, stop| hook(claim, &details, stop))
}
fn with_panic_mask<T>(
    mask_threshold: impl FnOnce(),
    disable_interrupts: impl FnOnce(),
    record: impl FnOnce() -> T,
) -> T {
    mask_threshold();
    disable_interrupts();
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    record()
}
fn file_tail(file: &str) -> ([u8; FILE_BYTES], u32) {
    let bytes = file.as_bytes();
    let mut kept = [0; FILE_BYTES];
    let mut length = 0;
    for (to, from) in kept
        .iter_mut()
        .zip(bytes.iter().skip(bytes.len().saturating_sub(FILE_BYTES)))
    {
        *to = *from;
        length += 1;
    }
    (kept, length)
}

/// The first [`MESSAGE_BYTES`] of `message`, zero-padded, and how many there
/// are; like [`file_tail`], without an index.
fn message_head(message: &str) -> ([u8; MESSAGE_BYTES], u32) {
    let mut kept = [0; MESSAGE_BYTES];
    let mut length = 0;
    for (to, from) in kept.iter_mut().zip(message.as_bytes()) {
        *to = *from;
        length += 1;
    }
    (kept, length)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_hart_keeps_its_full_raw_observation_and_registered_reason() {
        let mut record = encode_invalid_hart(u32::MAX).unwrap();
        let view = record.peek().unwrap();
        assert_eq!(view.class, Class::HardwareImpossible);
        assert_eq!(view.domain, Domain::InvalidHart as u32);
        assert_eq!(view.reason, INVALID_HART);
        assert_eq!(view.hart, u32::MAX);
        assert!(record.report(|_| Err(())).is_err());
        assert_eq!(record.peek().unwrap().hart, u32::MAX);
        record.report(|_| Ok::<_, ()>(())).unwrap();
        assert!(record.peek().is_err());
    }
    #[test]
    fn panic_metadata_round_trips_without_consuming_on_output_refusal() {
        let details = PanicDetails {
            file: "src/first.rs",
            line: 17,
            column: 4,
            message: Some("first panic"),
        };
        let mut record = encode_panic(0, true, 0x1234, &details).unwrap();
        let decoded = decode_panic(record.peek().unwrap().decode(Domain::Panic).unwrap()).unwrap();
        assert_eq!(decoded.file(), details.file);
        assert_eq!(decoded.message(), details.message);
        assert_eq!(decoded.interrupted_pc, Some(0x1234));
        assert_eq!((decoded.line, decoded.column), (17, 4));
        assert!(record.report(|_| Err(())).is_err());
        assert!(record.peek().is_ok());
        record.report(|_| Ok::<_, ()>(())).unwrap();
        assert!(record.peek().is_err());
    }
    #[test]
    fn bounded_text_keeps_existing_utf8_policy() {
        let file = "é".repeat(25) + "/lib.rs";
        let message = "a".repeat(MESSAGE_BYTES - 1) + "é";
        let details = PanicDetails {
            file: &file,
            line: 0,
            column: 0,
            message: Some(&message),
        };
        let record = encode_panic(0, false, 0, &details).unwrap();
        let decoded = decode_panic(record.peek().unwrap().decode(Domain::Panic).unwrap()).unwrap();
        assert!(decoded.file().ends_with("/lib.rs"));
        assert_eq!(decoded.message(), Some(&message[..MESSAGE_BYTES - 1]));
    }
    #[test]
    fn every_numeric_fault_preserves_exact_words() {
        for retired in [5, 10, 30] {
            assert_eq!(FaultReason::from_code(retired), None);
        }
        for code in (1..=36)
            .filter(|code| ![5, 10, 30].contains(code))
            .chain(256..=260)
        {
            let reason = FaultReason::from_code(code).unwrap();
            let words = [u32::MAX, 0x80000000, 17, 42];
            let record =
                Record::encode(reason.class(), Domain::Runtime, code, 0, 0, &words).unwrap();
            assert_eq!(
                record
                    .peek()
                    .unwrap()
                    .decode::<[u32; 4]>(Domain::Runtime)
                    .unwrap(),
                words
            );
        }
    }
}
